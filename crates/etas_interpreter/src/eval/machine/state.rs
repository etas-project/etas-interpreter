use etas_core::Span;

use crate::{
    control::{
        ControlSignal, ExecutionFault, PendingCommand, PendingConsole, PendingHostBoundary,
        PendingMemory, PendingModel, PendingPerform, PendingSession, PendingTaskControl,
    },
    value::InterpValue,
};
use etas_host::{HostError, HostValue, ModelResponse, PolicySubject, ToolRequest, ToolResponse};

use super::frame::EvalFrame;
use crate::orchestration::{MachineFrameSnapshot, MachineSnapshot};

pub(crate) enum PendingBoundary {
    Checkpoint { label: Option<String>, span: Span },
    Perform(Box<PendingPerform>),
    Memory(Box<PendingMemory>),
    Session(Box<PendingSession>),
    Console(Box<PendingConsole>),
    Command(Box<PendingCommand>),
    Model(Box<PendingModel>),
    Tool(Box<PendingTool>),
    Host(Box<PendingHostBoundary>),
    Task(Box<PendingTaskControl>),
}

#[derive(Clone, Debug)]
pub(crate) struct PendingTool {
    pub dispatch: PendingToolDispatch,
    pub policy_ref: Option<HostValue>,
    pub policy_subject: PolicySubject,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) enum PendingToolDispatch {
    Host(Box<ToolRequest>),
    Source,
}

pub(super) enum MachineInput {
    EnterProgram {
        call: super::scoped_call::ProgramCall,
        args: Vec<InterpValue>,
        span: Span,
    },
    Signal(ControlSignal),
    ModelResult(Result<ModelResponse, HostError>),
    ToolResult(Result<ToolResponse, HostError>),
    SourceToolApproved,
}

pub(crate) enum MachinePoll {
    ProgramChanged,
    CooperativeYield,
    Cancelled(etas_host::execution::CancellationCause),
    Complete(Box<InterpValue>),
    Yield(PendingBoundary),
    Fault(ExecutionFault),
}

#[derive(Default)]
pub(crate) struct EvalMachine {
    pub(crate) root_wait: Option<crate::driver::scheduler::RootTaskWait>,
    pub(super) checkpoint_root: Option<super::snapshot::CheckpointRoot>,
    pub(super) program_scope: Option<ProgramScope>,
    pub(super) stack: Vec<EvalFrame>,
    pub(super) active_call_depth: std::rc::Rc<std::cell::Cell<u32>>,
    pub(super) input: Option<MachineInput>,
    /// Shared with the run's evaluator; mirrors residual scope and revision
    /// boundary frames after every stack mutation.
    pub(super) residual_mirror:
        std::rc::Rc<std::cell::RefCell<crate::eval::residual::ResidualMirror>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProgramScope {
    pub program: crate::value::CheckedExecutionProgram,
    pub handler_floor: usize,
}

impl ProgramScope {
    pub(crate) fn program(&self) -> &etas_frontend::CheckedProgram {
        &self.program.checked
    }
}

impl EvalMachine {
    pub(crate) fn has_input(&self) -> bool {
        self.input.is_some()
    }
    pub(crate) fn program_scope(&self) -> Option<ProgramScope> {
        self.program_scope.clone()
    }
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn for_entry(
        program: crate::value::CheckedExecutionProgram,
        args: Vec<InterpValue>,
    ) -> Self {
        Self {
            checkpoint_root: Some(super::snapshot::CheckpointRoot { program, args }),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn from_snapshot(
        snapshot: &MachineSnapshot,
        checked: &etas_frontend::CheckedProject,
        slots: std::sync::Arc<crate::plan::SlotLayoutTable>,
        dispatch: &crate::plan::IntrinsicDispatchTable,
        closures: &crate::plan::ClosureLayoutTable,
        current: &crate::api::HostExecutionContext,
        limits: &etas_host::StorageLimits,
    ) -> Result<Self, String> {
        Self::from_snapshot_with_resources(
            snapshot,
            checked,
            slots,
            dispatch,
            closures,
            current,
            super::snapshot::SnapshotRestoreResources {
                task: None,
                locked_model: None,
                limits,
                revisions: None,
                root: None,
                ordering: None,
                residual: None,
            },
        )
    }

    pub(crate) fn from_snapshot_with_resources(
        snapshot: &MachineSnapshot,
        checked: &etas_frontend::CheckedProject,
        slots: std::sync::Arc<crate::plan::SlotLayoutTable>,
        dispatch: &crate::plan::IntrinsicDispatchTable,
        closures: &crate::plan::ClosureLayoutTable,
        current: &crate::api::HostExecutionContext,
        resources: super::snapshot::SnapshotRestoreResources<'_>,
    ) -> Result<Self, String> {
        Self::from_snapshot_with_contexts(
            snapshot, checked, slots, dispatch, closures, current, resources,
        )
        .map(|restored| restored.machine)
    }

    pub(crate) fn from_snapshot_with_contexts(
        snapshot: &MachineSnapshot,
        checked: &etas_frontend::CheckedProject,
        slots: std::sync::Arc<crate::plan::SlotLayoutTable>,
        dispatch: &crate::plan::IntrinsicDispatchTable,
        closures: &crate::plan::ClosureLayoutTable,
        current: &crate::api::HostExecutionContext,
        resources: super::snapshot::SnapshotRestoreResources<'_>,
    ) -> Result<super::snapshot::RestoredMachineSnapshot, String> {
        let limits = resources.limits;
        super::snapshot::SnapshotValidator::new(checked, &slots, dispatch, closures, limits)
            .with_inherited_handlers(
                resources
                    .task
                    .as_ref()
                    .map_or(0, |task| task.inherited_handlers),
            )
            .with_execution_revision(
                resources
                    .root
                    .as_ref()
                    .and_then(|root| root.program.revision.as_deref()),
            )
            .with_revisions(resources.revisions.as_deref())
            .with_tasks(
                resources.task.as_ref().map(|task| task.scheduler),
                resources.task.as_ref().map(|task| task.identity),
            )
            .validate_machine(snapshot)?;
        let mut contexts = std::collections::HashMap::new();
        let mut machine = Self::new();
        let residual =
            restore_residual_state(&snapshot.residual, checked, resources.residual.as_ref())?;
        let mut root_context = super::snapshot::RestoreContext::for_program(
            checked.clone(),
            resources.revisions.clone(),
        );
        root_context.current_host = Some(current.clone());
        root_context.locked_model = resources.locked_model.clone();
        root_context.ordering = resources.ordering.clone();
        root_context.residual = residual.clone();
        root_context.execution_program = resources.root.as_ref().map(|root| root.program.clone());
        root_context.task_handles = resources.task.is_some();
        root_context.shared_frames = resources.task.as_ref().map(|task| task.frames.clone());
        contexts.insert(std::sync::Arc::as_ptr(checked) as usize, root_context);
        let suspensions = if snapshot.suspended_programs.is_empty() {
            None
        } else {
            let table = std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashMap::new()));
            super::suspension::checkpoint::restore_graph(
                snapshot,
                resources
                    .revisions
                    .as_ref()
                    .ok_or("suspended checkpoint has no prepared revision environment")?,
                &mut contexts,
                &table,
                &machine.active_call_depth,
                current,
                super::suspension::checkpoint::RestoreOptions {
                    locked_model: &resources.locked_model,
                    ordering: resources.ordering.as_ref(),
                    residual: &residual,
                },
            )?;
            Some(table)
        };
        let mut scope: Option<ProgramScope> = None;
        let mut stack = Vec::with_capacity(snapshot.frames.len());
        for frame in snapshot.frames.iter().cloned() {
            let parent_checked = scope
                .as_ref()
                .map_or(checked, |scope| &scope.program.checked);
            let key = std::sync::Arc::as_ptr(parent_checked) as usize;
            let context = contexts.entry(key).or_insert_with(|| {
                let mut context = super::snapshot::RestoreContext::for_program(
                    parent_checked.clone(),
                    resources.revisions.clone(),
                );
                context.suspensions = suspensions.clone();
                context.locked_model = resources.locked_model.clone();
                context.ordering = resources.ordering.clone();
                context.residual = residual.clone();
                context.task_handles = resources.task.is_some();
                context.shared_frames = resources.task.as_ref().map(|task| task.frames.clone());
                context
            });
            context.suspensions = suspensions.clone();
            context.execution_program = scope
                .as_ref()
                .map(|scope| scope.program.clone())
                .or_else(|| resources.root.as_ref().map(|root| root.program.clone()));
            if let MachineFrameSnapshot::ProgramReturn(frame) = frame {
                super::snapshot::program::resolve_destination(
                    &frame,
                    parent_checked,
                    resources.revisions.as_deref(),
                    limits,
                )?;
                let call = super::scoped_call::ProgramCall::restore_target(
                    context.restore_call_target(frame.call)?,
                    parent_checked,
                )?;
                let destination = call.destination()?;
                let parent_program = scope
                    .as_ref()
                    .map(|scope| scope.program.clone())
                    .or_else(|| resources.root.as_ref().map(|root| root.program.clone()))
                    .ok_or("active checkpoint has no checked root execution program")?;
                stack.push(EvalFrame::ProgramReturn(Box::new(
                    super::frame::ProgramReturnFrame {
                        site: Some(frame.call_site.restore(context)?),
                        parent: scope.clone(),
                        parent_agent_context: frame
                            .parent_agent_context
                            .as_ref()
                            .map(|request| {
                                super::snapshot::restore_agent_context(
                                    request,
                                    &parent_program,
                                    resources.revisions.as_deref(),
                                )
                            })
                            .transpose()?,
                        parent_program,
                        parent_model_policy: super::snapshot::program::restore_model_policy(
                            frame.parent_model_policy,
                        ),
                        call,
                        outer: frame.outer.restore_with(context)?,
                        span: frame.span,
                        retry_depth: frame.retry_depth,
                    },
                )));
                scope = Some(ProgramScope {
                    program: destination,
                    handler_floor: frame.handler_floor,
                });
                continue;
            }
            stack.push(frame.restore_local(current, context)?);
        }
        machine.program_scope = scope;
        let active_program = machine
            .program_scope
            .as_ref()
            .map(|scope| scope.program.clone())
            .or_else(|| resources.root.as_ref().map(|root| root.program.clone()));
        let active_checked = active_program
            .as_ref()
            .map_or(checked, |program| &program.checked);
        let context = contexts
            .entry(std::sync::Arc::as_ptr(active_checked) as usize)
            .or_insert_with(|| {
                super::snapshot::RestoreContext::for_program(
                    active_checked.clone(),
                    resources.revisions.clone(),
                )
            });
        context.suspensions = suspensions;
        context.locked_model = resources.locked_model;
        context.ordering = resources.ordering;
        context.residual = residual;
        context.current_host = Some(current.clone());
        context.execution_program = active_program.clone();
        context.task_handles = resources.task.is_some();
        context.shared_frames = resources.task.as_ref().map(|task| task.frames.clone());
        machine.restore_input(
            snapshot.input.as_deref().cloned(),
            context,
            active_program.as_ref(),
        )?;
        machine.checkpoint_root = resources.root;
        machine.restore_frames(stack);
        Ok(super::snapshot::RestoredMachineSnapshot { machine, contexts })
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Result<MachineSnapshot, String> {
        self.capture_snapshot(&[], (0, 0, 0))
            .map(|snapshot| snapshot.machine)
    }

    #[cfg(test)]
    pub(crate) fn capture_snapshot(
        &self,
        args: &[InterpValue],
        residual_counters: (u64, u64, u64),
    ) -> Result<super::snapshot::CapturedMachineSnapshot, String> {
        let mut context = super::snapshot::CaptureContext::default();
        let args = self
            .checkpoint_root
            .as_ref()
            .map_or(args, |root| &root.args)
            .iter()
            .map(|argument| context.value(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let machine = self.capture_machine(&mut context, residual_counters)?;
        Ok(super::snapshot::CapturedMachineSnapshot {
            root: self.checkpoint_root.clone(),
            args,
            machine,
            revisions: context.revisions,
        })
    }

    pub(crate) fn checkpoint_root(&self) -> Option<&super::snapshot::CheckpointRoot> {
        self.checkpoint_root.as_ref()
    }

    pub(crate) fn pending_cpu_call(&self) -> Option<&crate::control::PendingCall> {
        match self.input.as_ref()? {
            MachineInput::Signal(crate::control::ControlSignal::Call(call))
                if self.stack.is_empty()
                    && matches!(
                        call.continuation,
                        crate::control::Continuation::Return
                            | crate::control::Continuation::BlockValue
                    ) =>
            {
                Some(call)
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn capture_machine(
        &self,
        context: &mut super::snapshot::CaptureContext,
        residual_counters: (u64, u64, u64),
    ) -> Result<MachineSnapshot, String> {
        self.capture_machine_in_program(context, residual_counters, None)
    }

    pub(crate) fn capture_machine_in_program(
        &self,
        context: &mut super::snapshot::CaptureContext,
        residual_counters: (u64, u64, u64),
        program: Option<&crate::value::CheckedExecutionProgram>,
    ) -> Result<MachineSnapshot, String> {
        let input = self.capture_input(
            context,
            self.checkpoint_root
                .as_ref()
                .map(|root| &root.program)
                .or_else(|| self.program_scope.as_ref().map(|scope| &scope.program))
                .or(program),
        )?;
        let scopes = super::snapshot::program::capture_scopes(self)?;
        let mut frames = Vec::with_capacity(self.stack.len());
        for (index, frame) in self.stack.iter().enumerate() {
            frames.push(match frame {
                EvalFrame::ProgramReturn(frame) => MachineFrameSnapshot::ProgramReturn(Box::new(
                    super::snapshot::program::capture(
                        frame,
                        scopes
                            .get(&index)
                            .ok_or("program boundary has no captured scope")?,
                        context,
                    )?,
                )),
                _ => MachineFrameSnapshot::capture(frame, context)?,
            });
        }
        let suspended_programs = context.capture_suspensions()?;
        let mut machine = MachineSnapshot::capture(frames);
        machine.input = input.map(Box::new);
        machine.suspended_programs = suspended_programs;
        machine.residual = context.capture_residual(
            self.checkpoint_root
                .as_ref()
                .map(|root| &root.program.checked)
                .or(self
                    .program_scope
                    .as_ref()
                    .map(|scope| &scope.program.checked))
                .or(program.map(|program| &program.checked)),
            residual_counters,
        )?;
        Ok(machine)
    }

    pub(super) fn push_frame(&mut self, frame: EvalFrame) {
        if matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)) {
            self.active_call_depth.set(
                self.active_call_depth
                    .get()
                    .checked_add(1)
                    .expect("machine call depth cannot exceed u32::MAX"),
            );
        }
        if let Some(entry) = residual_mirror_entry(&frame) {
            self.residual_mirror.borrow_mut().push(entry);
        }
        self.stack.push(frame);
    }

    pub(super) fn pop_frame(&mut self) -> Option<EvalFrame> {
        let frame = self.stack.pop()?;
        if matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)) {
            self.active_call_depth.set(
                self.active_call_depth
                    .get()
                    .checked_sub(1)
                    .expect("machine call-depth counter must match the frame stack"),
            );
        }
        if matches!(
            frame,
            EvalFrame::ResidualScope(_) | EvalFrame::ProgramReturn(_)
        ) {
            self.residual_mirror.borrow_mut().pop();
        }
        Some(frame)
    }

    /// Rebuilds the residual mirror from the explicit stack after a bulk
    /// stack mutation or when the machine joins a run's shared mirror.
    pub(super) fn resync_residual_mirror(&mut self) {
        let mut mirror = self.residual_mirror.borrow_mut();
        mirror.clear();
        for frame in &self.stack {
            if let Some(entry) = residual_mirror_entry(frame) {
                mirror.push(entry);
            }
        }
    }

    pub(super) fn join_residual_mirror(
        &mut self,
        mirror: &std::rc::Rc<std::cell::RefCell<crate::eval::residual::ResidualMirror>>,
    ) {
        if !std::rc::Rc::ptr_eq(&self.residual_mirror, mirror) {
            self.residual_mirror = mirror.clone();
            self.resync_residual_mirror();
        }
    }

    pub(super) fn truncate_to(&mut self, len: usize) {
        assert!(len <= self.stack.len(), "cannot extend stack by truncation");
        let removed_calls = self.stack[len..]
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_)))
            .count() as u32;
        self.active_call_depth.set(
            self.active_call_depth
                .get()
                .checked_sub(removed_calls)
                .expect("machine call-depth counter must match the frame stack"),
        );
        self.stack.truncate(len);
        self.resync_residual_mirror();
    }

    pub(super) fn restore_frames(&mut self, frames: Vec<EvalFrame>) {
        self.truncate_to(0);
        self.active_call_depth.set(
            self.active_call_depth
                .get()
                .checked_add(
                    frames
                        .iter()
                        .filter(|frame| {
                            matches!(frame, EvalFrame::Call(_) | EvalFrame::ProgramReturn(_))
                        })
                        .count() as u32,
                )
                .expect("restored call frames cannot exceed u32::MAX"),
        );
        self.stack = frames;
        self.resync_residual_mirror();
    }

    pub(super) fn frames(&self) -> &[EvalFrame] {
        &self.stack
    }

    pub(super) fn active_call_depth(&self) -> u32 {
        self.active_call_depth.get()
    }
}

fn residual_mirror_entry(frame: &EvalFrame) -> Option<crate::eval::residual::ResidualMirrorEntry> {
    use crate::eval::residual::{ResidualBoundary, ResidualMirrorEntry};
    match frame {
        EvalFrame::ResidualScope(scope) => Some(ResidualMirrorEntry::Scope(scope.clone())),
        EvalFrame::ProgramReturn(boundary) => Some(ResidualMirrorEntry::Boundary(
            boundary
                .call
                .action_projections()
                .map(|projections| ResidualBoundary {
                    projections,
                    parent: boundary.parent_program.clone(),
                }),
        )),
        _ => None,
    }
}

/// Restores every checkpointed residual scope and attempt through the freshly
/// prepared plans of `checked`, before any frame that references them.
fn restore_residual_state(
    snapshot: &crate::orchestration::ResidualMachineSnapshot,
    checked: &etas_frontend::CheckedProject,
    input: Option<&super::snapshot::ResidualRestoreInput>,
) -> Result<Option<std::rc::Rc<super::snapshot::RestoredResidualState>>, String> {
    use crate::control::residual::{ResidualAttemptTag, ResidualScopeInstance};
    if snapshot.is_empty() {
        return Ok(None);
    }
    let input = input.ok_or("checkpoint carries residual monitor state but no prepared plans")?;
    input
        .log
        .restore_history(&snapshot.exits, &input.prepared, checked)?;
    let mut scopes = Vec::with_capacity(snapshot.scopes.len());
    let mut highest_cursor = 0u64;
    let mut invocations = std::collections::BTreeSet::new();
    for scope in &snapshot.scopes {
        input
            .log
            .check_activation_capacity(input.limits.max_active_scopes)?;
        let prepared = input
            .prepared
            .scope(etas_hir::HirItemId(scope.item))
            .ok_or("checkpoint residual scope has no installed plan in the recompiled program")?
            .clone();
        if !invocations.insert(scope.invocation) {
            return Err("checkpoint residual scopes share an invocation identity".into());
        }
        let instance = ResidualScopeInstance::restore(
            scope,
            prepared,
            checked.clone(),
            &input.log,
            input.limits,
        )?;
        highest_cursor = highest_cursor.max(instance.cursor_floor());
        let scope = std::rc::Rc::new(std::cell::RefCell::new(instance));
        input.log.register(&scope, input.limits.max_active_scopes)?;
        scopes.push(scope);
    }
    let mut attempts = Vec::with_capacity(snapshot.attempts.len());
    let mut attempt_ids = std::collections::BTreeSet::new();
    for attempt in &snapshot.attempts {
        if !attempt_ids.insert(attempt.id) {
            return Err("checkpoint residual attempts share an identity".into());
        }
        highest_cursor = highest_cursor.max(attempt.cursor);
        attempts.push(ResidualAttemptTag::restore(attempt, &scopes)?);
    }
    if snapshot.next_cursor < highest_cursor {
        return Err("checkpoint residual cursor would reuse a recorded event".into());
    }
    Ok(Some(std::rc::Rc::new(
        super::snapshot::RestoredResidualState { scopes, attempts },
    )))
}
