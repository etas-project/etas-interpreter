use super::*;
use std::sync::Arc;

impl SnapshotValidator<'_> {
    pub(super) fn validate_suspensions(
        &self,
        snapshot: &MachineSnapshot,
        all_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        if snapshot.suspended_programs.is_empty() {
            if !self.suspension_references.borrow().is_empty() {
                return Err("checkpoint references a missing suspended program".into());
            }
            return Ok(());
        }
        if snapshot.suspended_programs.len() > self.limits.max_nodes
            || snapshot.total_call_frames()? > crate::api::MAX_CONFIGURABLE_CALL_DEPTH
        {
            return Err("checkpoint suspended execution graph exceeds resource limits".into());
        }
        self.validate_suspended_actions(snapshot)?;
        let mut owners = std::collections::HashMap::new();
        let mut visited = BTreeSet::new();
        let mut next = 0;
        loop {
            let reference = self.suspension_references.borrow().get(next).cloned();
            let Some((id, caller)) = reference else {
                break;
            };
            next += 1;
            let node = snapshot
                .suspended_programs
                .get(
                    id.checked_sub(1)
                        .ok_or("invalid suspended program identity")? as usize,
                )
                .ok_or("checkpoint references a missing suspended program")?;
            let owner = Arc::as_ptr(&caller) as usize;
            if owners
                .insert(id, owner)
                .is_some_and(|previous| previous != owner)
            {
                return Err(
                    "shared suspended program has conflicting parent program owners".into(),
                );
            }
            if !visited.insert(id) {
                continue;
            }
            let revisions = self
                .revisions
                .ok_or("suspended program requires freshly prepared revisions")?;
            let parent = revisions.execution_snapshot(&node.parent)?;
            if !Arc::ptr_eq(&caller, &parent.checked) {
                return Err(
                    "suspended program is referenced from a different checked parent".into(),
                );
            }
            if (!node.parent_scoped && node.parent_handler_floor != 0)
                || node.parent_handler_floor > node.boundary.handler_floor
                || !matches!(node.boundary.outer, ContinuationSnapshot::BlockValue)
                || node
                    .frames
                    .iter()
                    .any(|frame| matches!(frame, MachineFrameSnapshot::ProgramReturn(_)))
            {
                return Err("suspended program has inconsistent local boundary topology".into());
            }
            let native = super::super::program::resolve_destination(
                &node.boundary,
                &parent.checked,
                self.revisions,
                self.limits,
            )?;
            let mut parent_validator = SnapshotValidator::new(
                &parent.checked,
                &parent.plan.slots,
                &parent.plan.dispatch,
                &parent.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions);
            parent_validator.frame_definitions = self.frame_definitions.clone();
            parent_validator.frame_owners = self.frame_owners.clone();
            parent_validator.suspension_references = self.suspension_references.clone();
            parent_validator.invocation_site(
                &node.boundary.call_site,
                node.boundary.span,
                "suspended invocation origin",
            )?;
            let mut validator = SnapshotValidator::new(
                &native.checked,
                &native.plan.slots,
                &native.plan.dispatch,
                &native.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions);
            validator.frame_definitions = self.frame_definitions.clone();
            validator.frame_owners = self.frame_owners.clone();
            validator.suspension_references = self.suspension_references.clone();
            let mut local_scopes = BTreeSet::new();
            validator.validate_local_frames(&node.frames, &mut local_scopes)?;
            validator.continuation(
                &node.continuation,
                "suspended program continuation",
                &mut local_scopes,
            )?;
            let active = validator.handlers(&node.handlers)?;
            let mut unwind = Vec::new();
            let mut retries = Vec::new();
            Self::continuation_unwind_order(&node.continuation, &mut unwind, &mut retries);
            Self::frame_unwind_order(&node.frames, &mut unwind, &mut retries);
            let expected = active.iter().rev().copied().collect::<Vec<_>>();
            if unwind != expected {
                return Err("suspended handler scope topology mismatch".into());
            }
            if retries
                != node
                    .retries
                    .attempts
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
            {
                return Err("suspended retry scope topology mismatch".into());
            }
            for scope in local_scopes {
                if !all_scopes.insert(scope) {
                    return Err(
                        "suspended program duplicates an active handler scope identity".into(),
                    );
                }
            }
        }
        if visited.len() != snapshot.suspended_programs.len() {
            return Err("suspended program graph is missing a checked parent owner".into());
        }
        Ok(())
    }

    fn validate_suspended_actions(&self, snapshot: &MachineSnapshot) -> Result<(), String> {
        use crate::{control::CheckedActionInvocation, orchestration::ActionInvocationSnapshot};
        let revisions = self
            .revisions
            .ok_or("suspended program requires freshly prepared revisions")?;
        let mut targets: Vec<Option<CheckedActionInvocation>> =
            vec![None; snapshot.suspended_programs.len()];
        for index in snapshot.suspension_order()? {
            let node = &snapshot.suspended_programs[index];
            let parent = revisions.execution_snapshot(&node.parent)?;
            let native = super::super::program::resolve_destination(
                &node.boundary,
                &parent.checked,
                self.revisions,
                self.limits,
            )?;
            let mut proof = match &node.action {
                ActionInvocationSnapshot::Native { .. } => {
                    CheckedActionInvocation::restore_origin(&node.action, revisions)?
                }
                ActionInvocationSnapshot::FromSuspension { id } => targets
                    .get(
                        id.checked_sub(1)
                            .ok_or("invalid forwarded action identity")?
                            as usize,
                    )
                    .and_then(Option::as_ref)
                    .cloned()
                    .ok_or("forwarded action has no validated suspended source")?,
            };
            if !Arc::ptr_eq(&proof.program, &native.checked) {
                return Err("suspended action origin belongs to a different native program".into());
            }
            for edge in super::super::program::action_projections(
                &node.boundary,
                &parent.checked,
                revisions,
            )? {
                proof = proof.project(&edge)?;
            }
            if !Arc::ptr_eq(&proof.program, &parent.checked) {
                return Err("suspended action does not reach its checked parent".into());
            }
            targets[index] = Some(proof);
        }
        Ok(())
    }
}
