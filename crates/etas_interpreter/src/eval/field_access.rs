use super::*;
use crate::control::ExecutionFault;
use crate::plan::FieldAccessSite;

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
                self.eval_field_value_signal(FieldAccessSite::Expr(expr), value, field, span)
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
        site: FieldAccessSite<'_>,
        span: Span,
    ) -> Result<(etas_types::TypeId, etas_types::TypeId), ExecutionFault> {
        let Some(ty) = self.plan.records.field_type(self.checked, site) else {
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
        site: FieldAccessSite<'_>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        self.try_eval_field_value(site, base, field, span)
    }

    pub(super) fn eval_field_value_signal(
        &mut self,
        site: FieldAccessSite<'_>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> ControlSignal {
        match base {
            InterpValue::SyntaxBlock(syntax) => {
                return self.eval_syntax_text_projection(site, syntax, field, span);
            }
            InterpValue::Nominal { value, .. } => {
                return self.eval_field_value_signal(site, value.into_value(), field, span);
            }
            _ => {}
        }
        let code_projection = matches!(
            &base,
            InterpValue::AgentDefinition(_) | InterpValue::CallableDefinition(_)
        );
        if code_projection && let Some(signal) = self.code_operation_boundary(span) {
            return signal;
        }
        let result = match self.try_eval_field_value(site, base, field, span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        };
        if code_projection {
            self.code_operation_boundary(span).unwrap_or(result)
        } else {
            result
        }
    }

    fn try_eval_field_value(
        &self,
        site: FieldAccessSite<'_>,
        base: InterpValue,
        field: &str,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        match base {
            InterpValue::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => {
                let projection = self.plan.records.native_field(site).ok_or_else(|| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "command result field has no checked canonical runtime projection",
                    )
                })?;
                if projection.name != field {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "native field name differs from its checked runtime projection",
                    ));
                }
                Ok(match projection.role {
                    etas_std::RuntimeFieldRole::CommandExitCode => InterpValue::i32(exit_code),
                    etas_std::RuntimeFieldRole::CommandStdout => InterpValue::Bytes(stdout),
                    etas_std::RuntimeFieldRole::CommandStderr => InterpValue::Bytes(stderr),
                })
            }
            InterpValue::AgentDefinition(definition) if field != "body" => {
                let output = self.code_definition_field_type(
                    site,
                    definition.ty(),
                    definition.program(),
                    field,
                    span,
                )?;
                code::agent_field(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types.code,
                    output,
                    &definition,
                    field,
                )
                .map_err(|reason| {
                    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
                })
            }
            InterpValue::AgentDefinition(definition) => self.code_definition_body_projection(
                site,
                DefinitionBodyProjection {
                    ty: definition.ty(),
                    program: definition.program(),
                    syntax: definition.backend().syntax_operations(),
                    body: definition.body().map_err(|error| {
                        ExecutionFault::new(
                            AnalysisDiagnosticCode::MissingCheckedFact,
                            span,
                            error.to_string(),
                        )
                    })?,
                },
                field,
                span,
            ),
            InterpValue::CallableDefinition(definition) if field != "body" => {
                let output = self.code_definition_field_type(
                    site,
                    definition.ty(),
                    definition.program(),
                    field,
                    span,
                )?;
                code::callable_field(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types.code,
                    output,
                    &definition,
                    field,
                )
                .map_err(|reason| {
                    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, reason)
                })
            }
            InterpValue::CallableDefinition(definition) => self.code_definition_body_projection(
                site,
                DefinitionBodyProjection {
                    ty: definition.ty(),
                    program: definition.program(),
                    syntax: definition.syntax_operations(),
                    body: definition.body().cloned(),
                },
                field,
                span,
            ),
            InterpValue::SyntaxBlock(_) => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "syntax text requires a controlled runtime projection, not constant evaluation",
            )),
            InterpValue::Nominal { value, .. } => {
                self.try_eval_field_value(site, value.into_value(), field, span)
            }
            InterpValue::Record(fields) => {
                let layout = self.plan.records.field(site).ok_or_else(|| {
                    ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "record field expression has no checked slot layout",
                    )
                })?;
                if layout.name() != field {
                    return Err(ExecutionFault::new(
                        AnalysisDiagnosticCode::MissingCheckedFact,
                        span,
                        "record field differs from checked projection",
                    ));
                }
                layout.borrow(&fields).cloned().map_err(|message| {
                    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
                })
            }
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
                let (key_type, value_type) = self.memory_store_types_for_expr(site, span)?;
                path.push(field.to_owned());
                Ok(InterpValue::MemoryStore {
                    region_stable_id,
                    path,
                    key_type,
                    value_type,
                })
            }
            InterpValue::Message(message) => match field {
                "body" | "content" => Ok(message.payload.into_value()),
                "id" => Ok(InterpValue::String(message.id.into())),
                "from" => Ok(option_string(message.from)),
                "to" => Ok(option_string(message.to)),
                "role" => Ok(InterpValue::String(
                    message_role_name(message.role).to_owned().into(),
                )),
                "session" => message
                    .session
                    .map(|id| {
                        self.session_id_value(id, span)
                            .map(|value| InterpValue::OptionSome(value.into()))
                    })
                    .transpose()
                    .map(|value| value.unwrap_or(InterpValue::OptionNone)),
                "created_at" => Ok(InterpValue::String(message.created_at.into())),
                "provenance" => Ok(message
                    .provenance
                    .map(|provenance| {
                        InterpValue::OptionSome(crate::value::SharedValue::new(
                            InterpValue::Provenance(provenance),
                        ))
                    })
                    .unwrap_or(InterpValue::OptionNone)),
                _ => Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    format!("message field `{field}` does not exist at runtime"),
                )),
            },
            InterpValue::Conversation(conversation) => match field {
                "session" => self.session_id_value(conversation.session, span),
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
                    .map(|value| InterpValue::OptionSome(crate::value::SharedValue::new(value)))
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
            _ => Err(ExecutionFault::new(
                AnalysisDiagnosticCode::InvalidArguments,
                span,
                "field access expects a record, resource handle, memory store, or message receiver",
            )),
        }
    }

    fn code_definition_body_projection(
        &self,
        site: FieldAccessSite<'_>,
        definition: DefinitionBodyProjection<'_>,
        field: &str,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if field != "body" {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "unsupported checked definition field projection",
            ));
        }
        let output =
            self.code_definition_field_type(site, definition.ty, definition.program, field, span)?;
        let incomplete = || {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "definition body projection has no checked SyntaxBlock result",
            )
        };
        let Some(etas_types::Type::Option(inner)) = self.checked.type_store.get(output) else {
            return Err(incomplete());
        };
        if self.checked.types.known_std_types.code.syntax_block != Some(*inner) {
            return Err(incomplete());
        }
        match definition.body {
            Some(body) => Ok(InterpValue::OptionSome(
                InterpValue::SyntaxBlock(crate::value::SyntaxBlockValue::new(
                    *inner,
                    body,
                    definition.syntax.clone(),
                ))
                .into(),
            )),
            None => Ok(InterpValue::OptionNone),
        }
    }

    fn code_definition_field_type(
        &self,
        site: FieldAccessSite<'_>,
        receiver_ty: etas_types::TypeId,
        program: &etas_frontend::CheckedProgram,
        field: &str,
        span: Span,
    ) -> Result<etas_types::TypeId, ExecutionFault> {
        let incomplete = || {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "definition projection differs from its checked registry field and program",
            )
        };
        if !std::sync::Arc::ptr_eq(program, self.checked)
            || self
                .checked
                .types
                .known_std_types
                .projected_field_templates(&self.checked.type_store, receiver_ty)
                .is_none_or(|fields| !fields.iter().any(|candidate| candidate.name == field))
        {
            return Err(incomplete());
        }
        self.plan
            .records
            .field_type(self.checked, site)
            .ok_or_else(incomplete)
    }
}

struct DefinitionBodyProjection<'a> {
    ty: etas_types::TypeId,
    program: &'a etas_frontend::CheckedProgram,
    syntax: &'a std::sync::Arc<etas_frontend::code::service::NativeCodeSyntaxOperations>,
    body: Option<etas_code::syntax::SyntaxBlockRef>,
}

fn option_string(value: Option<String>) -> InterpValue {
    value
        .map(|value| {
            InterpValue::OptionSome(crate::value::SharedValue::new(InterpValue::String(
                value.into(),
            )))
        })
        .unwrap_or(InterpValue::OptionNone)
}

fn message_role_name(role: crate::value::MessageRoleValue) -> &'static str {
    match role {
        crate::value::MessageRoleValue::System => "system",
        crate::value::MessageRoleValue::User => "user",
        crate::value::MessageRoleValue::Assistant => "assistant",
        crate::value::MessageRoleValue::Tool => "tool",
    }
}
