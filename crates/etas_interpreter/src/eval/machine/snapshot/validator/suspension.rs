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
            let parent = revisions.execution_snapshot(node.parent())?;
            if !Arc::ptr_eq(&caller, &parent.checked) {
                return Err(
                    "suspended program is referenced from a different checked parent".into(),
                );
            }
            let node = match node {
                crate::orchestration::SuspendedExecutionSnapshot::Program(node) => node,
                crate::orchestration::SuspendedExecutionSnapshot::Task(node) => {
                    self.validate_task_suspension(node, &parent, snapshot, all_scopes)?;
                    continue;
                }
            };
            if (!node.parent_scoped && node.parent_handler_floor != self.inherited_handlers)
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
            .with_revisions(self.revisions)
            .with_execution_revision(parent.revision.as_deref());
            parent_validator.frame_definitions = self.frame_definitions.clone();
            parent_validator.tasks = self.tasks;
            parent_validator.task_identity = self.task_identity;
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
            .with_revisions(self.revisions)
            .with_execution_revision(native.revision.as_deref());
            validator.frame_definitions = self.frame_definitions.clone();
            validator.tasks = self.tasks;
            validator.task_identity = self.task_identity;
            validator.frame_owners = self.frame_owners.clone();
            validator.suspension_references = self.suspension_references.clone();
            validator.residual_references = self.residual_references.clone();
            node.model_policy.validate()?;
            node.boundary.parent_model_policy.validate()?;
            if let Some(request) = &node.agent_context {
                validator.agent_request_context(request)?;
            }
            if let Some(request) = &node.boundary.parent_agent_context {
                parent_validator.agent_request_context(request)?;
            }
            let mut local_scopes = BTreeSet::new();
            validator.validate_local_frames(&node.frames, &mut local_scopes)?;
            validator.continuation(
                &node.continuation,
                "suspended program continuation",
                &mut local_scopes,
            )?;
            validator.validate_residual_frame_coverage(
                &node.frames,
                &snapshot.residual,
                Some(&node.continuation),
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
        for node in &snapshot.suspended_programs {
            if let crate::orchestration::SuspendedExecutionSnapshot::Task(node) = node {
                if !all_scopes.contains(&node.handler) {
                    return Err("task suspension has no owning Handler boundary".into());
                }
            }
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
            let node = match node {
                crate::orchestration::SuspendedExecutionSnapshot::Program(node) => node,
                crate::orchestration::SuspendedExecutionSnapshot::Task(node) => {
                    let parent = revisions.execution_snapshot(&node.owner)?;
                    let projection = crate::plan::code::ActionProjection::restore_checkpoint(
                        &node.projection,
                        &parent,
                        revisions,
                    )?;
                    targets[index] = Some(projection.target().clone());
                    continue;
                }
            };
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

    fn validate_task_suspension(
        &self,
        node: &crate::orchestration::SuspendedTaskHandlerSnapshot,
        owner: &crate::value::CheckedExecutionProgram,
        graph: &MachineSnapshot,
        all_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        use crate::orchestration::{TaskHandlerWaitSnapshot as Wait, TaskValueSnapshot};
        if self.task_identity != Some(node.owner_task) {
            return Err("task suspension belongs to a different owning task".into());
        }
        let scheduler = self
            .tasks
            .ok_or("task suspension has no scheduler metadata")?;
        scheduler.validate_value(TaskValueSnapshot::Task {
            owner: node.child.owner.0,
            task: node.child.task.0,
        })?;
        if node.owner_handler_floor >= node.handler_base {
            return Err("task suspension has no enclosing Handler scope".into());
        }
        node.model_policy.validate()?;
        let validator = self.task_segment_validator(owner);
        let mut saved = MachineSnapshot::capture(node.frames.clone());
        saved.residual = graph.residual.clone();
        let mut local_scopes = BTreeSet::new();
        validator.validate_program_frames(&saved, &mut local_scopes)?;
        let active = validator
            .active_program(&saved)?
            .unwrap_or_else(|| owner.clone());
        if node.active_scoped
            != (node.owner_scoped
                || saved
                    .frames
                    .iter()
                    .any(|frame| matches!(frame, MachineFrameSnapshot::ProgramReturn(_))))
        {
            return Err("task suspension has inconsistent program boundaries".into());
        }
        let active_validator = self.task_segment_validator(&active);
        let pending = match &node.wait {
            Wait::Ready(input) => {
                saved.input = Some(input.clone());
                validator.validate_machine_input(&saved)?;
                input.continuation_root()
            }
            Wait::Await {
                target,
                continuation,
            }
            | Wait::Transfer {
                target,
                continuation,
                ..
            } => {
                scheduler.validate_value(TaskValueSnapshot::Task {
                    owner: target.owner.0,
                    task: target.task.0,
                })?;
                Some(continuation)
            }
            Wait::Run { scope } => {
                scheduler.validate_value(TaskValueSnapshot::Scheduler { owner: scope.0 })?;
                None
            }
        };
        if let Some(pending) = pending {
            active_validator.continuation(pending, "suspended task wait", &mut local_scopes)?;
        }
        let active_scopes = validator.validate_handler_stack(
            &saved,
            &node.handlers,
            node.retries.attempts.len(),
            0,
        )?;
        let mut unwind = Vec::new();
        let mut retries = Vec::new();
        if let Some(pending) = pending {
            Self::continuation_unwind_order(pending, &mut unwind, &mut retries);
        }
        Self::frame_unwind_order(&node.frames, &mut unwind, &mut retries);
        if unwind != active_scopes.iter().rev().copied().collect::<Vec<_>>()
            || retries
                != node
                    .retries
                    .attempts
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
        {
            return Err("task suspension dynamic scopes differ from its continuation".into());
        }
        for scope in local_scopes {
            if !all_scopes.insert(scope) {
                return Err("task suspension duplicates a Handler scope".into());
            }
        }
        Ok(())
    }
    fn task_segment_validator<'a>(
        &'a self,
        program: &'a crate::value::CheckedExecutionProgram,
    ) -> SnapshotValidator<'a> {
        let mut validator = SnapshotValidator::new(
            &program.checked,
            &program.plan.slots,
            &program.plan.dispatch,
            &program.plan.closures,
            self.limits,
        )
        .with_revisions(self.revisions)
        .with_execution_revision(program.revision.as_deref())
        .with_tasks(self.tasks, self.task_identity);
        validator.frame_definitions = self.frame_definitions.clone();
        validator.frame_owners = self.frame_owners.clone();
        validator.suspension_references = self.suspension_references.clone();
        validator.residual_references = self.residual_references.clone();
        validator
    }
}
