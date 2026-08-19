use std::path::Path;

use etas_builtin::BuiltinError;
use etas_effects::{
    ActionRef, COMMAND_RUN_ACTION, COMMAND_TAG, CONSOLE_STDERR_WRITE_ACTION,
    CONSOLE_STDIN_READ_ALL_ACTION, CONSOLE_STDIN_READ_LINE_ACTION, CONSOLE_STDOUT_WRITE_ACTION,
    CONSOLE_TAG,
};
use etas_std::StdLimitKind;

use super::*;
use crate::control::ExecutionFault;

macro_rules! std_value {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        }
    };
}

impl<'a> EvalContext<'a> {
    pub(super) fn execute_std_callable(
        &mut self,
        kind: StdCallable,
        checked_call: &crate::intrinsic::dispatch::CheckedStdIntrinsicCall,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match kind {
            StdCallable::Approval => self.execute_approval_callable(call_args, span),
            StdCallable::Checkpoint => self.execute_checkpoint_callable(call_args, span),
            StdCallable::MemoryRegion => {
                self.execute_memory_region_callable(checked_call.result_type, call_args, span)
            }
            StdCallable::MemoryStore(callable) => {
                self.execute_memory_store_callable(callable, call_args, span)
            }
            StdCallable::MemoryVersionConstructor => {
                self.execute_memory_version_constructor(call_args, span)
            }
            StdCallable::MoneyUsdConstructor => {
                self.execute_money_usd_constructor(checked_call.result_type, call_args, span)
            }
            StdCallable::StreamErrorHostConstructor => {
                self.execute_stream_error_host_constructor(call_args, span)
            }
            StdCallable::CurrentSession => self.execute_current_session(call_args, span),
            StdCallable::SessionPolicyConstructor(name) => {
                self.execute_session_policy_constructor(name, call_args, span)
            }
            StdCallable::RuntimeLimitConstructor(kind) => {
                self.execute_runtime_limit_constructor(kind, call_args, span)
            }
            StdCallable::TrustWrapper(wrapper) => {
                self.execute_trust_wrapper_constructor(wrapper, call_args, span)
            }
            StdCallable::Console(callable) => {
                self.execute_console_callable(callable, call_args, span)
            }
            StdCallable::Command(callable) => {
                self.execute_command_callable(callable, call_args, span)
            }
            StdCallable::Filesystem(callable) => {
                self.execute_filesystem_callable(callable, call_args, span)
            }
            StdCallable::Tcp(callable) => self.execute_tcp_callable(callable, call_args, span),
            StdCallable::Stream(callable) => {
                self.execute_stream_callable(callable, call_args, span)
            }
            StdCallable::Tls(callable) => self.execute_tls_callable(callable, call_args, span),
            StdCallable::Secret(callable) => {
                self.execute_secret_callable(callable, call_args, span)
            }
            StdCallable::Json(callable) => self.execute_json_callable(callable, call_args, span),
            StdCallable::Browser(callable) => {
                self.execute_browser_callable(callable, call_args, span)
            }
        }
    }

    fn execute_approval_callable(
        &mut self,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if call_args.len() != 3 {
            return ControlSignal::invalid_arguments(
                format!(
                    "std.runtime.approval.approve expects exactly three arguments, got {}",
                    call_args.len()
                ),
                span,
            );
        }
        let path = etas_hir::unresolved_path_from_segments(&["Approval"], span);
        ControlSignal::pending_perform(PendingPerform {
            expr: None,
            action: ResolvedActionRef {
                effect: HirEffectRef {
                    path,
                    args: Vec::new(),
                    span,
                },
                action: "request".to_owned(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            error_type: None,
            args: call_args,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_checkpoint_callable(
        &mut self,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [state]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.runtime.checkpoint expects exactly one argument, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let label = match state {
            InterpValue::String(label) => Some(label),
            _ => None,
        };
        ControlSignal::pending_checkpoint(PendingCheckpoint {
            label,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_memory_region_callable(
        &mut self,
        result_type: etas_types::TypeId,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [stable_id, name]: [InterpValue; 2] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.memory.region expects exactly two arguments, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        let InterpValue::String(stable_id) = stable_id else {
            return ControlSignal::invalid_arguments(
                "std.memory.region stable_id must be a string",
                span,
            );
        };
        let InterpValue::String(name) = name else {
            return ControlSignal::invalid_arguments(
                "std.memory.region store name must be a string",
                span,
            );
        };
        ControlSignal::Value(InterpValue::ResourceHandle {
            name,
            stable_id,
            ty: result_type,
        })
    }

    fn execute_memory_store_callable(
        &mut self,
        callable: crate::intrinsic::dispatch::MemoryStoreCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let mut args = call_args.into_iter();
        let Some(InterpValue::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        }) = args.next()
        else {
            return ControlSignal::invalid_arguments(
                format!(
                    "std.memory.{} expects a Store<K, V> as its first argument",
                    memory_store_callable_name(callable)
                ),
                span,
            );
        };
        self.finish_memory_store_method(MemoryStoreArgs {
            region_stable_id,
            path,
            key_type,
            value_type,
            method: memory_store_callable_name(callable).to_owned(),
            evaluated_args: args.collect(),
            span,
        })
    }

    fn execute_money_usd_constructor(
        &mut self,
        result_type: etas_types::TypeId,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [amount]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "std.runtime.budget.usd expects exactly one argument, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        if !matches!(amount, InterpValue::Number(_)) {
            return ControlSignal::invalid_arguments(
                "std.runtime.budget.usd amount must be numeric",
                span,
            );
        }
        ControlSignal::Value(InterpValue::Nominal {
            ty: result_type,
            value: Box::new(InterpValue::Record(RecordValue::new(vec![
                ("amount".to_owned(), amount),
                ("currency".to_owned(), InterpValue::String("USD".to_owned())),
            ]))),
        })
    }

    fn execute_runtime_limit_constructor(
        &mut self,
        kind: StdLimitKind,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let value = match call_args.as_slice() {
            [InterpValue::Number(value)] => *value,
            [_] => {
                let message = format!(
                    "{} expects one integer argument",
                    runtime_limit_constructor_name(kind)
                );
                return ControlSignal::invalid_arguments(message, span);
            }
            args => {
                let message = format!(
                    "{} expects exactly one integer argument, got {}",
                    runtime_limit_constructor_name(kind),
                    args.len()
                );
                return ControlSignal::invalid_arguments(message, span);
            }
        };
        if value.as_u128().is_none() && value.as_i128().is_none_or(|value| value < 0) {
            let message = format!(
                "{} expects a non-negative integer argument",
                runtime_limit_constructor_name(kind)
            );
            return ControlSignal::invalid_arguments(message, span);
        }
        ControlSignal::Value(InterpValue::Variant {
            name: runtime_limit_constructor_name(kind).to_owned(),
            fields: vec![InterpValue::Number(value)],
        })
    }

    fn execute_session_policy_constructor(
        &mut self,
        name: &'static str,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if call_args.len() != 1 {
            return ControlSignal::invalid_arguments(
                format!("{name} expects exactly one argument"),
                span,
            );
        }
        ControlSignal::Value(InterpValue::Variant {
            name: name.to_owned(),
            fields: call_args,
        })
    }

    fn execute_stream_error_host_constructor(
        &mut self,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [message]: [InterpValue; 1] = match call_args.try_into() {
            Ok(args) => args,
            Err(args) => {
                return ControlSignal::invalid_arguments(
                    format!(
                        "StreamError.Host expects exactly one argument, got {}",
                        args.len()
                    ),
                    span,
                );
            }
        };
        if !matches!(message, InterpValue::String(_)) {
            return ControlSignal::invalid_arguments(
                "StreamError.Host expects a string message".to_owned(),
                span,
            );
        }
        ControlSignal::Value(InterpValue::Variant {
            name: "Host".to_owned(),
            fields: vec![message],
        })
    }

    fn execute_current_session(
        &mut self,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if !call_args.is_empty() {
            return ControlSignal::invalid_arguments(
                "current_session expects no arguments".to_owned(),
                span,
            );
        }
        let Some(session) = self.current_session.clone() else {
            let message = "current_session requires an active runtime session".to_owned();
            return ControlSignal::runtime_fault(message, span);
        };
        ControlSignal::Value(InterpValue::String(session))
    }

    fn execute_memory_version_constructor(
        &mut self,
        mut call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if call_args.len() != 1 {
            return ControlSignal::invalid_arguments(
                "std.memory.version expects exactly one string argument".to_owned(),
                span,
            );
        }
        let InterpValue::String(token) = call_args.remove(0) else {
            return ControlSignal::invalid_arguments(
                "std.memory.version expects a string version token".to_owned(),
                span,
            );
        };
        let Some(version_type) = self.known_std_types.memory_version else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "std.memory.version requires checked std.memory.MemoryVersion type facts",
            );
        };
        ControlSignal::Value(InterpValue::Nominal {
            ty: version_type,
            value: Box::new(InterpValue::Record(
                vec![("opaque".to_owned(), InterpValue::String(token))].into(),
            )),
        })
    }

    fn execute_trust_wrapper_constructor(
        &mut self,
        wrapper: etas_types::TrustWrapper,
        mut call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        if call_args.len() != 1 {
            return ControlSignal::invalid_arguments(
                format!("{wrapper} expects exactly one argument"),
                span,
            );
        }
        ControlSignal::Value(InterpValue::Trust {
            wrapper,
            value: Box::new(call_args.remove(0)),
        })
    }

    fn execute_json_callable(
        &mut self,
        callable: JsonCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match callable {
            JsonCallable::InvalidJson => {
                let [message]: [InterpValue; 1] = match call_args.try_into() {
                    Ok(args) => args,
                    Err(args) => {
                        return self.invalid_arguments_abort(
                            span,
                            format!(
                                "JsonError.InvalidJson expects exactly one string argument, got {}",
                                args.len()
                            ),
                        );
                    }
                };
                if !matches!(message, InterpValue::String(_)) {
                    return self.invalid_arguments_abort(
                        span,
                        "JsonError.InvalidJson expects a string message",
                    );
                }
                ControlSignal::Value(InterpValue::Variant {
                    name: "InvalidJson".to_owned(),
                    fields: vec![message],
                })
            }
            JsonCallable::Parse => {
                let [text]: [InterpValue; 1] = match call_args.try_into() {
                    Ok(args) => args,
                    Err(args) => {
                        return self.invalid_arguments_abort(
                            span,
                            format!(
                                "std.json.parse expects exactly one string argument, got {}",
                                args.len()
                            ),
                        );
                    }
                };
                let InterpValue::String(text) = text else {
                    return self
                        .invalid_arguments_abort(span, "std.json.parse expects a string argument");
                };
                let json = match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        return ControlSignal::Value(json_error_result(format!(
                            "std.json.parse received invalid JSON: {error}"
                        )));
                    }
                };
                match super::host_value::host_json_support_value_from_serde(json) {
                    Ok(value) => ControlSignal::Value(json_ok_result(InterpValue::Json(value))),
                    Err(error) => ControlSignal::Value(json_error_result(format!(
                        "std.json.parse cannot represent JSON value: {error:?}"
                    ))),
                }
            }
            JsonCallable::Stringify => {
                let [value]: [InterpValue; 1] = match call_args.try_into() {
                    Ok(args) => args,
                    Err(args) => {
                        return self.invalid_arguments_abort(
                            span,
                            format!(
                                "std.json.stringify expects exactly one JsonValue argument, got {}",
                                args.len()
                            ),
                        );
                    }
                };
                let InterpValue::Json(value) = value else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.json.stringify expects a JsonValue argument",
                    );
                };
                let host_json = super::host_value::host_json_support_value_to_host(&value);
                match etas_host::host_value_to_json(&etas_host::HostValue::Json(host_json)) {
                    Ok(value) => {
                        ControlSignal::Value(json_ok_result(InterpValue::String(value.to_string())))
                    }
                    Err(error) => ControlSignal::Value(json_error_result(format!(
                        "std.json.stringify cannot encode JsonValue: {error:?}"
                    ))),
                }
            }
        }
    }

    fn execute_console_callable(
        &mut self,
        callable: ConsoleCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        let (operation, decode, action) = match callable {
            ConsoleCallable::ReadAll => {
                if !call_args.is_empty() {
                    return ControlSignal::fault(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "std.io.read_all expects no arguments",
                    );
                }
                (
                    ConsoleOperation::ReadAllStdin,
                    ConsoleDecode::String,
                    CONSOLE_STDIN_READ_ALL_ACTION,
                )
            }
            ConsoleCallable::ReadLine => {
                if !call_args.is_empty() {
                    return ControlSignal::fault(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        "std.io.read_line expects no arguments",
                    );
                }
                (
                    ConsoleOperation::ReadLineStdin,
                    ConsoleDecode::String,
                    CONSOLE_STDIN_READ_LINE_ACTION,
                )
            }
            ConsoleCallable::Print | ConsoleCallable::PrintLn | ConsoleCallable::EPrintLn => {
                let text = match call_args.as_slice() {
                    [InterpValue::String(text)] => text.clone(),
                    args => {
                        return ControlSignal::fault(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!(
                                "std.io callable expects exactly one string argument, got {}",
                                args.len()
                            ),
                        );
                    }
                };
                let operation = match callable {
                    ConsoleCallable::Print => ConsoleOperation::WriteStdout {
                        text,
                        newline: false,
                    },
                    ConsoleCallable::PrintLn => ConsoleOperation::WriteStdout {
                        text,
                        newline: true,
                    },
                    ConsoleCallable::EPrintLn => ConsoleOperation::WriteStderr {
                        text,
                        newline: true,
                    },
                    ConsoleCallable::ReadAll | ConsoleCallable::ReadLine => unreachable!(),
                };
                let action = match callable {
                    ConsoleCallable::Print | ConsoleCallable::PrintLn => {
                        CONSOLE_STDOUT_WRITE_ACTION
                    }
                    ConsoleCallable::EPrintLn => CONSOLE_STDERR_WRITE_ACTION,
                    ConsoleCallable::ReadAll | ConsoleCallable::ReadLine => unreachable!(),
                };
                (operation, ConsoleDecode::Unit, action)
            }
        };
        let action = ActionRef {
            tag: CONSOLE_TAG,
            action,
        };
        if !self.plan.action_mediation.requires_action(&action)
            || !self.plan.action_mediation.has_default_action(&action)
        {
            return ControlSignal::missing_checked_fact(
                "std.io console execution requires checked default action facts for Console",
                span,
            );
        }
        ControlSignal::pending_console(PendingConsole {
            request: ConsoleRequest {
                id: request_id,
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_command_callable(
        &mut self,
        callable: CommandCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match callable {
            CommandCallable::Run => self.execute_command_run(call_args, span),
        }
    }

    fn execute_command_run(&mut self, call_args: Vec<InterpValue>, span: Span) -> ControlSignal {
        let [command, sandbox] = call_args.as_slice() else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "std.host.command.run expects Command and SandboxProfile arguments",
            );
        };
        let InterpValue::Command {
            argv,
            env,
            cwd,
            stdin,
        } = command
        else {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "std.host.command.run first argument must be a Command support value",
            );
        };
        if !matches!(
            sandbox,
            InterpValue::Variant { name, fields }
                if name == "DefaultCommandSandbox" && fields.is_empty()
        ) {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "std.host.command.run requires the DefaultCommandSandbox support value",
            );
        }
        if argv.is_empty() {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "std.host.command.run requires a non-empty argv",
            );
        }
        if cwd.is_some() {
            return ControlSignal::fault(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "std.host.command.run Command.cwd requires workspace-root materialization before execution",
            );
        }
        let action = ActionRef {
            tag: COMMAND_TAG,
            action: COMMAND_RUN_ACTION,
        };
        if !self.plan.action_mediation.requires_action(&action)
            || !self.plan.action_mediation.has_default_action(&action)
        {
            return ControlSignal::missing_checked_fact(
                "std.host.command.run requires checked default action facts for Command.run",
                span,
            );
        }
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        ControlSignal::pending_command(PendingCommand {
            request: CommandRequest {
                id: request_id,
                argv: argv.clone(),
                env: env.clone(),
                cwd: None,
                stdin: stdin.clone(),
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode: CommandDecode::CommandResult,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_filesystem_callable(
        &mut self,
        callable: FilesystemCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let result = match callable {
            FilesystemCallable::ReadBytes => {
                let [path] = call_args.as_slice() else {
                    return self
                        .invalid_arguments_abort(span, "std.fs.read_bytes expects WorkspacePath");
                };
                self.workspace_path(path, FilesystemAccess::Read, span)
                    .map(|path| {
                        (
                            "Fs.read",
                            FilesystemOperation::Read { path },
                            HostBoundaryDecode::Bytes,
                        )
                    })
            }
            FilesystemCallable::WriteBytes => {
                let [path, body] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.fs.write_bytes expects WorkspacePath and bytes",
                    );
                };
                let path = std_value!(self.workspace_path(path, FilesystemAccess::Write, span));
                let contents =
                    std_value!(self.bytes_argument(body, "std.fs.write_bytes body", span,));
                Ok((
                    "Fs.write",
                    FilesystemOperation::Write {
                        path,
                        contents,
                        create_dirs: false,
                    },
                    HostBoundaryDecode::Unit,
                ))
            }
            FilesystemCallable::List => {
                let [path] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(span, "std.fs.list expects WorkspacePath");
                };
                self.workspace_path(path, FilesystemAccess::Read, span)
                    .map(|path| {
                        (
                            "Fs.list",
                            FilesystemOperation::ReadDir { path },
                            HostBoundaryDecode::PathList,
                        )
                    })
            }
            FilesystemCallable::Stat => {
                let [path] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(span, "std.fs.stat expects WorkspacePath");
                };
                self.workspace_path(path, FilesystemAccess::Read, span)
                    .map(|path| {
                        (
                            "Fs.stat",
                            FilesystemOperation::Stat { path },
                            HostBoundaryDecode::FilesystemStat,
                        )
                    })
            }
            FilesystemCallable::AtomicReplace => {
                let [path, body] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.fs.atomic_replace expects WorkspacePath and bytes",
                    );
                };
                let path = std_value!(self.workspace_path(path, FilesystemAccess::Write, span));
                let contents =
                    std_value!(self.bytes_argument(body, "std.fs.atomic_replace body", span,));
                Ok((
                    "Fs.atomic_replace",
                    FilesystemOperation::AtomicReplace { path, contents },
                    HostBoundaryDecode::Unit,
                ))
            }
        };
        let (action_name, operation, decode) = std_value!(result);
        let _action = std_value!(self.require_checked_default_action(action_name, span));
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Filesystem(FilesystemRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_tcp_callable(
        &mut self,
        callable: TcpCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match callable {
            TcpCallable::Connect => {
                let [host, port, _options] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.net.tcp.connect expects Host, Port, TcpOptions",
                    );
                };
                let host =
                    std_value!(
                        self.string_support_argument(host, &["host", "name"], "Host", span,)
                    );
                let port = std_value!(self.port_argument(port, span));
                let _action =
                    std_value!(self.require_checked_default_action("Net.tcp_connect", span));
                ControlSignal::pending_host(PendingHostBoundary {
                    request: HostBoundaryRequest::Tcp(TcpConnectRequest {
                        id: self.next_host_request_id(),
                        operation: TcpConnectOperation::Connect {
                            endpoint: TcpEndpoint { host, port },
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    }),
                    decode: HostBoundaryDecode::TcpStream,
                    span,
                    continuation: Continuation::BlockValue,
                })
            }
        }
    }

    fn execute_stream_callable(
        &mut self,
        callable: StreamCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let (action_name, operation, decode) = match callable {
            StreamCallable::Read => {
                let (stream, max_bytes, timeout) = match call_args.as_slice() {
                    [stream, max_bytes] => (stream, max_bytes, None),
                    [stream, max_bytes, timeout] => (stream, max_bytes, Some(timeout)),
                    _ => {
                        return self.invalid_arguments_abort(
                            span,
                            "std.stream.read expects ByteStream, max byte count, and optional Timeout",
                        );
                    }
                };
                let timeout_ms = std_value!(self.optional_timeout_ms(timeout, span));
                let stream = std_value!(self.stream_ref_argument(stream, span));
                let max_bytes =
                    std_value!(self.usize_argument(max_bytes, "stream read limit", span));
                (
                    "Stream.read",
                    StreamOperation::Read {
                        stream,
                        max_bytes,
                        timeout_ms,
                    },
                    HostBoundaryDecode::StreamRead,
                )
            }
            StreamCallable::ReadUntilLimit => {
                let (stream, limit, timeout) = match call_args.as_slice() {
                    [stream, limit] => (stream, limit, None),
                    [stream, limit, timeout] => (stream, limit, Some(timeout)),
                    _ => {
                        return self.invalid_arguments_abort(
                            span,
                            "std.stream.read_until_limit expects ByteStream, ByteLimit, and optional Timeout",
                        );
                    }
                };
                let timeout_ms = std_value!(self.optional_timeout_ms(timeout, span));
                let stream = std_value!(self.stream_ref_argument(stream, span));
                let limit_bytes =
                    std_value!(self.usize_argument(limit, "stream read-until-limit budget", span,));
                (
                    "Stream.read",
                    StreamOperation::ReadUntilLimit {
                        stream,
                        limit_bytes,
                        timeout_ms,
                    },
                    HostBoundaryDecode::StreamBytes,
                )
            }
            StreamCallable::WriteAll => {
                let [stream, body] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.stream.write_all expects ByteStream and bytes",
                    );
                };
                let stream = std_value!(self.stream_ref_argument(stream, span));
                let body = std_value!(self.bytes_argument(body, "std.stream.write_all body", span));
                (
                    "Stream.write",
                    StreamOperation::WriteAll { stream, body },
                    HostBoundaryDecode::Unit,
                )
            }
            StreamCallable::Flush => {
                let [stream] = call_args.as_slice() else {
                    return self
                        .invalid_arguments_abort(span, "std.stream.flush expects ByteStream");
                };
                let stream = std_value!(self.stream_ref_argument(stream, span));
                (
                    "Stream.flush",
                    StreamOperation::Flush { stream },
                    HostBoundaryDecode::Unit,
                )
            }
            StreamCallable::Close => {
                let [stream] = call_args.as_slice() else {
                    return self
                        .invalid_arguments_abort(span, "std.stream.close expects ByteStream");
                };
                let stream = std_value!(self.stream_ref_argument(stream, span));
                (
                    "Stream.close",
                    StreamOperation::Close { stream },
                    HostBoundaryDecode::Unit,
                )
            }
        };
        let _action = std_value!(self.require_checked_default_action(action_name, span));
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Stream(StreamRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn execute_tls_callable(
        &mut self,
        callable: TlsCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match callable {
            TlsCallable::Connect => {
                let [stream, server_name, _config] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.tls.connect expects TcpStream, Host, TlsConfig",
                    );
                };
                let stream = std_value!(self.tcp_stream_ref_argument(stream, span));
                let server_name = std_value!(self.string_support_argument(
                    server_name,
                    &["host", "name"],
                    "Host",
                    span,
                ));
                let _action =
                    std_value!(self.require_checked_default_action("Tls.handshake", span));
                ControlSignal::pending_host(PendingHostBoundary {
                    request: HostBoundaryRequest::Tls(TlsConnectRequest {
                        id: self.next_host_request_id(),
                        operation: TlsConnectOperation::Connect {
                            stream,
                            server_name,
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    }),
                    decode: HostBoundaryDecode::TlsStream,
                    span,
                    continuation: Continuation::BlockValue,
                })
            }
        }
    }

    fn execute_secret_callable(
        &mut self,
        callable: SecretCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match callable {
            SecretCallable::Read => {
                let [key] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(span, "std.secret.read expects SecretKey");
                };
                let key = std_value!(self.string_support_argument(
                    key,
                    &["key", "name"],
                    "SecretKey",
                    span,
                ));
                let _action = std_value!(self.require_checked_default_action("Secret.read", span));
                ControlSignal::pending_host(PendingHostBoundary {
                    request: HostBoundaryRequest::Secret(SecretRequest {
                        id: self.next_host_request_id(),
                        operation: SecretOperation::Read { key },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    }),
                    decode: HostBoundaryDecode::SecretValue,
                    span,
                    continuation: Continuation::BlockValue,
                })
            }
            SecretCallable::HmacSha256 => {
                let [key, body] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.crypto.hmac_sha256 expects SecretValue and bytes",
                    );
                };
                let InterpValue::HostHandle(handle) = key else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.crypto.hmac_sha256 expects a sealed SecretValue host handle",
                    );
                };
                let Some(key) = handle.secret_ref() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.crypto.hmac_sha256 expects a sealed SecretValue host handle",
                    );
                };
                let body =
                    std_value!(self.bytes_argument(body, "std.crypto.hmac_sha256 body", span,));
                let _action = std_value!(self.require_checked_default_action("Secret.use", span));
                ControlSignal::pending_host(PendingHostBoundary {
                    request: HostBoundaryRequest::Secret(SecretRequest {
                        id: self.next_host_request_id(),
                        operation: SecretOperation::HmacSha256 { key, body },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    }),
                    decode: HostBoundaryDecode::SecretBytes,
                    span,
                    continuation: Continuation::BlockValue,
                })
            }
        }
    }

    fn execute_browser_callable(
        &mut self,
        callable: BrowserCallable,
        call_args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let (action_name, operation, decode) = match callable {
            BrowserCallable::Attach | BrowserCallable::Create => {
                let [profile] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.browser.protocol attach/create expects BrowserProfile",
                    );
                };
                let profile = std_value!(self.string_support_argument(
                    profile,
                    &["profile", "name"],
                    "BrowserProfile",
                    span,
                ));
                let operation = match callable {
                    BrowserCallable::Attach => BrowserProtocolOperation::Attach { profile },
                    BrowserCallable::Create => BrowserProtocolOperation::Create { profile },
                    BrowserCallable::Send
                    | BrowserCallable::Recv
                    | BrowserCallable::Screenshot
                    | BrowserCallable::Close => unreachable!(),
                };
                (
                    "Browser.attach",
                    operation,
                    HostBoundaryDecode::BrowserPayload,
                )
            }
            BrowserCallable::Send => {
                let [session, message] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.browser.protocol.send expects BrowserSession and BrowserMessage",
                    );
                };
                let session = std_value!(self.browser_session_argument(session, span));
                let message = std_value!(self.bytes_argument(message, "BrowserMessage", span));
                (
                    "Browser.send",
                    BrowserProtocolOperation::Send { session, message },
                    HostBoundaryDecode::Unit,
                )
            }
            BrowserCallable::Recv => {
                let [session] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.browser.protocol.recv expects BrowserSession",
                    );
                };
                let session = std_value!(self.browser_session_argument(session, span));
                (
                    "Browser.recv",
                    BrowserProtocolOperation::Recv {
                        session,
                        max_bytes: 1024 * 1024,
                    },
                    HostBoundaryDecode::BrowserPayload,
                )
            }
            BrowserCallable::Screenshot => {
                let [session] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.browser.protocol.screenshot expects BrowserSession",
                    );
                };
                let session = std_value!(self.browser_session_argument(session, span));
                (
                    "Browser.screenshot",
                    BrowserProtocolOperation::Screenshot {
                        session,
                        max_bytes: 1024 * 1024,
                    },
                    HostBoundaryDecode::BrowserPayload,
                )
            }
            BrowserCallable::Close => {
                let [session] = call_args.as_slice() else {
                    return self.invalid_arguments_abort(
                        span,
                        "std.browser.protocol.close expects BrowserSession",
                    );
                };
                let session = std_value!(self.browser_session_argument(session, span));
                (
                    "Browser.close",
                    BrowserProtocolOperation::Close { session },
                    HostBoundaryDecode::Unit,
                )
            }
        };
        let _action = std_value!(self.require_checked_default_action(action_name, span));
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Browser(BrowserProtocolRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode,
            span,
            continuation: Continuation::BlockValue,
        })
    }

    fn require_checked_default_action(
        &self,
        action_name: &str,
        span: Span,
    ) -> Result<ActionRef, ExecutionFault> {
        let action = self
            .checked
            .effect_registry
            .action_by_name(action_name)
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!(
                        "standard action `{action_name}` is missing from the checked effect registry"
                    ),
                )
            })?;
        if !self.plan.action_mediation.requires_action(&action)
            || !self.plan.action_mediation.has_default_action(&action)
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("`{action_name}` execution requires checked default action facts"),
            ));
        }
        Ok(action)
    }

    fn invalid_arguments_abort(&self, span: Span, message: impl Into<String>) -> ControlSignal {
        ControlSignal::invalid_arguments(message, span)
    }

    fn workspace_path(
        &self,
        value: &InterpValue,
        access: FilesystemAccess,
        span: Span,
    ) -> Result<WorkspacePath, ExecutionFault> {
        let relative = self.path_string_argument(value, span)?;
        let roots = match access {
            FilesystemAccess::Read => &self.host_context.authority.sandbox.filesystem.read_roots,
            FilesystemAccess::Write => &self.host_context.authority.sandbox.filesystem.write_roots,
        };
        let [root] = roots.as_slice() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!(
                    "std.fs requires exactly one configured {} workspace root",
                    access.name()
                ),
            ));
        };
        let path = Path::new(&relative);
        let workspace_path = match access {
            FilesystemAccess::Read => root.resolve_existing(path),
            FilesystemAccess::Write => root.resolve_for_create(path),
        };
        match workspace_path {
            Ok(path) => Ok(path),
            Err(error) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("invalid workspace path: {}", error.message),
            )),
        }
    }

    fn path_string_argument(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        self.string_support_argument(value, &["value", "path"], "WorkspacePath", span)
    }

    fn string_support_argument(
        &self,
        value: &InterpValue,
        field_names: &[&str],
        expected: &str,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        let value = nominal_representation_ref(value);
        match value {
            InterpValue::String(value) => Ok(value.clone()),
            InterpValue::Record(record) => {
                let fields = record.snapshot();
                field_names
                    .iter()
                    .find_map(|field_name| {
                        fields.iter().find_map(|(name, value)| {
                            if name == field_name
                                && let InterpValue::String(value) =
                                    nominal_representation_ref(value)
                            {
                                return Some(value.clone());
                            }
                            None
                        })
                    })
                    .ok_or_else(|| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::InvalidArguments,
                            span,
                            format!("{expected} is missing its checked string field"),
                        )
                    })
            }
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("{expected} must be a string support value"),
            )),
        }
    }

    fn bytes_argument(
        &self,
        value: &InterpValue,
        expected: &str,
        span: Span,
    ) -> Result<Vec<u8>, ExecutionFault> {
        let value = nominal_representation_ref(value);
        match value {
            InterpValue::Bytes(bytes) => Ok(bytes.clone()),
            InterpValue::Record(record) => record
                .snapshot()
                .iter()
                .find_map(|(name, value)| {
                    if (name == "body" || name == "bytes" || name == "message")
                        && let InterpValue::Bytes(bytes) = nominal_representation_ref(value)
                    {
                        return Some(bytes.clone());
                    }
                    None
                })
                .ok_or_else(|| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("{expected} is missing its checked bytes field"),
                    )
                }),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("{expected} must be bytes"),
            )),
        }
    }

    fn port_argument(&self, value: &InterpValue, span: Span) -> Result<u16, ExecutionFault> {
        let value = nominal_representation_ref(value);
        let port = match value {
            InterpValue::Number(port) => port.as_u32(),
            InterpValue::Record(record) => record.snapshot().iter().find_map(|(name, value)| {
                if name == "port" || name == "value" {
                    return nominal_representation_ref(value)
                        .as_number()
                        .and_then(|value| value.as_u32());
                }
                None
            }),
            _ => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "Port must be an integer support value",
                ));
            }
        };
        let Some(port) = port.and_then(|port| u16::try_from(port).ok()) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "TCP port must be in range 0..=65535",
            ));
        };
        Ok(port)
    }

    fn usize_argument(
        &self,
        value: &InterpValue,
        expected: &str,
        span: Span,
    ) -> Result<usize, ExecutionFault> {
        let value = nominal_representation_ref(value);
        let value = match value {
            InterpValue::Number(value) => value.as_usize(),
            InterpValue::Record(record) => {
                let fields = record.snapshot();
                let Some(value) = int_field(&fields, &["value", "bytes", "limit", "max_bytes"])
                else {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("{expected} must include a non-negative integer field"),
                    ));
                };
                usize::try_from(value).ok()
            }
            _ => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("{expected} must be a non-negative integer"),
                ));
            }
        };
        value.ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                format!("{expected} is outside usize range"),
            )
        })
    }

    fn optional_timeout_ms(
        &self,
        value: Option<&InterpValue>,
        span: Span,
    ) -> Result<Option<u64>, ExecutionFault> {
        match value {
            None | Some(InterpValue::OptionNone) => Ok(None),
            Some(InterpValue::OptionSome(value)) => self.timeout_ms_argument(value, span).map(Some),
            Some(value) => self.timeout_ms_argument(value, span).map(Some),
        }
    }

    fn timeout_ms_argument(&self, value: &InterpValue, span: Span) -> Result<u64, ExecutionFault> {
        let value = nominal_representation_ref(value);
        let timeout = match value {
            InterpValue::Number(timeout) => timeout.as_u64(),
            InterpValue::Record(record) => {
                let fields = record.snapshot();
                fields.iter().find_map(|(name, value)| {
                    if matches!(name.as_str(), "ms" | "millis" | "milliseconds" | "value") {
                        return nominal_representation_ref(value)
                            .as_number()
                            .and_then(|value| value.as_u64());
                    }
                    None
                })
            }
            _ => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "Timeout must be an integer millisecond support value",
                ));
            }
        };
        timeout.ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "Timeout is outside u64 millisecond range",
            )
        })
    }

    fn stream_ref_argument(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<ByteStreamRef, ExecutionFault> {
        let InterpValue::HostHandle(handle) = value else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "ByteStream must be a sealed host handle",
            ));
        };
        handle.byte_stream_ref().ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "host handle is not a ByteStream",
            )
        })
    }

    fn tcp_stream_ref_argument(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<TcpStreamRef, ExecutionFault> {
        let InterpValue::HostHandle(handle) = value else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "TcpStream must be a sealed host handle",
            ));
        };
        handle.tcp_stream_ref().ok_or_else(|| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "host handle is not a TcpStream",
            )
        })
    }

    fn browser_session_argument(
        &self,
        value: &InterpValue,
        span: Span,
    ) -> Result<String, ExecutionFault> {
        let InterpValue::HostHandle(handle) = value else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "BrowserSession must be a sealed host handle",
            ));
        };
        handle
            .browser_session_id()
            .map(str::to_owned)
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    "host handle is not a BrowserSession",
                )
            })
    }

    pub(super) fn std_intrinsic(
        &self,
        symbol: SymbolId,
    ) -> Option<crate::intrinsic::dispatch::StdIntrinsicIdentity> {
        self.plan.dispatch.std_intrinsic(symbol)
    }
}

#[derive(Clone, Copy)]
enum FilesystemAccess {
    Read,
    Write,
}

impl FilesystemAccess {
    fn name(self) -> &'static str {
        match self {
            FilesystemAccess::Read => "read",
            FilesystemAccess::Write => "write",
        }
    }
}

fn int_field(fields: &[(String, InterpValue)], names: &[&str]) -> Option<i64> {
    names.iter().find_map(|field_name| {
        fields.iter().find_map(|(name, value)| {
            if name == field_name {
                return nominal_representation_ref(value)
                    .as_number()
                    .and_then(|value| value.as_i64());
            }
            None
        })
    })
}

fn nominal_representation_ref(mut value: &InterpValue) -> &InterpValue {
    while let InterpValue::Nominal { value: inner, .. } = value {
        value = inner;
    }
    value
}

fn json_ok_result(value: InterpValue) -> InterpValue {
    InterpValue::Variant {
        name: "Ok".to_owned(),
        fields: vec![value],
    }
}

fn json_error_result(message: String) -> InterpValue {
    InterpValue::Variant {
        name: "Err".to_owned(),
        fields: vec![InterpValue::Variant {
            name: "InvalidJson".to_owned(),
            fields: vec![InterpValue::String(message)],
        }],
    }
}

fn runtime_limit_constructor_name(kind: StdLimitKind) -> &'static str {
    match kind {
        StdLimitKind::Iterations => "Iterations",
        StdLimitKind::Tokens => "Tokens",
        StdLimitKind::ContextTokens => "ContextTokens",
        StdLimitKind::Cost => "Cost",
        StdLimitKind::WallTime => "WallTime",
        StdLimitKind::Attempts => "Attempts",
    }
}

fn memory_store_callable_name(
    callable: crate::intrinsic::dispatch::MemoryStoreCallable,
) -> &'static str {
    use crate::intrinsic::dispatch::MemoryStoreCallable;

    match callable {
        MemoryStoreCallable::Get => "get",
        MemoryStoreCallable::Put => "put",
        MemoryStoreCallable::PutVersioned => "put_versioned",
        MemoryStoreCallable::Contains => "contains",
        MemoryStoreCallable::Keys => "keys",
        MemoryStoreCallable::Insert => "insert",
        MemoryStoreCallable::Delete => "delete",
        MemoryStoreCallable::DeleteVersioned => "delete_versioned",
        MemoryStoreCallable::Update => "update",
        MemoryStoreCallable::Clear => "clear",
        MemoryStoreCallable::Select => "select",
        MemoryStoreCallable::Query => "query",
        MemoryStoreCallable::Scan => "scan",
        MemoryStoreCallable::RelatedTo => "related_to",
        MemoryStoreCallable::Upsert => "upsert",
    }
}

pub(super) fn builtin_abort_message(
    error: &crate::intrinsic::pure::AdapterError,
) -> Option<String> {
    match error {
        crate::intrinsic::pure::AdapterError::Builtin(BuiltinError::Abort { message }) => {
            Some(message.clone())
        }
        crate::intrinsic::pure::AdapterError::Builtin(BuiltinError::AssertionFailed) => {
            Some("assertion failed".to_owned())
        }
        _ => None,
    }
}
