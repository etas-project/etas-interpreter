use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_message_type_method_values(
        &mut self,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if method == "with_session" {
            let [message, session] = args else {
                return abort_message_method(
                    span,
                    "Message.with_session expects exactly two arguments",
                );
            };
            let InterpValue::Message(mut message) = message.clone() else {
                return abort_message_method(
                    span,
                    "Message.with_session expects a Message value as its first argument",
                );
            };
            let Some(session_config) = session_config_from_value(session) else {
                return abort_message_method(
                    span,
                    "Message.with_session expects a SessionConfig value as its second argument",
                );
            };
            message.session = Some(session_config.id.clone());
            let host_config = match super::boundary_session::session_config_to_host(&session_config)
            {
                Ok(config) => config,
                Err(message) => return ControlSignal::invalid_arguments(message, span),
            };
            self.events.push(
                crate::orchestration::WorkflowEvent::MessageSessionAttached {
                    id: message.id.clone(),
                    session: message.session.clone().unwrap_or_default(),
                    session_config: session_config.clone(),
                },
            );
            let request_id = self.next_host_request_id();
            return ControlSignal::pending_session(PendingSession {
                request: etas_host::SessionRequest {
                    id: request_id,
                    operation: etas_host::SessionOperation::Resolve {
                        config: host_config,
                    },
                    authority: self.host_authority(),
                    trace: self.host_trace(),
                    budget: self.host_budget(),
                },
                decode: SessionDecode::ResolveThenAppendMessage { message },
                span,
                continuation: Continuation::BlockValue,
            });
        }
        if method != "new" {
            return unsupported_method(span, "Message type", method);
        }
        let [payload] = args else {
            return abort_message_method(span, "Message.new expects exactly one argument");
        };
        let id = format!("msg-{}", self.next_message);
        self.next_message += 1;
        let created_at = match current_message_timestamp() {
            Ok(created_at) => created_at,
            Err(message) => {
                return ControlSignal::runtime_fault(message, span);
            }
        };
        let message = crate::value::MessageValue {
            id,
            from: None,
            to: None,
            role: crate::value::MessageRoleValue::User,
            session: self.current_session.clone(),
            created_at,
            payload: Box::new(payload.clone()),
            provenance: Some(crate::value::ProvenanceValue {
                trace_id: Some(format!("{:?}", self.host_context.trace.trace_id)),
                source: Some("Message.new".to_owned()),
            }),
        };
        self.events
            .push(crate::orchestration::WorkflowEvent::MessageCreated {
                id: message.id.clone(),
                from: message.from.clone(),
                to: message.to.clone(),
                session: message.session.clone(),
                role: crate::value::codec::message_role_json(message.role).to_owned(),
                created_at: message.created_at.clone(),
                payload: message.payload.clone(),
                provenance: message.provenance.clone(),
            });
        ControlSignal::Value(InterpValue::Message(message))
    }

    pub(in crate::eval) fn eval_message_value_method(
        &mut self,
        message: crate::value::MessageValue,
        method: &str,
        type_args: &[etas_hir::HirTypeId],
        args: &[HirArg],
        span: Span,
    ) -> ControlSignal {
        if method != "cast" {
            return unsupported_method(span, "Message value", method);
        }
        if !args.is_empty() {
            return abort_message_method(span, "Message.cast expects no value arguments");
        }
        if type_args.len() != 1 {
            return abort_message_method(
                span,
                "Message.cast requires one checked target type argument",
            );
        }
        self.eval_checked_message_cast(message, type_args, span)
    }

    pub(in crate::eval) fn eval_checked_message_cast(
        &mut self,
        mut message: crate::value::MessageValue,
        type_args: &[etas_hir::HirTypeId],
        span: Span,
    ) -> ControlSignal {
        let Some(target) = type_args
            .first()
            .and_then(|ty| self.checked.types.type_refs.get(ty))
            .copied()
        else {
            return ControlSignal::missing_checked_fact(
                "Message.cast requires checked target type facts",
                span,
            );
        };
        let Ok(host_payload) = super::host_value::interp_to_host_value(&message.payload) else {
            return ControlSignal::Value(InterpValue::OptionNone);
        };
        let Ok(payload) = super::host_value::host_to_typed_interp_value(
            host_payload,
            target,
            &self.checked.type_store,
        ) else {
            return ControlSignal::Value(InterpValue::OptionNone);
        };
        message.payload = Box::new(payload);
        ControlSignal::Value(InterpValue::OptionSome(Box::new(InterpValue::Message(
            message,
        ))))
    }

    pub(in crate::eval) fn eval_session_config_type_method_values(
        &mut self,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if method != "continue_or_new" {
            return unsupported_method(span, "SessionConfig type", method);
        }
        let [key] = args else {
            return abort_message_method(
                span,
                "SessionConfig.continue_or_new expects exactly one argument",
            );
        };
        ControlSignal::Value(InterpValue::Variant {
            name: "SessionConfig.continue_or_new".to_owned(),
            fields: vec![key.clone()],
        })
    }

    pub(in crate::eval) fn eval_conversation_type_method_values(
        &mut self,
        expr: HirExprId,
        method: &str,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if !matches!(method, "load" | "compact") {
            return unsupported_method(span, "Conversation type", method);
        }
        let [session] = args else {
            return ControlSignal::invalid_arguments(
                format!("Conversation.{method} expects exactly one argument"),
                span,
            );
        };
        let Some(session_config) = session_config_from_value(session) else {
            let message = format!("Conversation.{method} expects a SessionConfig value");
            return ControlSignal::invalid_arguments(message, span);
        };
        let request_id = self.next_host_request_id();
        let host_config = match super::boundary_session::session_config_to_host(&session_config) {
            Ok(config) => config,
            Err(message) => return ControlSignal::invalid_arguments(message, span),
        };
        let Some(payload_type) = conversation_payload_type(self.checked, expr) else {
            let checked_type = self
                .checked
                .types
                .expr_types
                .get(&expr)
                .map(|ty| etas_types::ty::display_type(&self.checked.type_store, *ty))
                .unwrap_or_else(|| "<missing>".to_owned());
            return ControlSignal::missing_checked_fact(
                format!(
                    "Conversation operation requires a checked Message<T> payload type; checked result type is `{checked_type}`"
                ),
                span,
            );
        };
        let decode = match method {
            "load" => SessionDecode::ResolveThenLoadConversation {
                config: session_config,
                payload_type,
            },
            "compact" => SessionDecode::ResolveThenCompactConversation {
                config: session_config,
                payload_type,
            },
            _ => unreachable!("conversation method was checked above"),
        };
        ControlSignal::pending_session(PendingSession {
            request: etas_host::SessionRequest {
                id: request_id,
                operation: etas_host::SessionOperation::Resolve {
                    config: host_config,
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode,
            span,
            continuation: Continuation::BlockValue,
        })
    }
}

fn conversation_payload_type(
    checked: &etas_frontend::CheckedProject,
    expr: HirExprId,
) -> Option<etas_types::TypeId> {
    let conversation = *checked.types.expr_types.get(&expr)?;
    let fields = etas_types::record_fields_with_applied_params(&checked.type_store, conversation)?;
    let messages = fields
        .fields
        .iter()
        .find(|field| field.name == "messages")?;
    let etas_types::Type::Array(message) = checked.type_store.get(messages.ty)? else {
        return None;
    };
    let etas_types::Type::Message(payload) = checked.type_store.get(*message)? else {
        return None;
    };
    Some(*payload)
}
