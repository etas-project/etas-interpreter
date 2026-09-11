use super::*;
use crate::control::ExecutionFault;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_field(
        &mut self,
        expr: HirExprId,
        base: HirExprId,
        field: &str,
        span: Span,
        frame: &mut Frame,
    ) -> ControlSignal {
        match self.eval_expr(base, frame) {
            ControlSignal::Value(value) => {
                self.eval_field_value_signal(Some(expr), value, field, span)
            }
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Checkpoint(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => compose_signal_continuation(
                signal,
                Continuation::FieldReceiver {
                    expr,
                    field: field.to_owned(),
                    span,
                    frame: frame.clone(),
                },
            ),
            ControlSignal::Return(value) => ControlSignal::Return(value),
            ControlSignal::Resume(value) => ControlSignal::Resume(value),
            ControlSignal::Finish(value) => ControlSignal::Finish(value),
            ControlSignal::Break => ControlSignal::Break,
            ControlSignal::Fault(fault) => ControlSignal::Fault(fault),
            ControlSignal::Cancelled(cause) => ControlSignal::Cancelled(cause),
            ControlSignal::Continue => ControlSignal::Continue,
        }
    }

    fn memory_store_types_for_expr(
        &self,
        expr: Option<HirExprId>,
        span: Span,
    ) -> Result<(etas_types::TypeId, etas_types::TypeId), ExecutionFault> {
        let Some(expr) = expr else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory store field is missing a checked expression id for host value decoding",
            ));
        };
        let Some(ty) = self.checked.types.expr_types.get(&expr).copied() else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory store field is missing its checked Store[K, V] type",
            ));
        };
        match self.checked.type_store.get(ty) {
            Some(etas_types::Type::Store { key, value }) => Ok((*key, *value)),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory store field must be checked as Store[K, V]",
            )),
        }
    }

    fn memory_store_types_for_resource_field(
        &self,
        resource_ty: etas_types::TypeId,
        field: &str,
        span: Span,
    ) -> Result<(etas_types::TypeId, etas_types::TypeId), ExecutionFault> {
        let schema = match self.checked.type_store.get(resource_ty) {
            Some(etas_types::Type::ResourceHandle(
                etas_types::ResourceHandleType::MemoryRegion { schema },
            ))
            | Some(etas_types::Type::MemoryRegion(schema)) => *schema,
            _ => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    "resource handle field access requires a checked MemoryRegion schema",
                ));
            }
        };
        let schema = match self.checked.type_store.get(schema) {
            Some(etas_types::Type::MemoryRegion(inner)) => *inner,
            _ => schema,
        };
        let Some(etas_types::Type::Record(record)) = self.checked.type_store.get(schema) else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory-region schema must be a checked record type",
            ));
        };
        let Some(field_ty) = record
            .fields
            .iter()
            .find(|candidate| candidate.name == field)
            .map(|candidate| candidate.ty)
        else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("memory-region schema does not contain store field `{field}`"),
            ));
        };
        match self.checked.type_store.get(field_ty) {
            Some(etas_types::Type::Store { key, value }) => Ok((*key, *value)),
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "memory-region schema field must be checked as Store[K, V]",
            )),
        }
    }

    pub(super) fn eval_const_field_value(
        &self,
        expr: Option<HirExprId>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        self.try_eval_field_value(expr, base, field, span)
    }

    pub(super) fn eval_field_value_signal(
        &mut self,
        expr: Option<HirExprId>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> ControlSignal {
        match self.try_eval_field_value(expr, base, field, span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    fn try_eval_field_value(
        &self,
        expr: Option<HirExprId>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match base {
            InterpValue::Nominal { value, .. } => {
                self.try_eval_field_value(expr, *value, field, span)
            }
            InterpValue::Record(fields) => fields
                .snapshot()
                .into_iter()
                .find_map(|(name, value)| (name == field).then_some(value))
                .ok_or_else(|| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!("record field `{field}` does not exist at runtime"),
                    )
                }),
            InterpValue::ResourceHandle { stable_id, ty, .. } => {
                let (key_type, value_type) =
                    self.memory_store_types_for_resource_field(ty, field, span)?;
                Ok(InterpValue::MemoryStore {
                    region_stable_id: stable_id,
                    path: vec![field.to_owned()],
                    key_type,
                    value_type,
                })
            }
            InterpValue::MemoryStore {
                region_stable_id,
                mut path,
                ..
            } => {
                let (key_type, value_type) = self.memory_store_types_for_expr(expr, span)?;
                path.push(field.to_owned());
                Ok(InterpValue::MemoryStore {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                })
            }
            InterpValue::Message(message) => match field {
                "body" | "content" => Ok(*message.payload),
                "id" => Ok(InterpValue::String(message.id)),
                "from" => Ok(option_string(message.from)),
                "to" => Ok(option_string(message.to)),
                "role" => Ok(InterpValue::String(
                    message_role_name(message.role).to_owned(),
                )),
                "session" => Ok(option_string(message.session)),
                "created_at" => Ok(InterpValue::String(message.created_at)),
                "provenance" => Ok(message
                    .provenance
                    .map(|provenance| {
                        InterpValue::OptionSome(Box::new(InterpValue::Provenance(provenance)))
                    })
                    .unwrap_or(InterpValue::OptionNone)),
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("message field `{field}` does not exist at runtime"),
                )),
            },
            InterpValue::Conversation(conversation) => match field {
                "session" => Ok(InterpValue::String(conversation.session)),
                "messages" => Ok(InterpValue::Array(ArrayValue::new(
                    conversation
                        .messages
                        .into_iter()
                        .map(InterpValue::Message)
                        .collect(),
                ))),
                "summary" => {
                    let Some(context) = conversation.selected_context else {
                        return Ok(InterpValue::OptionNone);
                    };
                    let result_type = super::resolve_std_type(
                        self.checked,
                        &["std", "agent", "session", "SessionPublishedContext"],
                    )
                    .ok_or_else(|| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            "conversation summary lacks checked field type",
                        )
                    })?;
                    let value =
                        etas_host::session::published_context_value(&context, &self.storage_limits)
                            .map_err(|e| {
                                ExecutionFault::new(
                                    AnalysisDiagnosticCode::InvalidArguments,
                                    span,
                                    e.message,
                                )
                            })?;
                    super::host_value::host_to_checked_interp_value(
                        value,
                        result_type,
                        self.checked,
                        &self.storage_limits,
                    )
                    .map(|value| InterpValue::OptionSome(Box::new(value)))
                    .map_err(|error| {
                        ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, error)
                    })
                }
                "cursor" => Ok(option_string(conversation.cursor)),
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("conversation field `{field}` does not exist at runtime"),
                )),
            },
            InterpValue::Variant { name, fields }
                if name == "SessionConfig.continue_or_new" && fields.len() == 1 =>
            {
                match field {
                    "id" => Ok(InterpValue::String(format!(
                        "continue_or_new:{}",
                        stable_session_config_key(&fields[0])
                    ))),
                    _ => Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::InvalidArguments,
                        span,
                        format!(
                            "session config field `{field}` is not materialized by continue_or_new"
                        ),
                    )),
                }
            }
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "field access expects a record, resource handle, memory store, or message receiver",
            )),
        }
    }
}

fn option_string(value: Option<String>) -> InterpValue {
    value
        .map(|value| InterpValue::OptionSome(Box::new(InterpValue::String(value))))
        .unwrap_or(InterpValue::OptionNone)
}

fn stable_session_config_key(value: &InterpValue) -> String {
    match value {
        InterpValue::String(value) => value.clone(),
        InterpValue::Number(value) => value.display_value(),
        InterpValue::Bool(value) => value.to_string(),
        other => format!("{other:?}"),
    }
}

fn message_role_name(role: crate::value::MessageRoleValue) -> &'static str {
    match role {
        crate::value::MessageRoleValue::System => "system",
        crate::value::MessageRoleValue::User => "user",
        crate::value::MessageRoleValue::Assistant => "assistant",
        crate::value::MessageRoleValue::Tool => "tool",
    }
}
