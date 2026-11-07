use super::{CaptureContext, RestoreContext};
use crate::eval::machine::{EvalMachine, state::MachineInput};
use crate::{control::*, orchestration::*, value::CheckedExecutionProgram};

impl EvalMachine {
    pub(crate) fn capture_input(
        &self,
        context: &mut CaptureContext,
        owner: Option<&CheckedExecutionProgram>,
    ) -> Result<Option<MachineInputSnapshot>, String> {
        self.input
            .as_ref()
            .map(|input| {
                Ok(match input {
                    MachineInput::Signal(signal) => MachineInputSnapshot::Signal(
                        ControlSignalSnapshot::capture(signal, context)?,
                    ),
                    MachineInput::EnterProgram { call, args, span } => {
                        let parent = self
                            .stack
                            .iter()
                            .rev()
                            .find_map(|frame| match frame {
                                crate::eval::machine::frame::EvalFrame::ProgramReturn(boundary) => {
                                    Some(&boundary.parent_program)
                                }
                                _ => None,
                            })
                            .or(owner)
                            .ok_or("pending program entry has no checked owner")?;
                        MachineInputSnapshot::EnterProgram {
                            owner: parent.checkpoint_identity(&mut context.revisions),
                            call: context.call_target(&call.call_target())?,
                            args: capture_values(args, context)?,
                            span: *span,
                        }
                    }
                    MachineInput::ModelResult(result) => {
                        MachineInputSnapshot::ModelResult(result.clone())
                    }
                    MachineInput::ToolResult(result) => {
                        MachineInputSnapshot::ToolResult(result.clone())
                    }
                    MachineInput::SourceToolApproved => MachineInputSnapshot::SourceToolApproved,
                })
            })
            .transpose()
    }

    pub(crate) fn restore_input(
        &mut self,
        input: Option<MachineInputSnapshot>,
        context: &mut RestoreContext,
        program: Option<&CheckedExecutionProgram>,
    ) -> Result<(), String> {
        self.input = input
            .map(|input| {
                Ok::<_, String>(match input {
                    MachineInputSnapshot::Signal(signal) => {
                        MachineInput::Signal(signal.restore(context, program)?)
                    }
                    MachineInputSnapshot::EnterProgram {
                        owner,
                        call,
                        args,
                        span,
                    } => {
                        let program =
                            program.ok_or("pending program entry has no checked restore owner")?;
                        let parent =
                            if owner == program.checkpoint_identity(&mut Default::default()) {
                                program.clone()
                            } else {
                                context
                                    .revisions
                                    .as_ref()
                                    .ok_or("pending program entry has no prepared revision set")?
                                    .execution_snapshot(&owner)?
                            };
                        let previous = context.execution_program.clone();
                        context.select_program(parent.checked.clone());
                        context.execution_program = Some(parent.clone());
                        let call = super::super::scoped_call::ProgramCall::restore_target(
                            context.restore_call_target(call)?,
                            &parent.checked,
                        )?;
                        let args = restore_values(args, context)?;
                        context.select_program(program.checked.clone());
                        context.execution_program = previous;
                        MachineInput::EnterProgram { call, args, span }
                    }
                    MachineInputSnapshot::ModelResult(result) => MachineInput::ModelResult(result),
                    MachineInputSnapshot::ToolResult(result) => MachineInput::ToolResult(result),
                    MachineInputSnapshot::SourceToolApproved => MachineInput::SourceToolApproved,
                })
            })
            .transpose()?;
        Ok(())
    }
}

impl ControlSignalSnapshot {
    fn capture(signal: &ControlSignal, context: &mut CaptureContext) -> Result<Self, String> {
        Ok(match signal {
            ControlSignal::Value(value) => {
                Self::Input(ContinuationInputSnapshot::Value(context.value(value)?))
            }
            ControlSignal::Return(value) => {
                Self::Input(ContinuationInputSnapshot::Return(context.value(value)?))
            }
            ControlSignal::Resume(value) => {
                Self::Input(ContinuationInputSnapshot::Resume(context.value(value)?))
            }
            ControlSignal::Finish(value) => {
                Self::Input(ContinuationInputSnapshot::Finish(context.value(value)?))
            }
            ControlSignal::Break => Self::Input(ContinuationInputSnapshot::Break),
            ControlSignal::Continue => Self::Input(ContinuationInputSnapshot::Continue),
            ControlSignal::Apply(pending) => Self::Apply {
                continuation: context.continuation(&pending.continuation)?,
                input: ContinuationInputSnapshot::capture(&pending.input, context)?,
            },
            ControlSignal::Checkpoint(pending) => Self::Checkpoint {
                label: pending.label.clone(),
                continuation: context.continuation(&pending.continuation)?,
            },
            ControlSignal::Expr(pending) => Self::Expr {
                expr: pending.expr,
                frame: context.frame(&pending.frame)?,
                continuation: context.continuation(&pending.continuation)?,
            },
            ControlSignal::Block(pending) => Self::Block {
                block: pending.block,
                next_stmt_index: pending.next_stmt_index,
                frame: context.frame(&pending.frame)?,
                continuation: context.continuation(&pending.continuation)?,
                residual: pending
                    .residual
                    .as_ref()
                    .map(|activation| {
                        Ok::<_, String>(ResidualActivationSnapshot {
                            item: activation.scope.item,
                            application: activation
                                .scope
                                .applications
                                .iter()
                                .position(|application| {
                                    std::sync::Arc::ptr_eq(application, &activation.application)
                                })
                                .ok_or("pending residual activation has no checked application")?,
                        })
                    })
                    .transpose()?,
            },
            ControlSignal::Call(pending) => Self::Call {
                pure: pending.purity == CallPurity::Pure,
                site: pending
                    .site
                    .as_ref()
                    .map(|site| CallSiteSnapshot::capture(site, context))
                    .transpose()?,
                target: context.call_target(&pending.target)?,
                args: capture_values(&pending.args, context)?,
                span: pending.span,
                continuation: context.continuation(&pending.continuation)?,
            },
            ControlSignal::Task(pending) => Self::Task(match &**pending {
                PendingTaskControl::Run { scope, span } => TaskControlSnapshot::Run {
                    scope: scope.0,
                    span: *span,
                },
                PendingTaskControl::Park { task, token, span } => TaskControlSnapshot::Park {
                    owner: task.owner.0,
                    task: task.task.0,
                    token: *token,
                    span: *span,
                },
                PendingTaskControl::Requeue { token, value, span } => {
                    TaskControlSnapshot::Requeue {
                        token: *token,
                        value: context.value(value)?,
                        span: *span,
                    }
                }
            }),
            ControlSignal::Fault(fault) => Self::Fault((**fault).clone()),
            ControlSignal::Perform(perform) if perform.error_type.is_some() => Self::Error {
                error_type: perform
                    .error_type
                    .expect("typed Error has its checked payload type"),
                payload: context
                    .value(perform.args.first().ok_or("typed Error has no payload")?)?,
                span: perform.span,
                continuation: context.continuation(&perform.continuation)?,
                residual: perform
                    .residual
                    .as_ref()
                    .map(|attempt| context.residual_attempt(attempt))
                    .transpose()?,
            },
            _ => {
                return Err(
                    "pending native/Host work must reach a coordinator snapshot point".into(),
                );
            }
        })
    }

    fn restore(
        self,
        context: &mut RestoreContext,
        program: Option<&CheckedExecutionProgram>,
    ) -> Result<ControlSignal, String> {
        Ok(match self {
            Self::Error {
                error_type,
                payload,
                span,
                continuation,
                residual,
            } => {
                let mut perform = PendingPerform::typed_error(
                    error_type,
                    payload.restore_with(context)?,
                    span,
                    continuation.restore_with(context)?,
                );
                perform.residual = residual
                    .map(|attempt| context.residual_attempt(attempt))
                    .transpose()?;
                ControlSignal::pending_perform(perform)
            }
            Self::Input(input) => match input.restore(context)? {
                ContinuationInput::Value(value) => ControlSignal::Value(value),
                ContinuationInput::Return(value) => ControlSignal::Return(value),
                ContinuationInput::Resume(value) => ControlSignal::Resume(value),
                ContinuationInput::Finish(value) => ControlSignal::Finish(value),
                ContinuationInput::Break => ControlSignal::Break,
                ContinuationInput::Continue => ControlSignal::Continue,
            },
            Self::Apply {
                continuation,
                input,
            } => ControlSignal::pending_continuation(PendingContinuation {
                continuation: continuation.restore_with(context)?,
                input: input.restore(context)?,
            }),
            Self::Checkpoint {
                label,
                continuation,
            } => ControlSignal::pending_checkpoint(PendingCheckpoint {
                label,
                continuation: continuation.restore_with(context)?,
            }),
            Self::Expr {
                expr,
                frame,
                continuation,
            } => ControlSignal::pending_expr(PendingExpr {
                expr,
                frame: context.frame(frame)?,
                continuation: continuation.restore_with(context)?,
            }),
            Self::Block {
                block,
                next_stmt_index,
                frame,
                continuation,
                residual,
            } => {
                let residual = residual
                    .map(|saved| {
                        let program =
                            program.ok_or("pending residual activation has no checked owner")?;
                        let scope = program
                            .plan
                            .residual
                            .scope(saved.item)
                            .ok_or("pending activation has no rechecked residual plan")?
                            .clone();
                        let application = scope
                            .applications
                            .get(saved.application)
                            .ok_or("pending activation has no rechecked residual application")?
                            .clone();
                        Ok::<_, String>(Box::new(residual::ResidualScopeActivation {
                            scope,
                            application,
                        }))
                    })
                    .transpose()?;
                ControlSignal::pending_block(PendingBlock {
                    block,
                    next_stmt_index,
                    frame: context.frame(frame)?,
                    continuation: continuation.restore_with(context)?,
                    residual,
                })
            }
            Self::Call {
                pure,
                site,
                target,
                args,
                span,
                continuation,
            } => ControlSignal::pending_call(PendingCall {
                purity: if pure {
                    CallPurity::Pure
                } else {
                    CallPurity::Ordinary
                },
                site: site.map(|site| site.restore(context)).transpose()?,
                target: context.restore_call_target(target)?,
                args: restore_values(args, context)?,
                span,
                continuation: continuation.restore_with(context)?,
            }),
            Self::Task(pending) => {
                use crate::driver::scheduler::{TaskHandle, TaskId, TaskScopeId};
                ControlSignal::pending_task(match pending {
                    TaskControlSnapshot::Run { scope, span } => PendingTaskControl::Run {
                        scope: TaskScopeId(scope),
                        span,
                    },
                    TaskControlSnapshot::Park {
                        owner,
                        task,
                        token,
                        span,
                    } => PendingTaskControl::Park {
                        task: TaskHandle {
                            owner: TaskScopeId(owner),
                            task: TaskId(task),
                        },
                        token,
                        span,
                    },
                    TaskControlSnapshot::Requeue { token, value, span } => {
                        PendingTaskControl::Requeue {
                            token,
                            value: value.restore_with(context)?,
                            span,
                        }
                    }
                })
            }
            Self::Fault(fault) => ControlSignal::Fault(Box::new(fault)),
        })
    }
}

impl ContinuationInputSnapshot {
    fn capture(input: &ContinuationInput, context: &mut CaptureContext) -> Result<Self, String> {
        Ok(match input {
            ContinuationInput::Value(value) => Self::Value(context.value(value)?),
            ContinuationInput::Return(value) => Self::Return(context.value(value)?),
            ContinuationInput::Resume(value) => Self::Resume(context.value(value)?),
            ContinuationInput::Finish(value) => Self::Finish(context.value(value)?),
            ContinuationInput::Break => Self::Break,
            ContinuationInput::Continue => Self::Continue,
        })
    }
    fn restore(self, context: &mut RestoreContext) -> Result<ContinuationInput, String> {
        Ok(match self {
            Self::Value(value) => ContinuationInput::Value(value.restore_with(context)?),
            Self::Return(value) => ContinuationInput::Return(value.restore_with(context)?),
            Self::Resume(value) => ContinuationInput::Resume(value.restore_with(context)?),
            Self::Finish(value) => ContinuationInput::Finish(value.restore_with(context)?),
            Self::Break => ContinuationInput::Break,
            Self::Continue => ContinuationInput::Continue,
        })
    }
}

fn capture_values(
    values: &[crate::value::InterpValue],
    context: &mut CaptureContext,
) -> Result<Vec<ValueSnapshot>, String> {
    values.iter().map(|value| context.value(value)).collect()
}
fn restore_values(
    values: Vec<ValueSnapshot>,
    context: &mut RestoreContext,
) -> Result<Vec<crate::value::InterpValue>, String> {
    values
        .into_iter()
        .map(|value| value.restore_with(context))
        .collect()
}
