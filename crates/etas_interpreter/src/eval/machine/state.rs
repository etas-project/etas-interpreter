use etas_core::Span;

use crate::{
    control::{
        ControlSignal, ExecutionFault, PendingCommand, PendingConsole, PendingHostBoundary,
        PendingMemory, PendingModel, PendingPerform, PendingSession,
    },
    value::InterpValue,
};
use etas_host::{HostError, HostValue, ModelResponse, PolicySubject, ToolRequest, ToolResponse};

use super::frame::EvalFrame;
use crate::orchestration::{MachineFrameSnapshot, MachineSnapshot};

pub(crate) enum PendingBoundary {
    Perform(Box<PendingPerform>),
    Memory(Box<PendingMemory>),
    Session(Box<PendingSession>),
    Console(Box<PendingConsole>),
    Command(Box<PendingCommand>),
    Model(Box<PendingModel>),
    Tool(Box<PendingTool>),
    Host(Box<PendingHostBoundary>),
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
    pub(super) checkpoint_root: Option<super::snapshot::CheckpointRoot>,
    pub(super) program_scope: Option<ProgramScope>,
    pub(super) stack: Vec<EvalFrame>,
    pub(super) active_call_depth: std::rc::Rc<std::cell::Cell<u32>>,
    pub(super) input: Option<MachineInput>,
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
    pub(crate) fn program_scope(&self) -> Option<ProgramScope> {
        self.program_scope.clone()
    }
    pub(crate) fn new() -> Self {
        Self::default()
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
                locked_model: None,
                limits,
                revisions: None,
                root: None,
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
        let limits = resources.limits;
        super::snapshot::SnapshotValidator::new(checked, &slots, dispatch, closures, limits)
            .with_revisions(resources.revisions.as_deref())
            .validate_machine(snapshot)?;
        let mut contexts = std::collections::HashMap::new();
        let mut machine = Self::new();
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
                &resources.locked_model,
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
                context
            });
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
        machine.checkpoint_root = resources.root;
        machine.restore_frames(stack);
        Ok(machine)
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Result<MachineSnapshot, String> {
        self.capture_snapshot(&[]).map(|snapshot| snapshot.machine)
    }

    pub(crate) fn capture_snapshot(
        &self,
        args: &[InterpValue],
    ) -> Result<super::snapshot::CapturedMachineSnapshot, String> {
        let mut context = super::snapshot::CaptureContext::default();
        for argument in self
            .checkpoint_root
            .as_ref()
            .map_or(args, |root| &root.args)
        {
            context.value(argument)?;
        }
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
                        &mut context,
                    )?,
                )),
                _ => MachineFrameSnapshot::capture(frame, &mut context)?,
            });
        }
        let suspended_programs = context.capture_suspensions()?;
        let mut machine = MachineSnapshot::capture(frames);
        machine.suspended_programs = suspended_programs;
        Ok(super::snapshot::CapturedMachineSnapshot {
            root: self.checkpoint_root.clone(),
            machine,
            revisions: context.revisions,
        })
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
        Some(frame)
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
    }

    pub(super) fn frames(&self) -> &[EvalFrame] {
        &self.stack
    }

    pub(super) fn active_call_depth(&self) -> u32 {
        self.active_call_depth.get()
    }
}
