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
            .map(|input| self.capture_pending_input(input, context, owner))
            .transpose()
    }

    pub(crate) fn capture_pending_input(
        &self,
        input: &MachineInput,
        context: &mut CaptureContext,
        owner: Option<&CheckedExecutionProgram>,
    ) -> Result<MachineInputSnapshot, String> {
        Ok(match input {
            MachineInput::Signal(signal) => MachineInputSnapshot::Signal(Box::new(
                ControlSignalSnapshot::capture(signal, context)?,
            )),
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
                    owner: context.execution_identity(parent)?,
                    call: super::program::capture_call(call, context)?,
                    args: capture_values(args, context)?,
                    span: *span,
                }
            }
            MachineInput::ModelResult(result) => MachineInputSnapshot::ModelResult(result.clone()),
            MachineInput::ToolResult(result) => MachineInputSnapshot::ToolResult(result.clone()),
            MachineInput::SourceToolApproved => MachineInputSnapshot::SourceToolApproved,
        })
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
                        let boundary = self.stack.iter().rev().find_map(|frame| match frame {
                            crate::eval::machine::frame::EvalFrame::ProgramReturn(boundary) => {
                                Some((boundary.parent_program.clone(), boundary.call.clone()))
                            }
                            _ => None,
                        });
                        let parent = if let Some((parent, _)) = &boundary {
                            let saved = CheckedExecutionProgram::restore_identity(
                                &owner,
                                parent,
                                context.revisions.as_deref(),
                            )?;
                            if !std::sync::Arc::ptr_eq(&saved.checked, &parent.checked)
                                || saved.plan.root != parent.plan.root
                            {
                                return Err(
                                    "pending program entry has a different checked parent".into()
                                );
                            }
                            parent.clone()
                        } else if let Some(revisions) = &context.revisions {
                            revisions.execution_snapshot(&owner)?
                        } else if owner == program.checkpoint_identity(&mut Default::default()) {
                            program.clone()
                        } else {
                            return Err(
                                "pending program entry has no prepared program environment".into(),
                            );
                        };
                        let previous = context.execution_program.clone();
                        context.select_program(parent.checked.clone());
                        context.execution_program = Some(parent.clone());
                        let call = if let Some((_, call)) = boundary {
                            call
                        } else {
                            super::program::restore_call(call, context, &parent)?
                        };
                        // enter_program has already projected these values into
                        // the destination before it exposes this machine input.
                        let destination = call.destination()?;
                        context.select_program(destination.checked.clone());
                        context.execution_program = Some(destination);
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
            ControlSignal::ProgramExpr(pending) => Self::ProgramExpr {
                call: Box::new(super::program::capture_expression(&pending.call, context)?),
                args: capture_values(&pending.args, context)?,
                span: pending.span,
                continuation: context.continuation(&pending.continuation)?,
            },
            ControlSignal::Block(pending) => Self::Block {
                agent_context: pending.agent_context.as_ref().map(|request| {
                    Box::new(super::capture_agent_context(
                        request,
                        &mut context.revisions,
                    ))
                }),
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
                PendingTaskControl::AgentReply { .. }
                | PendingTaskControl::ManagedAgent(_)
                | PendingTaskControl::ManagedAuthorization(_)
                | PendingTaskControl::NativeAuthorization(_)
                | PendingTaskControl::AuthorizedNative(_)
                | PendingTaskControl::RestoreHandlerWait { .. } => {
                    return Err(
                        "checkpoint capture of live request authorization is not supported".into(),
                    );
                }
                PendingTaskControl::NextCompletion {
                    scope,
                    result_type,
                    span,
                } => TaskControlSnapshot::NextCompletion {
                    scope: scope.0,
                    result_type: *result_type,
                    span: *span,
                },
                PendingTaskControl::ResumeHandled {
                    task,
                    handler,
                    value,
                    span,
                } => TaskControlSnapshot::ResumeHandled {
                    owner: task.owner.0,
                    task: task.task.0,
                    handler: *handler,
                    value: context.value(value)?,
                    span: *span,
                },
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
            ControlSignal::Perform(perform)
                if perform.expr.is_none()
                    && perform.default_boundary.is_none()
                    && perform.forwarded_call.is_none() =>
            {
                perform
                    .checked_action
                    .as_ref()
                    .ok_or("delegated Handler request has no checked invocation")?
                    .validate_origin()?;
                let evidence = match &perform.continuation {
                    Continuation::TaskResume(_) => None,
                    _ => Some(
                        perform
                            .checked_action
                            .as_ref()
                            .unwrap()
                            .checkpoint_evidence(context)?,
                    ),
                };
                Self::HandlerRequest {
                    evidence,
                    action: perform.action.clone(),
                    inference: perform
                        .inference
                        .as_deref()
                        .map(|request| {
                            super::capture_agent_context(request, &mut context.revisions)
                        })
                        .map(Box::new),
                    args: capture_values(&perform.args, context)?,
                    span: perform.span,
                    continuation: context.continuation(&perform.continuation)?,
                    residual: perform
                        .residual
                        .as_ref()
                        .map(|attempt| context.residual_attempt(attempt))
                        .transpose()?,
                }
            }
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
            Self::HandlerRequest {
                evidence,
                action,
                inference,
                args,
                span,
                continuation,
                residual,
            } => {
                let continuation = continuation.restore_with(context)?;
                let proof = match evidence {
                    Some(evidence) => CheckedActionInvocation::restore_evidence(
                        &evidence,
                        context
                            .revisions
                            .as_deref()
                            .ok_or("deferred Handler request has no checked revisions")?,
                    )?,
                    None => {
                        let Continuation::TaskResume(saved) = &continuation else {
                            return Err("delegated Handler request has no caller segment".into());
                        };
                        saved.projected_action()?
                    }
                };
                let owner = program.ok_or("delegated Handler request has no checked owner")?;
                if !std::sync::Arc::ptr_eq(&proof.program, &owner.checked) {
                    return Err("delegated Handler request differs from its checked owner".into());
                }
                ControlSignal::pending_perform(PendingPerform {
                    checked_action: Some(proof),
                    default_boundary: None,
                    forwarded_call: None,
                    expr: None,
                    action,
                    error_type: None,
                    inference: inference
                        .as_deref()
                        .map(|request| context.agent_context(request))
                        .transpose()?,
                    args: restore_values(args, context)?,
                    span,
                    continuation,
                    residual: residual
                        .map(|attempt| context.residual_attempt(attempt))
                        .transpose()?,
                })
            }
            Self::Input(input) => match input.restore(context)? {
                ContinuationInput::Value(value) => ControlSignal::Value(value),
                ContinuationInput::Return(value) => ControlSignal::Return(value),
                ContinuationInput::Resume(value) => ControlSignal::Resume(value),
                ContinuationInput::Finish(value) => ControlSignal::Finish(value),
                ContinuationInput::Break => ControlSignal::Break,
                ContinuationInput::Continue => ControlSignal::Continue,
                input @ ContinuationInput::RetryFailure { .. } => {
                    ControlSignal::pending_continuation(PendingContinuation {
                        continuation: Continuation::BlockValue,
                        input,
                    })
                }
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
            Self::ProgramExpr {
                call,
                args,
                span,
                continuation,
            } => {
                let parent = program.ok_or("pending expression program has no checked caller")?;
                ControlSignal::ProgramExpr(Box::new(PendingProgramExpr {
                    call: super::program::restore_expression(*call, context, parent)?,
                    args: restore_values(args, context)?,
                    span,
                    continuation: continuation.restore_with(context)?,
                }))
            }
            Self::Block {
                agent_context,
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
                    agent_context: agent_context
                        .as_ref()
                        .map(|request| context.agent_context(request))
                        .transpose()?,
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
                    TaskControlSnapshot::NextCompletion {
                        scope,
                        result_type,
                        span,
                    } => PendingTaskControl::NextCompletion {
                        scope: TaskScopeId(scope),
                        result_type,
                        span,
                    },
                    TaskControlSnapshot::ResumeHandled {
                        owner,
                        task,
                        handler,
                        value,
                        span,
                    } => PendingTaskControl::ResumeHandled {
                        task: TaskHandle {
                            owner: TaskScopeId(owner),
                            task: TaskId(task),
                        },
                        handler,
                        value: value.restore_with(context)?,
                        span,
                    },
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
            ContinuationInput::RetryFailure { span, message } => Self::RetryFailure {
                span: *span,
                message: message.clone(),
            },
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
            Self::RetryFailure { span, message } => {
                ContinuationInput::RetryFailure { span, message }
            }
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
