use super::*;
use etas_host::HostError;

impl<'a> EvalContext<'a> {
    pub(crate) fn source_browser_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Browser", action) = self.source_default_signature(perform)? else {
            return None;
        };
        let callable = match action {
            "attach" => BrowserCallable::Attach,
            "create" => BrowserCallable::Create,
            "send" => BrowserCallable::Send,
            "recv" => BrowserCallable::Recv,
            "screenshot" => BrowserCallable::Screenshot,
            "close" => BrowserCallable::Close,
            _ => return None,
        };
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked browser default");
        let span = perform.span;
        let mut args = perform.args.clone();
        if !matches!(callable, BrowserCallable::Attach | BrowserCallable::Create) {
            match crate::value::host_handle::open_source_handle(
                self.checked,
                proof.parameters[0],
                "connection",
                &args[0],
            ) {
                Ok(inner) => args[0] = inner.clone(),
                Err(reason) => return Some(ControlSignal::invalid_arguments(reason, span)),
            }
        }
        if callable == BrowserCallable::Send {
            let InterpValue::Json(_) = &args[1] else {
                return Some(ControlSignal::missing_checked_fact(
                    "Browser.send requires a checked JSON message",
                    span,
                ));
            };
            match super::host_value::interp_to_host_json_string(&args[1]) {
                Ok(value) => args[1] = InterpValue::Bytes(value.into_bytes().into()),
                Err(error) => return Some(ControlSignal::invalid_arguments(error.message, span)),
            }
        }
        let decode = HostBoundaryDecode::SourceBrowser {
            result_type: proof.output,
        };
        let signal = self.execute_browser_callable(callable, args, decode, span);
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    pub(crate) fn source_secret_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Secret", action) = self.source_default_signature(perform)? else {
            return None;
        };
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked secret default");
        let span = perform.span;
        let mut args = perform.args.clone();
        let (callable, key_type) = match action {
            "read" => (SecretCallable::Read, proof.parameters[0]),
            "use" => {
                let handle = crate::value::host_handle::open_source_handle(
                    self.checked,
                    proof.parameters[0],
                    "handle",
                    &args[0],
                );
                let Ok(InterpValue::HostHandle(handle)) = handle else {
                    return Some(ControlSignal::invalid_arguments(
                        "Secret.use requires a checked sealed secret",
                        span,
                    ));
                };
                let key_type = handle.nominal_type();
                args[0] = InterpValue::HostHandle(handle.clone());
                (SecretCallable::HmacSha256, key_type)
            }
            _ => return None,
        };
        let decode = HostBoundaryDecode::SourceSecret {
            result_type: proof.output,
        };
        let signal =
            self.execute_secret_callable(callable, key_type, proof.output, decode, args, span);
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    pub(crate) fn source_network_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let (owner, action) = self.source_default_signature(perform)?;
        let proof = perform
            .checked_action
            .as_ref()
            .expect("checked source default");
        let span = perform.span;
        let decode = HostBoundaryDecode::SourceConnection {
            result_type: proof.output,
        };
        if (owner, action) == ("Net", "tcp_connect") {
            let signal =
                self.execute_tcp_callable(TcpCallable::Connect, perform.args.clone(), decode, span);
            return Some(self.continue_chain_signal(signal, perform.continuation.clone()));
        }
        let stream = match (owner, action) {
            ("Tls", "handshake") => None,
            ("Stream", "read") => Some(StreamCallable::Read),
            ("Stream", "write") => Some(StreamCallable::WriteAll),
            ("Stream", "flush") => Some(StreamCallable::Flush),
            ("Stream", "close") => Some(StreamCallable::Close),
            _ => return None,
        };
        let mut args = perform.args.clone();
        let [handle, ..] = args.as_mut_slice() else {
            return Some(ControlSignal::missing_checked_fact(
                "network action has no checked handle",
                span,
            ));
        };
        match crate::value::host_handle::open_source_handle(
            self.checked,
            proof.parameters[0],
            "connection",
            handle,
        ) {
            Ok(inner) => *handle = inner.clone(),
            Err(reason) => return Some(ControlSignal::invalid_arguments(reason, span)),
        }
        if matches!(stream, Some(StreamCallable::Read)) {
            let [_, _, timeout] = args.as_mut_slice() else {
                return Some(ControlSignal::missing_checked_fact(
                    "stream read requires its checked timeout",
                    span,
                ));
            };
            *timeout = match self.checked_option_payload(timeout, proof.parameters[2]) {
                Some(None) => InterpValue::OptionNone,
                Some(Some(value)) => InterpValue::OptionSome(value.clone().into()),
                None => {
                    return Some(ControlSignal::missing_checked_fact(
                        "stream read timeout differs from its checked Option",
                        span,
                    ));
                }
            };
        }
        let signal = match stream {
            Some(stream) => self.execute_stream_callable(stream, proof.output, args, span),
            None => self.execute_tls_callable(TlsCallable::Connect, args, decode, span),
        };
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    pub(crate) fn network_host_error_signal(
        &mut self,
        host: PendingHostBoundary,
        error: HostError,
    ) -> Option<ControlSignal> {
        let (error_type, fact_name) = match &host.request {
            HostBoundaryRequest::Tcp(_) => (
                self.known_std_types.network_error,
                "std.net.tcp NetworkError",
            ),
            HostBoundaryRequest::Tls(_) => (self.known_std_types.tls_error, "std.tls TlsError"),
            HostBoundaryRequest::Browser(_) if self.checked.hir.source_standard_types => (
                super::resolve_std_type(
                    self.checked,
                    &["std", "browser", "protocol", "BrowserError"],
                ),
                "std.browser.protocol.BrowserError",
            ),
            HostBoundaryRequest::Secret(request) if self.checked.hir.source_standard_types => {
                match request.operation {
                    etas_host::SecretOperation::Read { .. } => (
                        super::resolve_std_type(self.checked, &["std", "secret", "SecretError"]),
                        "std.secret.SecretError",
                    ),
                    etas_host::SecretOperation::HmacSha256 { .. } => (
                        super::resolve_std_type(self.checked, &["std", "crypto", "CryptoError"]),
                        "std.crypto.CryptoError",
                    ),
                }
            }
            _ => return None,
        };
        let Some(error_type) = error_type else {
            return Some(ControlSignal::missing_checked_fact(
                format!("network host failure requires checked {fact_name} type"),
                host.span,
            ));
        };
        let perform = PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: network_error_raise_action(host.span),
            error_type: Some(error_type),
            args: vec![typed_host_error_value(
                error_type,
                error,
                self.checked.hir.source_standard_types,
            )],
            span: host.span,
            continuation: host.continuation,
            residual: None,
        };
        Some(self.propagate_perform_signal(perform))
    }
}

fn network_error_raise_action(span: Span) -> ResolvedActionRef {
    ResolvedActionRef {
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&["Error"], span),
            args: Vec::new(),
            span,
        },
        action: "raise".to_owned(),
        action_symbol: ResolveResult::Unresolved,
        span,
    }
}

fn typed_host_error_value(
    error_type: etas_types::TypeId,
    error: HostError,
    source: bool,
) -> InterpValue {
    let mut fields = vec![
        (
            "code".into(),
            InterpValue::String(error.code.as_str().to_owned().into()),
        ),
        ("message".into(), InterpValue::String(error.message.into())),
    ];
    if !source {
        let details = error
            .details
            .into_iter()
            .map(|detail| {
                InterpValue::Record(
                    vec![
                        ("key".into(), InterpValue::String(detail.key.into())),
                        ("value".into(), InterpValue::String(detail.value.into())),
                    ]
                    .into(),
                )
            })
            .collect::<Vec<_>>();
        fields.push(("details".into(), InterpValue::Array(details.into())));
    }
    InterpValue::Nominal {
        ty: error_type,
        value: InterpValue::Record(fields.into()).into(),
    }
}
