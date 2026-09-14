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
    Signal(ControlSignal),
    ModelResult(Result<ModelResponse, HostError>),
    ToolResult(Result<ToolResponse, HostError>),
    SourceToolApproved,
}

pub(crate) enum MachinePoll {
    CooperativeYield,
    Cancelled(etas_host::execution::CancellationCause),
    Complete(Box<InterpValue>),
    Yield(PendingBoundary),
    Fault(ExecutionFault),
}

#[derive(Default)]
pub(crate) struct EvalMachine {
    stack: Vec<EvalFrame>,
    active_call_depth: u32,
    pub(super) input: Option<MachineInput>,
}

impl EvalMachine {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_snapshot(
        snapshot: &MachineSnapshot,
        checked: &etas_frontend::CheckedProject,
        slots: std::sync::Arc<crate::plan::SlotLayoutTable>,
        dispatch: &crate::plan::IntrinsicDispatchTable,
        closures: &crate::plan::ClosureLayoutTable,
        current: &crate::api::HostExecutionContext,
        limits: &etas_host::StorageLimits,
    ) -> Result<Self, String> {
        super::snapshot::SnapshotValidator::new(checked, &slots, dispatch, closures, limits)
            .validate_machine(snapshot)?;
        let mut context = super::snapshot::RestoreContext::default();
        let stack = snapshot
            .frames
            .iter()
            .cloned()
            .map(|frame| -> Result<_, String> {
                Ok(match frame {
                    MachineFrameSnapshot::Block { continuation } => {
                        EvalFrame::Block(super::frame::BlockFrame {
                            continuation: continuation.restore_with(&mut context)?,
                        })
                    }
                    MachineFrameSnapshot::Expr { continuation } => {
                        EvalFrame::Expr(super::frame::ExprFrame {
                            continuation: continuation.restore_with(&mut context)?,
                        })
                    }
                    MachineFrameSnapshot::Call { continuation, span } => {
                        EvalFrame::Call(super::frame::CallFrame {
                            continuation: continuation.restore_with(&mut context)?,
                            span,
                        })
                    }
                    MachineFrameSnapshot::Continuation { continuation } => {
                        EvalFrame::Continuation(super::frame::ContinuationFrame {
                            continuation: continuation.restore_with(&mut context)?,
                        })
                    }
                    MachineFrameSnapshot::Handler { continuation } => {
                        let frame =
                            EvalFrame::from_continuation(continuation.restore_with(&mut context)?);
                        if !matches!(frame, EvalFrame::Handler(_)) {
                            return Err(
                                "checkpoint handler frame does not contain a handler boundary"
                                    .to_owned(),
                            );
                        }
                        frame
                    }
                    MachineFrameSnapshot::Retry { continuation } => {
                        let frame =
                            EvalFrame::from_continuation(continuation.restore_with(&mut context)?);
                        if !matches!(frame, EvalFrame::Retry(_)) {
                            return Err("checkpoint retry frame does not contain a retry attempt"
                                .to_owned());
                        }
                        frame
                    }
                    MachineFrameSnapshot::ModelLoop(frame) => {
                        EvalFrame::ModelLoop(Box::new(frame.restore(current, &mut context)?))
                    }
                    MachineFrameSnapshot::SourceToolReturn(frame) => {
                        EvalFrame::SourceToolReturn(frame.restore(current, &mut context)?)
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut machine = Self::new();
        machine.restore_frames(stack);
        Ok(machine)
    }

    pub(crate) fn snapshot(&self) -> Result<MachineSnapshot, String> {
        let mut frames = Vec::with_capacity(self.stack.len());
        for frame in &self.stack {
            frames.push(MachineFrameSnapshot::capture(frame)?);
        }
        Ok(MachineSnapshot { frames })
    }

    pub(super) fn push_frame(&mut self, frame: EvalFrame) {
        if matches!(frame, EvalFrame::Call(_)) {
            self.active_call_depth = self
                .active_call_depth
                .checked_add(1)
                .expect("machine call depth cannot exceed u32::MAX");
        }
        self.stack.push(frame);
    }

    pub(super) fn pop_frame(&mut self) -> Option<EvalFrame> {
        let frame = self.stack.pop()?;
        if matches!(frame, EvalFrame::Call(_)) {
            self.active_call_depth = self
                .active_call_depth
                .checked_sub(1)
                .expect("machine call-depth counter must match the frame stack");
        }
        Some(frame)
    }

    pub(super) fn truncate_to(&mut self, len: usize) {
        assert!(len <= self.stack.len(), "cannot extend stack by truncation");
        let removed_calls = self.stack[len..]
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_)))
            .count() as u32;
        self.active_call_depth = self
            .active_call_depth
            .checked_sub(removed_calls)
            .expect("machine call-depth counter must match the frame stack");
        self.stack.truncate(len);
    }

    pub(super) fn restore_frames(&mut self, frames: Vec<EvalFrame>) {
        self.active_call_depth = frames
            .iter()
            .filter(|frame| matches!(frame, EvalFrame::Call(_)))
            .count() as u32;
        self.stack = frames;
    }

    pub(super) fn frames(&self) -> &[EvalFrame] {
        &self.stack
    }

    pub(super) fn active_call_depth(&self) -> u32 {
        self.active_call_depth
    }
}
