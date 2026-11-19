use super::*;
use crate::intrinsic::dispatch::SessionContextCallable;
use etas_host::session::{MessageEnvelope, message_envelope_from_host_value};
use etas_host::{HostError, SessionCursor, SessionOperation, SessionRef, SessionResult};

impl EvalContext<'_> {
    pub(crate) fn source_session_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let ("Session", name) = self.source_default_signature(perform)? else {
            return None;
        };
        let proof = perform.checked_action.as_ref().unwrap();
        let span = perform.span;
        let signal = match name {
            "publish" | "reconcile" => {
                let kind = if name == "publish" {
                    SessionContextCallable::Publish
                } else {
                    SessionContextCallable::Reconcile
                };
                match self.session_context_operation(kind, &proof.parameters, &perform.args) {
                    Ok(operation) => self.pending_session_context(operation, proof.output, span),
                    Err(error) => self.storage_error_signal(error, span),
                }
            }
            "resolve" | "append" | "read" => {
                match self.source_session_operation(name, &proof.parameters, &perform.args) {
                    Ok(operation) => ControlSignal::pending_session(PendingSession {
                        request: etas_host::SessionRequest {
                            id: self.next_host_request_id(),
                            operation,
                            authority: self.host_authority(),
                            trace: self.host_trace(),
                            budget: self.host_budget(),
                        },
                        decode: SessionDecode::SourceValue {
                            result_type: proof.output,
                        },
                        span,
                        continuation: Continuation::BlockValue,
                    }),
                    Err(error) if name == "read" => self.storage_error_signal(error, span),
                    Err(error) => ControlSignal::invalid_arguments(error.message, span),
                }
            }
            _ => return None,
        };
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    fn source_session_operation(
        &self,
        name: &str,
        parameters: &[etas_types::TypeId],
        args: &[InterpValue],
    ) -> Result<SessionOperation, HostError> {
        use crate::eval::storage::{invalid, text};
        match (name, parameters, args) {
            ("resolve", [config_type], [config]) => {
                let config = self.checked_session_config(config, Some(config_type))?;
                Ok(SessionOperation::Resolve { config })
            }
            ("append", [message_type], [message]) => {
                crate::value::contract::runtime::validate_value(
                    self.checked,
                    self.plan,
                    message,
                    *message_type,
                    &self.storage_limits,
                )
                .map_err(invalid)?;
                let message = crate::eval::message_context::source_message_value(message)
                    .and_then(|message| session_message_from_message_value(&message))
                    .map_err(invalid)?;
                Ok(SessionOperation::Append { message })
            }
            ("read", [config_type, cursor_type, limit_type], [config, cursor, limit]) => {
                let config = self.checked_session_config(config, Some(config_type))?;
                let cursor = self
                    .checked_option_payload(cursor, *cursor_type)
                    .ok_or_else(|| {
                        invalid("Session.read requires a checked Option<SessionCursor>")
                    })?
                    .map(|cursor| {
                        let fields = self.checked_storage_record(
                            cursor,
                            &["std", "agent", "session", "SessionCursor"],
                        )?;
                        Ok(SessionCursor {
                            opaque: text(fields, "opaque", &self.storage_limits)?.to_owned(),
                        })
                    })
                    .transpose()?;
                let limit = self
                    .checked_option_payload(limit, *limit_type)
                    .ok_or_else(|| invalid("Session.read requires a checked Option<u32>"))?
                    .map(|limit| {
                        limit
                            .as_number()
                            .and_then(|number| number.as_u32())
                            .filter(|limit| {
                                *limit > 0
                                    && *limit as usize <= self.storage_limits.max_page_entries
                            })
                            .ok_or_else(|| invalid("session page limit is outside checked bounds"))
                    })
                    .transpose()?;
                Ok(SessionOperation::Load {
                    session: SessionRef { id: config.id },
                    context: config.context,
                    cursor,
                    limit,
                })
            }
            _ => Err(invalid(
                "session action payload differs from its checked signature",
            )),
        }
    }

    pub(super) fn decode_source_session_result(
        &self,
        operation: &SessionOperation,
        result: &SessionResult,
        ty: etas_types::TypeId,
    ) -> Result<InterpValue, String> {
        let value = match (operation, result) {
            (SessionOperation::Resolve { .. }, SessionResult::Resolved { session, .. }) => {
                HostValue::String(session.id.clone())
            }
            (SessionOperation::Append { .. }, SessionResult::Appended { message, .. }) => {
                source_message_response(MessageEnvelope {
                    id: message.id.clone(),
                    from: message.from.clone(),
                    to: message.to.clone(),
                    role: message.role,
                    session: Some(message.session.clone()),
                    created_at: message.created_at.clone(),
                    payload: message.payload.clone(),
                    provenance: message.provenance.clone(),
                })?
            }
            (SessionOperation::Load { session, limit, .. }, SessionResult::History { .. }) => {
                let mut value = etas_host::session::session_history_selection_value(
                    result,
                    session,
                    *limit,
                    &self.storage_limits,
                )
                .map_err(|error| error.message)?;
                let HostValue::Record(fields) = &mut value else {
                    unreachable!("validated session page representation");
                };
                let Some((_, HostValue::List(messages))) =
                    fields.iter_mut().find(|(name, _)| name == "messages")
                else {
                    unreachable!("validated session page messages");
                };
                *messages = std::mem::take(messages)
                    .into_iter()
                    .map(|message| {
                        let message = message_envelope_from_host_value(message)
                            .map_err(|error| error.message)?;
                        source_message_response(message)
                    })
                    .collect::<Result<_, _>>()?;
                value
            }
            _ => return Err("session host returned a different operation's result".into()),
        };
        crate::eval::host_value::host_to_checked_interp_value(
            value,
            ty,
            self.checked,
            &self.storage_limits,
        )
    }
}

fn source_message_response(message: MessageEnvelope) -> Result<HostValue, String> {
    use etas_host::SessionMessageRole as Role;
    let millis = etas_host::session::session_timestamp_millis(&message.created_at)
        .ok_or("session message has no representable timestamp")?;
    let role = match message.role {
        Role::System => "System",
        Role::User => "User",
        Role::Assistant => "Assistant",
        Role::Tool => "Tool",
    };
    let provenance = message
        .provenance
        .as_ref()
        .map(provenance_from_host)
        .transpose()?
        .map(|provenance| {
            HostValue::Record(vec![
                ("trace_id".into(), optional_text(provenance.trace_id)),
                ("source".into(), optional_text(provenance.source)),
            ])
        });
    Ok(HostValue::Record(vec![
        ("id".into(), HostValue::String(message.id)),
        ("from".into(), optional_text(message.from)),
        (
            "role".into(),
            HostValue::Variant {
                name: role.into(),
                fields: Vec::new(),
            },
        ),
        (
            "session".into(),
            optional_text(message.session.map(|session| session.id)),
        ),
        (
            "created_at".into(),
            HostValue::Record(vec![("unix_millis".into(), HostValue::UInt(millis))]),
        ),
        ("body".into(), message.payload.clone()),
        ("content".into(), message.payload),
        ("provenance".into(), host_option(provenance)),
    ]))
}

fn optional_text(text: Option<String>) -> HostValue {
    host_option(text.map(HostValue::String))
}

fn host_option(value: Option<HostValue>) -> HostValue {
    match value {
        Some(value) => HostValue::Variant {
            name: "Some".into(),
            fields: vec![value],
        },
        None => HostValue::Variant {
            name: "None".into(),
            fields: Vec::new(),
        },
    }
}
