use super::host_value::host_to_typed_interp_value;
use super::*;

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_session_result(&self, session: &PendingSession) -> Option<InterpValue> {
        if !matches!(
            session.request.operation,
            etas_host::SessionOperation::Append { .. }
        ) {
            return None;
        }
        self.completed_host_boundary_result("session", &self.session_boundary_key(session))
    }

    pub(crate) fn replay_session_signal(
        &mut self,
        session: PendingSession,
        value: InterpValue,
    ) -> ControlSignal {
        self.apply_continuation(session.continuation, value)
    }

    pub(crate) fn session_boundary_result_value(
        &mut self,
        session: &PendingSession,
        result: &etas_host::SessionResult,
    ) -> Result<Option<InterpValue>, String> {
        match (&session.decode, result) {
            (
                SessionDecode::ReturnMessage { message },
                etas_host::SessionResult::Appended { .. },
            ) => Ok(Some(InterpValue::Message(message.clone()))),
            (
                SessionDecode::ReturnConversation { payload_type },
                etas_host::SessionResult::History { .. },
            )
            | (
                SessionDecode::ReturnConversation { payload_type },
                etas_host::SessionResult::Compacted { .. },
            ) => conversation_from_session_result(result, *payload_type, &self.checked.type_store)
                .map(InterpValue::Conversation)
                .map(Some),
            _ => Ok(None),
        }
    }

    pub(crate) fn session_boundary_key(&self, session: &PendingSession) -> String {
        match &session.request.operation {
            etas_host::SessionOperation::Resolve { config } => {
                format!("session:resolve:{}", config.id)
            }
            etas_host::SessionOperation::Append { message } => {
                format!("session:append:{}:{}", message.session.id, message.id)
            }
            etas_host::SessionOperation::Load { session, .. } => {
                format!("session:load:{}", session.id)
            }
            etas_host::SessionOperation::Compact { session, .. } => {
                format!("session:compact:{}", session.id)
            }
        }
    }

    pub(crate) fn session_result_signal(
        &mut self,
        session: PendingSession,
        result: etas_host::SessionResult,
    ) -> ControlSignal {
        match (session.decode, result) {
            (
                SessionDecode::ResolveThenAppendMessage { message },
                etas_host::SessionResult::Resolved {
                    session: resolved_session,
                    ..
                },
            ) => {
                let request_id = self.next_host_request_id();
                let mut host_message = match session_message_from_message_value(&message) {
                    Ok(message) => message,
                    Err(error) => {
                        return ControlSignal::invalid_arguments(error, session.span);
                    }
                };
                host_message.session = resolved_session;
                ControlSignal::pending_session(PendingSession {
                    request: etas_host::SessionRequest {
                        id: request_id,
                        operation: etas_host::SessionOperation::Append {
                            message: host_message,
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    },
                    decode: SessionDecode::ReturnMessage { message },
                    span: session.span,
                    continuation: session.continuation,
                })
            }
            (
                SessionDecode::ResolveThenLoadConversation {
                    config,
                    payload_type,
                },
                etas_host::SessionResult::Resolved {
                    session: resolved_session,
                    ..
                },
            ) => {
                let request_id = self.next_host_request_id();
                let host_config = match session_config_to_host(&config) {
                    Ok(config) => config,
                    Err(message) => {
                        return ControlSignal::invalid_arguments(message, session.span);
                    }
                };
                ControlSignal::pending_session(PendingSession {
                    request: etas_host::SessionRequest {
                        id: request_id,
                        operation: etas_host::SessionOperation::Load {
                            session: resolved_session,
                            context: host_config.context,
                            cursor: None,
                            limit: None,
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    },
                    decode: SessionDecode::ReturnConversation { payload_type },
                    span: session.span,
                    continuation: session.continuation,
                })
            }
            (
                SessionDecode::ResolveThenCompactConversation {
                    config,
                    payload_type,
                },
                etas_host::SessionResult::Resolved {
                    session: resolved_session,
                    ..
                },
            ) => {
                let request_id = self.next_host_request_id();
                let host_config = match session_config_to_host(&config) {
                    Ok(config) => config,
                    Err(message) => {
                        return ControlSignal::invalid_arguments(message, session.span);
                    }
                };
                ControlSignal::pending_session(PendingSession {
                    request: etas_host::SessionRequest {
                        id: request_id,
                        operation: etas_host::SessionOperation::Compact {
                            session: resolved_session,
                            policy: host_config.compaction,
                        },
                        authority: self.host_authority(),
                        trace: self.host_trace(),
                        budget: self.host_budget(),
                    },
                    decode: SessionDecode::ReturnConversation { payload_type },
                    span: session.span,
                    continuation: session.continuation,
                })
            }
            (
                SessionDecode::ReturnMessage { message },
                etas_host::SessionResult::Appended { .. },
            ) => self.apply_continuation(session.continuation, InterpValue::Message(message)),
            (
                SessionDecode::ReturnConversation { payload_type },
                result @ etas_host::SessionResult::History { .. },
            )
            | (
                SessionDecode::ReturnConversation { payload_type },
                result @ etas_host::SessionResult::Compacted { .. },
            ) => match conversation_from_session_result(
                &result,
                payload_type,
                &self.checked.type_store,
            ) {
                Ok(conversation) => self.apply_continuation(
                    session.continuation,
                    InterpValue::Conversation(conversation),
                ),
                Err(error) => ControlSignal::runtime_fault(error, session.span),
            },
            (decode, result) => ControlSignal::runtime_fault(
                format!("unexpected session host result for {decode:?}: {result:?}"),
                session.span,
            ),
        }
    }

    pub(crate) fn record_session_result_event(&mut self, result: &etas_host::SessionResult) {
        match result {
            etas_host::SessionResult::Resolved { session, created } => {
                self.events.push(WorkflowEvent::SessionResolved {
                    session: session.id.clone(),
                    created: *created,
                });
            }
            etas_host::SessionResult::Appended {
                message,
                deduplicated,
            } => {
                self.events.push(WorkflowEvent::SessionMessageAppended {
                    session: message.session.id.clone(),
                    message: message.id.clone(),
                    deduplicated: *deduplicated,
                });
            }
            etas_host::SessionResult::History {
                session,
                messages,
                summary,
                cursor,
            } => {
                self.events.push(WorkflowEvent::SessionHistoryLoaded {
                    session: session.id.clone(),
                    message_count: messages.len(),
                    has_summary: summary.is_some(),
                    cursor: cursor.as_ref().map(|cursor| cursor.opaque.clone()),
                });
            }
            etas_host::SessionResult::Compacted { session, summary } => {
                self.events.push(WorkflowEvent::SessionCompacted {
                    session: session.id.clone(),
                    summary_message_count: summary.message_count,
                });
            }
        }
    }

    pub(crate) fn session_host_error_signal(
        &mut self,
        session: PendingSession,
        error: etas_host::HostError,
    ) -> ControlSignal {
        let message = format!("session host boundary failed: {}", error.message);
        ControlSignal::runtime_fault(message, session.span)
    }
}

fn conversation_from_session_result(
    result: &etas_host::SessionResult,
    payload_type: etas_types::TypeId,
    store: &etas_types::TypeStore,
) -> Result<crate::value::ConversationValue, String> {
    match result {
        etas_host::SessionResult::History {
            session,
            messages,
            summary,
            cursor,
        } => {
            let messages = messages
                .iter()
                .map(|message| message_value_from_session_message(message, payload_type, store))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(crate::value::ConversationValue {
                session: session.id.clone(),
                messages,
                summary: summary.as_ref().map(session_summary_value),
                cursor: cursor.as_ref().map(|cursor| cursor.opaque.clone()),
            })
        }
        etas_host::SessionResult::Compacted { session, summary } => {
            Ok(crate::value::ConversationValue {
                session: session.id.clone(),
                messages: Vec::new(),
                summary: Some(session_summary_value(summary)),
                cursor: None,
            })
        }
        _ => Err(format!(
            "unexpected session host result for conversation: {result:?}"
        )),
    }
}

pub(crate) fn session_config_to_host(
    config: &crate::value::SessionConfigValue,
) -> Result<etas_host::SessionConfig, String> {
    let context = match config.context.as_deref() {
        None => etas_host::ContextPolicy::All,
        Some(value) => context_policy_from_value(value).ok_or_else(|| {
            "SessionConfig.context is not a checked ContextPolicy value".to_owned()
        })?,
    };
    let retention = match config.retention.as_deref() {
        None => etas_host::RetentionPolicy::Forever,
        Some(value) => retention_policy_from_value(value).ok_or_else(|| {
            "SessionConfig.retention is not a checked RetentionPolicy value".to_owned()
        })?,
    };
    let compaction = match config.compaction.as_deref() {
        None => etas_host::CompactionPolicy::None,
        Some(value) => compaction_policy_from_value(value).ok_or_else(|| {
            "SessionConfig.compaction is not a checked CompactionPolicy value".to_owned()
        })?,
    };
    Ok(etas_host::SessionConfig {
        id: config.id.clone(),
        context,
        retention,
        compaction,
    })
}

pub(super) fn session_message_from_message_value(
    message: &crate::value::MessageValue,
) -> Result<etas_host::SessionMessage, String> {
    let envelope = message_envelope_from_message_value(message)?;
    let session = envelope
        .session
        .ok_or_else(|| "session message requires an attached session".to_owned())?;
    Ok(etas_host::SessionMessage {
        id: envelope.id,
        from: envelope.from,
        to: envelope.to,
        role: envelope.role,
        session: session.clone(),
        created_at: envelope.created_at,
        payload: envelope.payload,
        provenance: envelope.provenance,
        dedup_key: Some(format!("{}:{}", session.id, message.id)),
    })
}

pub(super) fn message_envelope_from_message_value(
    message: &crate::value::MessageValue,
) -> Result<etas_host::session::MessageEnvelope, String> {
    Ok(etas_host::session::MessageEnvelope {
        id: message.id.clone(),
        from: message.from.clone(),
        to: message.to.clone(),
        role: session_role_from_value(message.role),
        session: message
            .session
            .as_ref()
            .map(|id| etas_host::SessionRef { id: id.clone() }),
        created_at: message.created_at.clone(),
        payload: interp_to_host_value(&message.payload)?,
        provenance: message.provenance.as_ref().map(provenance_to_host),
    })
}

fn message_value_from_session_message(
    message: &etas_host::SessionMessage,
    payload_type: etas_types::TypeId,
    store: &etas_types::TypeStore,
) -> Result<crate::value::MessageValue, String> {
    Ok(crate::value::MessageValue {
        id: message.id.clone(),
        from: message.from.clone(),
        to: message.to.clone(),
        role: message_role_from_session_role(message.role),
        session: Some(message.session.id.clone()),
        created_at: message.created_at.clone(),
        payload: Box::new(
            host_to_typed_interp_value(message.payload.clone(), payload_type, store).map_err(
                |error| {
                    format!(
                        "session message payload does not match checked Message<T> type: {error}"
                    )
                },
            )?,
        ),
        provenance: match message.provenance.as_ref() {
            Some(value) => Some(provenance_from_host(value)?),
            None => None,
        },
    })
}

fn session_role_from_value(role: crate::value::MessageRoleValue) -> etas_host::SessionMessageRole {
    match role {
        crate::value::MessageRoleValue::System => etas_host::SessionMessageRole::System,
        crate::value::MessageRoleValue::User => etas_host::SessionMessageRole::User,
        crate::value::MessageRoleValue::Assistant => etas_host::SessionMessageRole::Assistant,
        crate::value::MessageRoleValue::Tool => etas_host::SessionMessageRole::Tool,
    }
}

fn message_role_from_session_role(
    role: etas_host::SessionMessageRole,
) -> crate::value::MessageRoleValue {
    match role {
        etas_host::SessionMessageRole::System => crate::value::MessageRoleValue::System,
        etas_host::SessionMessageRole::User => crate::value::MessageRoleValue::User,
        etas_host::SessionMessageRole::Assistant => crate::value::MessageRoleValue::Assistant,
        etas_host::SessionMessageRole::Tool => crate::value::MessageRoleValue::Tool,
    }
}

pub(super) fn provenance_to_host(
    provenance: &crate::value::ProvenanceValue,
) -> etas_host::HostValue {
    etas_host::HostValue::Record(vec![
        (
            "trace_id".to_owned(),
            provenance
                .trace_id
                .clone()
                .map(etas_host::HostValue::String)
                .unwrap_or(etas_host::HostValue::Unit),
        ),
        (
            "source".to_owned(),
            provenance
                .source
                .clone()
                .map(etas_host::HostValue::String)
                .unwrap_or(etas_host::HostValue::Unit),
        ),
    ])
}

pub(super) fn provenance_from_host(
    value: &etas_host::HostValue,
) -> Result<crate::value::ProvenanceValue, String> {
    let etas_host::HostValue::Record(fields) = value else {
        return Err("message provenance must be a host record".to_owned());
    };
    if let Some((name, _)) = fields
        .iter()
        .find(|(name, _)| !matches!(name.as_str(), "trace_id" | "source"))
    {
        return Err(format!(
            "message provenance contains unknown field `{name}`"
        ));
    }
    Ok(crate::value::ProvenanceValue {
        trace_id: host_record_optional_string_field(fields, "trace_id")?,
        source: host_record_optional_string_field(fields, "source")?,
    })
}

fn host_record_optional_string_field(
    fields: &[(String, etas_host::HostValue)],
    name: &str,
) -> Result<Option<String>, String> {
    let value = fields
        .iter()
        .find(|(field, _)| field == name)
        .ok_or_else(|| format!("message provenance is missing `{name}`"))?
        .1
        .clone();
    match value {
        etas_host::HostValue::String(value) => Ok(Some(value)),
        etas_host::HostValue::Unit => Ok(None),
        _ => Err(format!(
            "message provenance field `{name}` must be string or unit"
        )),
    }
}

fn session_summary_value(summary: &etas_host::SessionSummary) -> crate::value::SessionSummaryValue {
    crate::value::SessionSummaryValue {
        text: summary.text.clone(),
        message_count: summary.message_count,
    }
}

fn context_policy_from_value(value: &InterpValue) -> Option<etas_host::ContextPolicy> {
    match value {
        InterpValue::Variant { name, fields } if name == "All" && fields.is_empty() => {
            Some(etas_host::ContextPolicy::All)
        }
        InterpValue::Variant { name, fields } if name == "LastTurns" && fields.len() == 1 => {
            usize_from_value(&fields[0]).map(etas_host::ContextPolicy::LastTurns)
        }
        InterpValue::Variant { name, fields }
            if name == "SummaryPlusRecent" && fields.len() == 1 =>
        {
            usize_from_value(&fields[0])
                .map(|recent| etas_host::ContextPolicy::SummaryPlusRecent { recent })
        }
        _ => None,
    }
}

fn retention_policy_from_value(value: &InterpValue) -> Option<etas_host::RetentionPolicy> {
    match value {
        InterpValue::Variant { name, fields } if name == "Forever" && fields.is_empty() => {
            Some(etas_host::RetentionPolicy::Forever)
        }
        InterpValue::Variant { name, fields } if name == "Days" && fields.len() == 1 => {
            u64_from_value(&fields[0]).map(etas_host::RetentionPolicy::Days)
        }
        _ => None,
    }
}

fn compaction_policy_from_value(value: &InterpValue) -> Option<etas_host::CompactionPolicy> {
    match value {
        InterpValue::Variant { name, fields } if name == "None" && fields.is_empty() => {
            Some(etas_host::CompactionPolicy::None)
        }
        InterpValue::Variant { name, fields } if name == "SummarizeWhen" && fields.len() == 1 => {
            limit_tokens_from_value(&fields[0]).map(|max_context_tokens| {
                etas_host::CompactionPolicy::SummarizeWhen { max_context_tokens }
            })
        }
        _ => None,
    }
}

fn limit_tokens_from_value(value: &InterpValue) -> Option<u64> {
    match value {
        InterpValue::Variant { name, fields }
            if matches!(name.as_str(), "ContextTokens" | "Tokens") && fields.len() == 1 =>
        {
            u64_from_value(&fields[0])
        }
        value => u64_from_value(value),
    }
}

fn usize_from_value(value: &InterpValue) -> Option<usize> {
    u64_from_value(value).and_then(|value| usize::try_from(value).ok())
}

fn u64_from_value(value: &InterpValue) -> Option<u64> {
    match value {
        InterpValue::Number(value) => value.as_u64(),
        _ => None,
    }
}
