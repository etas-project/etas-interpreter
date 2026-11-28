use super::*;
use etas_host::{PtyOperation, PtySize};

impl EvalContext<'_> {
    pub(super) fn source_pty_operation(
        &self,
        perform: &PendingPerform,
        action: &str,
    ) -> Result<(PtyOperation, HostBoundaryDecode), ExecutionFault> {
        let span = perform.span;
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked source Command action");
        match (action, perform.args.as_slice(), proof.parameters.as_slice()) {
            ("start_pty", [command, sandbox, size], [command_ty, sandbox_ty, size_ty]) => {
                let command = self.source_command_input(command, *command_ty, span)?;
                let selector = command_record(sandbox, *sandbox_ty)
                    .and_then(|r| r.sandbox_selector())
                    .map(|path| path.to_vec());
                let profile = pty_profile(sandbox, *sandbox_ty, span)?.to_owned();
                Ok((
                    PtyOperation::Start {
                        argv: command.argv,
                        env: command.env,
                        cwd: command.cwd,
                        stdin: command.stdin,
                        size: self.pty_size(size, *size_ty, span)?,
                        profile,
                        selector,
                    },
                    HostBoundaryDecode::PtyHandle {
                        result_type: proof.output,
                    },
                ))
            }
            ("read_pty", [handle, limit], [ty, _]) => Ok((
                PtyOperation::Read {
                    process: self.pty_identity(handle, *ty, span)?.0.to_owned(),
                    max_bytes: self.usize_argument(limit, "PTY read bound", span)?,
                },
                HostBoundaryDecode::PtyEvent {
                    result_type: proof.output,
                },
            )),
            ("write_pty", [handle, InterpValue::Bytes(bytes)], [ty, _]) => Ok((
                PtyOperation::Write {
                    process: self.pty_identity(handle, *ty, span)?.0.to_owned(),
                    bytes: bytes.to_vec(),
                },
                HostBoundaryDecode::Unit,
            )),
            ("resize_pty", [handle, size], [ty, size_ty]) => Ok((
                PtyOperation::Resize {
                    process: self.pty_identity(handle, *ty, span)?.0.to_owned(),
                    size: self.pty_size(size, *size_ty, span)?,
                },
                HostBoundaryDecode::Unit,
            )),
            ("stop_pty", [handle], [ty]) => Ok((
                PtyOperation::Stop {
                    process: self.pty_identity(handle, *ty, span)?.0.to_owned(),
                },
                HostBoundaryDecode::Unit,
            )),
            _ => Err(command_contract_fault(
                span,
                "PTY payload differs from its checked action",
            )),
        }
    }

    fn pty_size(
        &self,
        value: &InterpValue,
        ty: TypeId,
        span: Span,
    ) -> Result<PtySize, ExecutionFault> {
        let record = command_record(value, ty)
            .ok_or_else(|| command_contract_fault(span, "PTY size requires its checked record"))?;
        let count = |field| {
            self.usize_argument(
                record
                    .get_ref(field)
                    .ok_or_else(|| command_contract_fault(span, "missing PTY dimension"))?,
                field,
                span,
            )
        };
        Ok(PtySize {
            cols: count("cols")?,
            rows: count("rows")?,
        })
    }

    fn pty_identity<'v>(
        &self,
        value: &'v InterpValue,
        ty: TypeId,
        span: Span,
    ) -> Result<(&'v str, Option<&'v [String]>), ExecutionFault> {
        let seal = open_source_handle(self.checked, ty, "process", value)
            .map_err(|reason| command_contract_fault(span, reason))?;
        match seal {
            InterpValue::HostHandle(handle) => handle.pty_session().ok_or_else(|| {
                command_contract_fault(span, "PTY operation requires its originating session")
            }),
            _ => Err(command_contract_fault(
                span,
                "PTY operation requires its originating session",
            )),
        }
    }

    pub(in crate::eval) fn source_pty_profile(
        &self,
        perform: &PendingPerform,
    ) -> Option<Vec<String>> {
        let (_, "Command", action) = self.source_action_signature(perform)? else {
            return None;
        };
        let parameters = &perform.checked_action.as_ref()?.parameters;
        let profile = match (action, perform.args.as_slice(), parameters.as_slice()) {
            ("start_pty", [_, sandbox, _], [_, ty, _]) => {
                return command_record(sandbox, *ty)?
                    .sandbox_selector()
                    .map(|p| p.to_vec());
            }
            ("read_pty" | "write_pty" | "resize_pty" | "stop_pty", [handle, ..], [ty, ..]) => {
                self.pty_identity(handle, *ty, perform.span).ok()?.1?
            }
            _ => return None,
        };
        Some(profile.to_vec())
    }

    /// A directly performed, explicitly selected action must agree with the
    /// original resource even when the value was returned by a language Handler.
    pub(in crate::eval) fn validate_pty_selector(
        &self,
        perform: &PendingPerform,
    ) -> Result<(), ExecutionFault> {
        let Some(proof) = &perform.checked_action else {
            return Ok(());
        };
        let etas_effects::Effect::AppliedAction(instance) = &proof.effect else {
            return Ok(());
        };
        if instance.action.tag != etas_effects::COMMAND_TAG {
            return Ok(());
        }
        let Some((
            _,
            "Command",
            "start_pty" | "read_pty" | "write_pty" | "resize_pty" | "stop_pty",
        )) = self.source_action_signature(perform)
        else {
            return Ok(());
        };
        let explicit = perform
            .expr
            .map(|expr| {
                etas_effects::checked_pty_profile_constraint(
                    self.checked.hir.program(),
                    &self.checked.std_registry,
                    expr,
                )
            })
            .transpose()
            .map_err(|reason| command_contract_fault(perform.span, reason))?
            .flatten();
        let expected = explicit
            .as_deref()
            .or_else(|| match instance.args.as_slice() {
                [etas_types::EffectArgRef::Path(path)] => Some(path.as_slice()),
                _ => None,
            });
        if let Some(expected) = expected
            && self.source_pty_profile(perform).as_deref() != Some(expected)
        {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                perform.span,
                "PTY action selector differs from the originating sandbox profile",
            ));
        }
        Ok(())
    }

    pub(crate) fn pty_error_signal(
        &mut self,
        boundary: PendingHostBoundary,
        code: &str,
    ) -> ControlSignal {
        if let Some(signal) = self.cancellation_signal(boundary.span) {
            return signal;
        }
        let Some(ty) = resolve_std_type(self.checked, &["std", "host", "command", "PtyError"])
        else {
            return ControlSignal::missing_checked_fact(
                "PTY action has no checked PtyError",
                boundary.span,
            );
        };
        let message = match code {
            "busy" => "terminal I/O is already in progress",
            "invalid_input" => "invalid terminal input",
            "closed" => "terminal session is closed",
            "unsupported" => "terminal operation is unavailable",
            "io" => "terminal I/O failed",
            "output_incomplete" => "terminal output was truncated",
            "cleanup_failed" => "terminal job cleanup failed",
            _ => {
                return ControlSignal::missing_checked_fact(
                    "invalid PTY error code",
                    boundary.span,
                );
            }
        };
        let record = HostValue::Record(vec![
            ("code".into(), HostValue::String(code.into())),
            ("message".into(), HostValue::String(message.into())),
        ]);
        match super::super::host_value::decode_checked_host_value(record, ty, self.checked) {
            Ok(value) => self.raise_file_error(ty, value, boundary.continuation, boundary.span),
            Err(reason) => ControlSignal::missing_checked_fact(reason, boundary.span),
        }
    }
}

fn pty_profile(value: &InterpValue, ty: TypeId, span: Span) -> Result<&str, ExecutionFault> {
    match command_record(value, ty).and_then(|record| record.get_ref("name")) {
        Some(InterpValue::String(name)) => Ok(name.as_str()),
        _ => Err(command_contract_fault(
            span,
            "PTY requires its checked sandbox profile",
        )),
    }
}
