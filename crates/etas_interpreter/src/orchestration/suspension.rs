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
    ParameterDefault(etas_code::descriptor::ParameterDefaultInfo),
}

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionProgramSnapshot {
    pub program: CheckpointProgramReference,
    pub root: ExecutionRootSnapshot,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ActionOriginSnapshot {
    ControllerStep {
        inference: HirExprId,
    },
    Source {
        expr: HirExprId,
        bindings: Vec<(String, TypeId)>,
        type_parameter_bindings: Vec<(TypeId, TypeId)>,
        effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    },
    Standard {
        call: CallTargetSnapshot,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ActionInvocationSnapshot {
    Native {
        native: ExecutionProgramSnapshot,
        origin: ActionOriginSnapshot,
    },
    FromSuspension {
        id: u32,
    },
}

/// Checked bridge identities are resolved against the current prepared code;
/// the saved record carries no executable body or invocation authority.
#[derive(
    Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, etas_core::RetainedSize,
)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckedProjectionSnapshot {
    pub origin: super::CallableProjectionOrigin,
    pub to_candidate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActionEvidenceSnapshot {
    pub origin: ActionInvocationSnapshot,
    pub projections: Vec<CheckedProjectionSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) struct ActionProjectionSnapshot {
    pub source: ActionEvidenceSnapshot,
    pub steps: Vec<CheckedProjectionSnapshot>,
}

#[derive(Clone, Debug)]
pub(crate) enum TaskHandlerWaitSnapshot {
    Completion {
        scope: crate::driver::scheduler::TaskScopeId,
        continuation: ContinuationSnapshot,
        result_type: TypeId,
        span: Span,
    },
    Ready(Box<super::MachineInputSnapshot>),
    Await {
        target: crate::driver::scheduler::TaskHandle,
        continuation: ContinuationSnapshot,
    },
    Run {
        scope: crate::driver::scheduler::TaskScopeId,
    },
    Transfer {
        target: crate::driver::scheduler::TaskHandle,
        token: crate::value::ContinuationTokenValue,
        continuation: ContinuationSnapshot,
        output: TypeId,
        span: Span,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct SuspendedTaskHandlerSnapshot {
    pub child: crate::driver::scheduler::TaskHandle,
    pub owner_task: crate::driver::scheduler::TaskIdentity,
    pub handler: super::HandlerScopeId,
    pub owner: ExecutionProgramSnapshot,
    pub owner_scoped: bool,
    pub owner_handler_floor: usize,
    pub handler_base: usize,
    pub retry_base: usize,
    pub active_scoped: bool,
    pub frames: Vec<MachineFrameSnapshot>,
    pub wait: TaskHandlerWaitSnapshot,
    pub handlers: HandlerSnapshot,
    pub retries: RetrySnapshot,
    pub model_policy: ModelExecutionPolicySnapshot,
    pub projection: ActionProjectionSnapshot,
    pub span: Span,
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
    pub agent_context: Option<Box<super::AgentRequestContextSnapshot>>,
    pub action: ActionInvocationSnapshot,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) enum SuspendedExecutionSnapshot {
    Program(SuspendedProgramSnapshot),
    Task(SuspendedTaskHandlerSnapshot),
}

impl SuspendedExecutionSnapshot {
    pub(crate) fn parent(&self) -> &ExecutionProgramSnapshot {
        match self {
            Self::Program(node) => &node.parent,
            Self::Task(node) => &node.owner,
        }
    }
    pub(crate) fn frames(&self) -> &[MachineFrameSnapshot] {
        match self {
            Self::Program(node) => &node.frames,
            Self::Task(node) => &node.frames,
        }
    }
    pub(crate) fn handlers(&self) -> &HandlerSnapshot {
        match self {
            Self::Program(node) => &node.handlers,
            Self::Task(node) => &node.handlers,
        }
    }
    pub(crate) fn retries(&self) -> &RetrySnapshot {
        match self {
            Self::Program(node) => &node.retries,
            Self::Task(node) => &node.retries,
        }
    }
    fn roots(&self) -> Vec<&ContinuationSnapshot> {
        let mut roots = Vec::new();
        frame_roots(self.frames(), &mut roots);
        match self {
            Self::Program(node) => roots.push(&node.continuation),
            Self::Task(node) => match &node.wait {
                TaskHandlerWaitSnapshot::Ready(input) => roots.extend(input.continuation_root()),
                TaskHandlerWaitSnapshot::Await { continuation, .. }
                | TaskHandlerWaitSnapshot::Completion { continuation, .. }
                | TaskHandlerWaitSnapshot::Transfer { continuation, .. } => {
                    roots.push(continuation)
                }
                TaskHandlerWaitSnapshot::Run { .. } => (),
            },
        }
        roots
    }
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
            ContinuationSnapshot::ProgramResume { id }
            | ContinuationSnapshot::TaskResume { id } => {
                found.insert(*id);
            }
            ContinuationSnapshot::HandleBoundary { inner, .. }
            | ContinuationSnapshot::ReenterHandler { boundary: inner }
            | ContinuationSnapshot::RestoreModelPolicy { inner, .. }
            | ContinuationSnapshot::AgentExecutionScope { inner, .. }
            | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => work.push(inner),
            ContinuationSnapshot::CallBoundary { outer, .. }
            | ContinuationSnapshot::HandlerDispatch { outer, .. } => work.push(outer),
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
        work.extend(self.auxiliary.iter().map(|entry| &entry.continuation));
        work.extend(
            self.input
                .as_deref()
                .and_then(|input| input.continuation_root()),
        );
        for node in &self.suspended_programs {
            work.extend(node.roots());
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
                | ContinuationSnapshot::AgentExecutionScope { inner, .. }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => work.push(inner),
                ContinuationSnapshot::CallBoundary { outer, .. }
                | ContinuationSnapshot::HandlerDispatch { outer, .. } => work.push(outer),
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
        roots.extend(self.auxiliary.iter().map(|entry| &entry.continuation));
        roots.extend(
            self.input
                .as_deref()
                .and_then(|input| input.continuation_root()),
        );
        let root_ids = references(roots);
        let children = self
            .suspended_programs
            .iter()
            .map(|node| references(node.roots()))
            .collect::<Vec<_>>();
        for (node, children) in self.suspended_programs.iter().zip(&children) {
            let SuspendedExecutionSnapshot::Program(node) = node else {
                continue;
            };
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
                .checked_add(usize::from(matches!(
                    node,
                    SuspendedExecutionSnapshot::Program(_)
                )))
                .and_then(|count| {
                    count.checked_add(
                        node.frames()
                            .iter()
                            .filter(|frame| {
                                matches!(
                                    frame,
                                    MachineFrameSnapshot::Call { .. }
                                        | MachineFrameSnapshot::ProgramReturn(_)
                                )
                            })
                            .count(),
                    )
                })
                .ok_or("checkpoint call frame count overflow")?;
        }
        u32::try_from(count).map_err(|_| "checkpoint has too many call frames".into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HandlerActivationSnapshot {
    pub types: Vec<(String, TypeId)>,
    pub parameters: Vec<(TypeId, TypeId)>,
    pub rows: Vec<(String, etas_types::EffectRowRef)>,
    pub arm: etas_hir::HirHandlerArmId,
    pub request: ActionEvidenceSnapshot,
}

impl HandlerActivationSnapshot {
    pub fn bindings(&self) -> crate::control::GenericBindings {
        crate::control::GenericBindings {
            types: self.types.iter().cloned().collect(),
            parameters: self.parameters.iter().copied().collect(),
            rows: self.rows.iter().cloned().collect(),
        }
    }
    pub fn restore(
        &self,
        checked: &etas_frontend::CheckedProgram,
        revisions: &crate::api::code::PreparedRevisionSet,
        outer: &crate::control::GenericBindings,
    ) -> Result<crate::control::handler_activation::HandlerActivation, String> {
        let request =
            crate::control::CheckedActionInvocation::restore_evidence(&self.request, revisions)?;
        let activation = crate::control::handler_activation::HandlerActivation::checked(
            checked, self.arm, request, outer,
        )?;
        if activation.bindings != self.bindings()
            || self.types.len() != activation.bindings.types.len()
            || self.parameters.len() != activation.bindings.parameters.len()
            || self.rows.len() != activation.bindings.rows.len()
        {
            return Err(
                "Handler activation bindings differ from its current checked request".into(),
            );
        }
        Ok(activation)
    }
}
