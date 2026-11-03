use etas_core::Span;
use etas_hir::{HirExprId, HirItemId};
use etas_types::TypeId;

use super::{
    CallTargetSnapshot, CheckpointProgramReference, ContinuationSnapshot, HandlerSnapshot,
    MachineFrameSnapshot, ModelExecutionPolicySnapshot, ProgramReturnSnapshot, RetrySnapshot,
};

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum ExecutionRootSnapshot {
    Item(HirItemId),
    Lambda(HirExprId),
    StdReference(HirExprId),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionProgramSnapshot {
    pub program: CheckpointProgramReference,
    pub root: ExecutionRootSnapshot,
}

#[derive(Clone, Debug)]
pub(crate) enum ActionOriginSnapshot {
    Source {
        expr: HirExprId,
        bindings: Vec<(String, TypeId)>,
    },
    Standard {
        call: CallTargetSnapshot,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum ActionInvocationSnapshot {
    Native {
        native: ExecutionProgramSnapshot,
        origin: ActionOriginSnapshot,
    },
    FromSuspension {
        id: u32,
    },
}

/// A flat graph node. ProgramResume edges carry table IDs, not nested machines.
#[derive(Clone, Debug)]
pub(crate) struct SuspendedProgramSnapshot {
    pub parent: ExecutionProgramSnapshot,
    pub parent_scoped: bool,
    pub parent_handler_floor: usize,
    pub boundary: Box<ProgramReturnSnapshot>,
    pub frames: Vec<MachineFrameSnapshot>,
    pub continuation: ContinuationSnapshot,
    pub handlers: HandlerSnapshot,
    pub retries: RetrySnapshot,
    pub model_policy: ModelExecutionPolicySnapshot,
    pub action: ActionInvocationSnapshot,
    pub span: Span,
}

fn frame_roots<'a>(frames: &'a [MachineFrameSnapshot], roots: &mut Vec<&'a ContinuationSnapshot>) {
    for frame in frames {
        roots.extend(frame.continuation_roots().into_iter().flatten());
    }
}

fn references(mut work: Vec<&ContinuationSnapshot>) -> Vec<u32> {
    let mut visited = std::collections::HashSet::new();
    let mut found = std::collections::BTreeSet::new();
    while let Some(node) = work.pop() {
        if !visited.insert(std::ptr::from_ref(node)) {
            continue;
        }
        match node {
            ContinuationSnapshot::ProgramResume { id } => {
                found.insert(*id);
            }
            ContinuationSnapshot::HandleBoundary { inner, .. }
            | ContinuationSnapshot::ReenterHandler { boundary: inner }
            | ContinuationSnapshot::RestoreModelPolicy { inner, .. }
            | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => work.push(inner),
            ContinuationSnapshot::CallBoundary { outer, .. }
            | ContinuationSnapshot::HandlerDispatch { outer } => work.push(outer),
            ContinuationSnapshot::Chain { inner, outer } => {
                work.push(outer);
                work.push(inner);
            }
            _ => {}
        }
    }
    found.into_iter().collect()
}

impl ContinuationSnapshot {
    pub(crate) fn suspension_ids(&self) -> Vec<u32> {
        references(vec![self])
    }
}

impl super::MachineSnapshot {
    pub(crate) fn handler_scope_ids(&self) -> Vec<super::HandlerScopeId> {
        let mut work = Vec::new();
        frame_roots(&self.frames, &mut work);
        for node in &self.suspended_programs {
            work.push(&node.continuation);
            frame_roots(&node.frames, &mut work);
        }
        let mut visited = std::collections::HashSet::new();
        let mut found = std::collections::BTreeSet::new();
        while let Some(node) = work.pop() {
            if !visited.insert(std::ptr::from_ref(node)) {
                continue;
            }
            match node {
                ContinuationSnapshot::HandleBoundary {
                    scope_id, inner, ..
                } => {
                    found.insert(*scope_id);
                    work.push(inner);
                }
                ContinuationSnapshot::ReenterHandler { boundary: inner }
                | ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => work.push(inner),
                ContinuationSnapshot::CallBoundary { outer, .. }
                | ContinuationSnapshot::HandlerDispatch { outer } => work.push(outer),
                ContinuationSnapshot::Chain { inner, outer } => {
                    work.push(outer);
                    work.push(inner);
                }
                _ => {}
            }
        }
        found.into_iter().collect()
    }

    /// Validates a reachable DAG and returns a stack-safe, leaves-first restore order.
    pub(crate) fn suspension_order(&self) -> Result<Vec<usize>, String> {
        let mut roots = Vec::new();
        frame_roots(&self.frames, &mut roots);
        let root_ids = references(roots);
        let children = self
            .suspended_programs
            .iter()
            .map(|node| {
                let mut roots = vec![&node.continuation];
                frame_roots(&node.frames, &mut roots);
                references(roots)
            })
            .collect::<Vec<_>>();
        for (node, children) in self.suspended_programs.iter().zip(&children) {
            if let ActionInvocationSnapshot::FromSuspension { id } = node.action {
                if !node.continuation.suspension_ids().contains(&id) || !children.contains(&id) {
                    return Err(
                        "forwarded action evidence is not owned by its pending suspension".into(),
                    );
                }
            }
        }
        let mut colors = vec![0u8; children.len()];
        let mut result = Vec::new();
        let mut pending = root_ids
            .into_iter()
            .rev()
            .map(|id| (id, false))
            .collect::<Vec<_>>();
        while let Some((id, leaving)) = pending.pop() {
            let index = usize::try_from(id)
                .ok()
                .and_then(|id| id.checked_sub(1))
                .filter(|index| *index < children.len())
                .ok_or("checkpoint references a missing suspended program")?;
            if leaving {
                colors[index] = 2;
                result.push(index);
                continue;
            }
            match colors[index] {
                2 => continue,
                1 => return Err("cyclic suspended program checkpoint graph".into()),
                _ => {}
            }
            colors[index] = 1;
            pending.push((id, true));
            pending.extend(children[index].iter().rev().map(|id| (*id, false)));
        }
        if colors.iter().any(|color| *color != 2) {
            return Err("checkpoint contains an unreachable suspended program".into());
        }
        Ok(result)
    }

    pub(crate) fn total_call_frames(&self) -> Result<u32, String> {
        let mut count = self
            .frames
            .iter()
            .filter(|frame| {
                matches!(
                    frame,
                    MachineFrameSnapshot::Call { .. } | MachineFrameSnapshot::ProgramReturn(_)
                )
            })
            .count();
        for node in &self.suspended_programs {
            count = count
                .checked_add(1)
                .and_then(|count| {
                    count.checked_add(
                        node.frames
                            .iter()
                            .filter(|frame| matches!(frame, MachineFrameSnapshot::Call { .. }))
                            .count(),
                    )
                })
                .ok_or("checkpoint call frame count overflow")?;
        }
        u32::try_from(count).map_err(|_| "checkpoint has too many call frames".into())
    }
}
