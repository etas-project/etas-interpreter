use super::*;
use crate::{control::ExecutionFault, value::host_handle::open_source_handle};
use etas_host::{CommandOutput, WorkspacePathRef};
use etas_types::TypeId;

struct SourceCommandInput {
    argv: Vec<String>,
    env: Vec<(String, String)>,
    cwd: Option<WorkspacePathRef>,
    stdin: Option<Vec<u8>>,
}

impl<'a> EvalContext<'a> {
    pub(crate) fn source_command_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Command", action) = self.source_default_signature(perform)? else {
            return None;
        };
        let operation = match action {
            "run" => CommandCallable::Run,
            "start" => CommandCallable::Start,
            "reconnect" => CommandCallable::Reconnect,
            "read" => CommandCallable::Read,
            "write" => CommandCallable::Write,
            "close_stdin" => CommandCallable::CloseStdin,
            "stop" => CommandCallable::Stop,
            _ => return None,
        };
        let proof = perform.checked_action.as_ref()?;
        let span = perform.span;
        let continuation = perform.continuation.clone();
        let args = perform.args.as_slice();
        let parameters = proof.parameters.as_slice();
        let output = proof.output;
        let invalid =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, reason);
        let prepare = |context: &mut Self| -> Result<ControlSignal, ExecutionFault> {
            let (operation, decode) = match (operation, args, parameters) {
                (
                    CommandCallable::Run | CommandCallable::Start,
                    [command, sandbox],
                    [command_ty, sandbox_ty],
                ) => {
                    context.select_source_command_profile(sandbox, *sandbox_ty, span)?;
                    let command = context.source_command_input(command, *command_ty, span)?;
                    if operation == CommandCallable::Run {
                        return Ok(ControlSignal::pending_command(PendingCommand {
                            request: CommandRequest {
                                id: context.next_host_request_id(),
                                argv: command.argv,
                                env: command.env,
                                cwd: command.cwd,
                                stdin: command.stdin,
                                authority: context.host_authority(),
                                trace: context.host_trace(),
                                budget: context.host_budget(),
                            },
                            decode: CommandDecode::SourceResult {
                                result_type: output,
                            },
                            span,
                            continuation,
                        }));
                    }
                    if command.stdin.is_some() {
                        return Err(invalid(
                            "command start requires stdin to be written through its process handle",
                        ));
                    }
                    (
                        CommandProcessOperation::Start {
                            argv: command.argv,
                            env: command.env,
                            cwd: command.cwd,
                        },
                        HostBoundaryDecode::SourceCommandHandle {
                            result_type: output,
                        },
                    )
                }
                (
                    CommandCallable::Reconnect,
                    [InterpValue::String(process), sandbox],
                    [_, sandbox_ty],
                ) => {
                    context.select_source_command_profile(sandbox, *sandbox_ty, span)?;
                    if process.is_empty() {
                        return Err(invalid("command reconnect requires a process id"));
                    }
                    (
                        CommandProcessOperation::Reconnect {
                            process: process.to_string(),
                        },
                        HostBoundaryDecode::SourceCommandHandle {
                            result_type: output,
                        },
                    )
                }
                (CommandCallable::Read, [handle, maximum], [handle_ty, _]) => {
                    let process = context.source_command_process(handle, *handle_ty, span)?;
                    let max_bytes = context.usize_argument(maximum, "command read limit", span)?;
                    if max_bytes == 0 {
                        return Err(invalid("command read requires a positive byte limit"));
                    }
                    (
                        CommandProcessOperation::Read { process, max_bytes },
                        HostBoundaryDecode::CommandProcessEvent {
                            result_type: output,
                        },
                    )
                }
                (CommandCallable::Write, [handle, InterpValue::Bytes(body)], [handle_ty, _]) => (
                    CommandProcessOperation::Write {
                        process: context.source_command_process(handle, *handle_ty, span)?,
                        bytes: body.to_vec(),
                    },
                    HostBoundaryDecode::Unit,
                ),
                (CommandCallable::CloseStdin, [handle], [handle_ty]) => (
                    CommandProcessOperation::CloseStdin {
                        process: context.source_command_process(handle, *handle_ty, span)?,
                    },
                    HostBoundaryDecode::Unit,
                ),
                (CommandCallable::Stop, [handle], [handle_ty]) => (
                    CommandProcessOperation::Stop {
                        process: context.source_command_process(handle, *handle_ty, span)?,
                    },
                    HostBoundaryDecode::CommandProcessEvent {
                        result_type: output,
                    },
                ),
                _ => {
                    return Err(command_contract_fault(
                        span,
                        "command payload differs from its checked action",
                    ));
                }
            };
            Ok(context.pending_command_process(operation, decode, span, continuation))
        };
        Some(match prepare(self) {
            Ok(signal) => signal,
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        })
    }

    pub(super) fn pending_command_process(
        &mut self,
        operation: CommandProcessOperation,
        decode: HostBoundaryDecode,
        span: Span,
        continuation: Continuation,
    ) -> ControlSignal {
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::CommandProcess(CommandProcessRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode,
            span,
            continuation,
        })
    }

    fn select_source_command_profile(
        &self,
        value: &InterpValue,
        ty: TypeId,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let record = command_record(value, ty).ok_or_else(|| {
            command_contract_fault(span, "command requires its checked sandbox profile")
        })?;
        match record.get_ref("name") {
            Some(InterpValue::String(name)) if name.as_str() == "default" => Ok(()),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "the command host exposes only the default sandbox profile",
            )),
        }
    }

    fn source_command_process(
        &self,
        value: &InterpValue,
        ty: TypeId,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        let value = open_source_handle(self.checked, ty, "process", value)
            .map_err(|reason| command_contract_fault(span, reason))?;
        self.command_process_id_argument(value, span)
    }

    fn source_command_input(
        &self,
        value: &InterpValue,
        ty: TypeId,
        span: Span,
    ) -> Result<SourceCommandInput, ExecutionFault> {
        let invalid = |member: &str| {
            command_contract_fault(
                span,
                format!("command has no checked {member} representation"),
            )
        };
        let record = command_record(value, ty).ok_or_else(|| invalid("argument record"))?;
        let Some(InterpValue::String(program)) = record.get_ref("program") else {
            return Err(invalid("program"));
        };
        let Some(InterpValue::Array(args)) = record.get_ref("args") else {
            return Err(invalid("argument list"));
        };
        let environment = record
            .get_ref("environment")
            .ok_or_else(|| invalid("environment"))?;
        let HostValue::Map(environment) =
            super::host_value::checked_map_to_host(environment, self.checked, &self.storage_limits)
                .map_err(|reason| command_contract_fault(span, reason))?
        else {
            unreachable!("checked map projection returns a host map");
        };
        let argv = std::iter::once(program.to_string())
            .map(Ok)
            .chain(args.borrow().iter().map(|value| match value {
                InterpValue::String(arg) => Ok(arg.to_string()),
                _ => Err(invalid("text argument")),
            }))
            .collect::<Result<_, _>>()?;
        let env = environment
            .into_iter()
            .map(|(key, value)| match (key, value) {
                (HostValue::String(key), HostValue::String(value)) => Ok((key, value)),
                _ => Err(invalid("environment entry")),
            })
            .collect::<Result<_, _>>()?;
        let layout = etas_types::record_fields_with_applied_params(&self.checked.type_store, ty)
            .map_err(|error| command_contract_fault(span, error.to_string()))?
            .ok_or_else(|| invalid("argument layout"))?;
        let option = |name| {
            let field = layout
                .fields
                .iter()
                .find(|field| field.name == name)
                .ok_or_else(|| invalid(name))?;
            let value = record.get_ref(name).ok_or_else(|| invalid(name))?;
            self.checked_option_payload(value, field.ty)
                .ok_or_else(|| invalid(name))
        };
        let cwd = option("cwd")?
            .map(|value| {
                let InterpValue::Existential(path) = value else {
                    return Err(invalid("directory package"));
                };
                path.validate(self.checked)
                    .map_err(|reason| command_contract_fault(span, reason))?;
                self.source_workspace_path(&path.value, path.concrete_type, span)
            })
            .transpose()?;
        let stdin = option("stdin")?
            .map(|value| match value {
                InterpValue::Bytes(body) => Ok(body.to_vec()),
                _ => Err(invalid("stdin bytes")),
            })
            .transpose()?;
        Ok(SourceCommandInput {
            argv,
            env,
            cwd,
            stdin,
        })
    }

    pub(crate) fn command_result_value(
        &mut self,
        command: &PendingCommand,
        result: CommandOutput,
    ) -> Result<InterpValue, String> {
        match command.decode {
            CommandDecode::CommandResult => Ok(InterpValue::CommandResult {
                exit_code: result.exit_code,
                stdout: result.stdout.into(),
                stderr: result.stderr.into(),
            }),
            CommandDecode::SourceResult { result_type } => host_to_checked_interp_value(
                HostValue::Record(vec![
                    ("exit_code".into(), HostValue::Int(result.exit_code.into())),
                    ("stdout".into(), HostValue::Bytes(result.stdout)),
                    ("stderr".into(), HostValue::Bytes(result.stderr)),
                ]),
                result_type,
                self.checked,
                &self.storage_limits,
            ),
        }
    }

    pub(crate) fn command_boundary_key(
        &self,
        command: &PendingCommand,
    ) -> Result<etas_host::replay::ReplayFingerprint, crate::control::ExecutionFault> {
        self.replay_fingerprint(&command.request, command.request.id, command.span)
    }
}

fn command_record(value: &InterpValue, ty: TypeId) -> Option<&RecordValue> {
    let InterpValue::Nominal { ty: actual, value } = value else {
        return None;
    };
    match value.as_ref() {
        InterpValue::Record(record) if *actual == ty => Some(record),
        _ => None,
    }
}

fn command_contract_fault(span: Span, reason: impl Into<String>) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
}
