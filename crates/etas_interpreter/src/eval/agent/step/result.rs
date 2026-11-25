use super::*;
use crate::eval::tool::{ToolInvocationError, ToolRejection};
use etas_host::{ModelOutput, ModelTermination, ModelUsage};

impl EvalContext<'_> {
    pub(in crate::eval) fn step_result_value(
        &mut self,
        model: &PendingModel,
        response: ModelOutput,
        usage: Option<ModelUsage>,
        provider_data: Option<InterpValue>,
    ) -> Result<InterpValue, ExecutionFault> {
        let missing = |reason: String| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                model.span,
                reason,
            )
        };
        let ModelDecode::Step { result, output } = model.decode else {
            return Err(missing(
                "step response requires its checked result ABI".into(),
            ));
        };
        let content = response
            .message
            .content
            .iter()
            .map(|item| match item {
                ModelContent::Text(text) => variant("Text", vec![HostValue::String(text.clone())]),
                ModelContent::Value(value) => variant("Value", vec![value.clone()]),
            })
            .collect();
        let turn = vec![
            (
                "request_id".into(),
                option(response.provider_request_id.clone().map(HostValue::String)),
            ),
            ("content".into(), HostValue::List(content)),
            (
                "provider_data".into(),
                HostValue::Map(
                    response
                        .message
                        .provider_data
                        .iter()
                        .map(|(key, value)| (HostValue::String(key.clone()), value.clone()))
                        .collect(),
                ),
            ),
        ];
        let mut member_calls = std::collections::BTreeMap::new();
        let decision = match &response.termination {
            ModelTermination::Truncated => stopped("Truncated", Vec::new()),
            ModelTermination::Refused(reason) => {
                stopped("Refused", vec![HostValue::String(reason.clone())])
            }
            ModelTermination::InvalidToolCall(reason) => stopped(
                "InvalidToolCall",
                vec![diagnostic("invalid_tool_call", reason.clone())],
            ),
            ModelTermination::InvalidOutput(reason) => stopped(
                "InvalidOutput",
                vec![diagnostic("invalid_output", reason.clone())],
            ),
            ModelTermination::Complete if !response.message.tool_calls.is_empty() => {
                match self.step_tool_calls(model, &response.message.tool_calls, &mut member_calls) {
                    Ok(calls) => variant("CallTools", vec![HostValue::List(calls)]),
                    Err(ToolInvocationError::Rejected(error)) => stopped(
                        "InvalidToolCall",
                        vec![diagnostic(error.code, error.message)],
                    ),
                    Err(ToolInvocationError::Fault(fault)) => return Err(fault),
                }
            }
            ModelTermination::Complete => {
                let value =
                    if self.type_is_string(output) && model.request.response_schema.is_none() {
                        let mut text = String::new();
                        let mut valid = true;
                        for content in &response.message.content {
                            match content {
                                ModelContent::Text(value) => text.push_str(value),
                                ModelContent::Value(_) => valid = false,
                            }
                        }
                        if valid {
                            Ok(InterpValue::String(text.into()))
                        } else {
                            Err("string output contains structured content".into())
                        }
                    } else {
                        self.typed_model_result_value(model, response, output)
                            .map_err(|error| error.message)
                    };
                match value {
                    Ok(value) => variant(
                        "Final",
                        vec![interp_to_host_value(&value).map_err(missing)?],
                    ),
                    Err(reason) => {
                        stopped("InvalidOutput", vec![diagnostic("invalid_output", reason)])
                    }
                }
            }
        };
        let mut fields = vec![
            ("decision".into(), decision),
            ("usage".into(), option(usage.map(usage_value))),
        ];
        let mut value = match provider_data {
            Some(map) => {
                let turn_ty =
                    checked_record_field(self.checked, result, "turn").map_err(missing)?;
                let turn =
                    decode_record_with_field(self.checked, turn_ty, turn, "provider_data", map)
                        .map_err(missing)?;
                decode_record_with_field(self.checked, result, fields, "turn", turn)
            }
            None => {
                fields.push(("turn".into(), HostValue::Record(turn)));
                crate::eval::host_value::decode_checked_host_value(
                    HostValue::Record(fields),
                    result,
                    self.checked,
                )
            }
        }
        .map_err(missing)?;
        // The request task stays suspended between Model.step and response decoding.
        if let Some(turn) = &mut self.task.agent_turn
            && let Some(state) = &turn.state
            && !member_calls.is_empty()
        {
            let snapshot = std::rc::Rc::new(crate::value::MemberToolSnapshot {
                instance: turn.reference,
                request: self.task.logical_identity,
                state: state.clone(),
                calls: member_calls,
            });
            bind_member_tool_calls(&mut value, &snapshot);
            turn.tool_snapshots
                .retain(|snapshot| std::rc::Rc::weak_count(snapshot) > 0);
            turn.tool_snapshots.push(snapshot);
        }
        Ok(value)
    }

    pub(in crate::eval) fn step_provider_data_builder(
        &self,
        result: etas_types::TypeId,
        data: &[(String, HostValue)],
    ) -> Result<Option<(CallTarget, InterpValue)>, String> {
        let turn = checked_record_field(self.checked, result, "turn")?;
        let map = checked_record_field(self.checked, turn, "provider_data")?;
        if matches!(self.checked.type_store.get(map), Some(Type::Map { .. })) {
            return Ok(None);
        }
        let mut types =
            etas_types::TypeInterner::from_store(self.checked.type_store.as_ref().clone());
        let layout = etas_types::storage_layout::materialize_storage_shapes(
            &mut types,
            &self.checked.types,
            map,
            etas_code::descriptor::DescriptorLimits::default().max_nodes,
        )
        .map_err(|error| error.to_string())?;
        let etas_types::storage_layout::StorageKind::Collection {
            kind: etas_types::storage_layout::StorageCollection::Map,
            restore,
            fallible_restore: false,
            ..
        } = &layout[0].kind
        else {
            return Err("model provider data requires a checked Map builder".into());
        };
        let json = self
            .checked
            .types
            .known_std_types
            .json_value
            .ok_or("provider data requires checked JsonValue")?;
        let entries = data
            .iter()
            .map(|(name, value)| {
                let value = crate::eval::host_value::decode_checked_host_value(
                    value.clone(),
                    json,
                    self.checked,
                )?;
                Ok(InterpValue::Tuple(
                    vec![InterpValue::String(name.clone().into()), value].into(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let target = self.storage_adapter_target(restore)?;
        Ok(Some((target, InterpValue::Array(entries.into()))))
    }

    fn step_tool_calls(
        &mut self,
        model: &PendingModel,
        calls: &[etas_host::ModelToolCall],
        member_calls: &mut std::collections::BTreeMap<String, SymbolId>,
    ) -> Result<Vec<HostValue>, ToolInvocationError> {
        let missing = |message: &str| {
            ToolInvocationError::Fault(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                model.span,
                message,
            ))
        };
        let rejected = |message: String| {
            ToolInvocationError::Rejected(ToolRejection {
                code: "invalid_tool_call",
                message,
            })
        };
        let mut seen = std::collections::BTreeSet::new();
        let ModelDecode::Step { result, .. } = model.decode else {
            return Err(missing("tool requests require a checked step result"));
        };
        let Some(Type::Applied { args, .. }) = self.checked.type_store.get(result) else {
            return Err(missing("step has no checked context ABI"));
        };
        let context = self
            .task
            .agent_context
            .as_ref()
            .ok_or_else(|| missing("step has no active method context"))?;
        if !args
            .first()
            .is_some_and(|ty| self.agent_context_matches(*ty, context.method))
        {
            return Err(missing("step has no checked method context"));
        }
        let method = self
            .checked
            .types
            .agent_methods
            .get(&context.method)
            .ok_or_else(|| missing("step method is missing"))?;
        let tools = self
            .agent_tool_surface(method.agent, method.body)
            .map_err(ToolInvocationError::Fault)?;
        calls
            .iter()
            .map(|call| {
                if call.id.is_empty() || !seen.insert(&call.id) {
                    return Err(rejected("tool call IDs must be nonempty and unique".into()));
                }
                let binding = tools
                    .iter()
                    .find(|tool| tool.schema.tool.name == call.tool)
                    .ok_or_else(|| {
                        rejected(format!(
                            "tool `{}` is not a registered native tool in this context",
                            call.tool
                        ))
                    })?;
                self.storage_limits
                    .result_value_size(&call.args)
                    .map_err(|error| rejected(error.message))?;
                crate::eval::tool::schema::validate_host_value_schema(
                    &call.args,
                    &binding.schema.input,
                    "args",
                )
                .map_err(rejected)?;
                let (id, revision) = self
                    .canonical_tool_identity(binding.symbol)
                    .map_err(|message| missing(&message))?;
                if binding
                    .source
                    .as_ref()
                    .is_some_and(|source| self.checked.hir.member_tool_owner(source.item).is_some())
                {
                    member_calls.insert(call.id.clone(), binding.symbol);
                }
                Ok(HostValue::Record(vec![
                    ("id".into(), HostValue::String(call.id.clone())),
                    (
                        "tool".into(),
                        HostValue::Record(vec![
                            ("id".into(), HostValue::String(id)),
                            ("revision".into(), HostValue::String(revision)),
                        ]),
                    ),
                    ("args".into(), call.args.clone()),
                ]))
            })
            .collect()
    }
}

fn checked_record_field(
    checked: &etas_frontend::CheckedProject,
    ty: etas_types::TypeId,
    name: &str,
) -> Result<etas_types::TypeId, String> {
    etas_types::record_fields_with_applied_params(&checked.type_store, ty)
        .map_err(|error| error.to_string())?
        .and_then(|record| record.fields.into_iter().find(|field| field.name == name))
        .map(|field| field.ty)
        .ok_or_else(|| format!("model response has no checked field `{name}`"))
}

fn decode_record_with_field(
    checked: &etas_frontend::CheckedProject,
    ty: etas_types::TypeId,
    fields: Vec<(String, HostValue)>,
    prepared_name: &str,
    prepared_value: InterpValue,
) -> Result<InterpValue, String> {
    let layout = etas_types::record_fields_with_applied_params(&checked.type_store, ty)
        .map_err(|error| error.to_string())?
        .ok_or("model response requires a checked record layout")?;
    let mut fields: std::collections::BTreeMap<_, _> = fields.into_iter().collect();
    let mut values = Vec::with_capacity(layout.fields.len());
    let mut prepared_value = Some(prepared_value);
    for field in layout.fields {
        let value = if field.name == prepared_name {
            prepared_value.take().expect("one prepared record field")
        } else {
            let value = fields
                .remove(&field.name)
                .expect("checked model response field");
            crate::eval::host_value::decode_checked_host_value(value, field.ty, checked)?
        };
        values.push((field.name, value));
    }
    let record = InterpValue::Record(crate::value::RecordValue::new(values));
    Ok(match checked.type_store.get(ty) {
        Some(Type::Record(_)) => record,
        _ => InterpValue::Nominal {
            ty,
            value: record.into(),
        },
    })
}

fn bind_member_tool_calls(
    value: &mut InterpValue,
    snapshot: &std::rc::Rc<crate::value::MemberToolSnapshot>,
) {
    fn record(value: &mut InterpValue) -> Option<&mut crate::value::RecordValue> {
        match value {
            InterpValue::Nominal { value, .. } => record(value.make_mut()),
            InterpValue::Record(record) => Some(record),
            _ => None,
        }
    }
    let Some(decision) = record(value).and_then(|result| result.field_mut("decision")) else {
        return;
    };
    let decision = match decision {
        InterpValue::Nominal { value, .. } => value.make_mut(),
        decision => decision,
    };
    let InterpValue::Variant { name, fields } = decision else {
        return;
    };
    if name.as_str() != "CallTools" {
        return;
    }
    let mut values = std::mem::take(fields).into_values();
    if let [InterpValue::Array(calls)] = values.as_mut_slice() {
        for call in calls.borrow_mut() {
            if !matches!(
                crate::value::conversation::protocol::field(call, "id"),
                Ok(InterpValue::String(id)) if snapshot.calls.contains_key(id.as_str())
            ) {
                continue;
            }
            if let Some(tool) = record(call)
                .and_then(|call| call.field_mut("tool"))
                .and_then(record)
            {
                tool.bind_member_tool(std::rc::Rc::downgrade(snapshot));
            }
        }
    }
    *fields = values.into();
}

fn stopped(name: &str, fields: Vec<HostValue>) -> HostValue {
    variant("Stopped", vec![variant(name, fields)])
}

fn diagnostic(code: &str, message: String) -> HostValue {
    HostValue::Record(vec![
        ("code".into(), HostValue::String(code.into())),
        ("message".into(), HostValue::String(message)),
        ("path".into(), option(None)),
    ])
}

fn usage_value(usage: ModelUsage) -> HostValue {
    HostValue::Record(vec![
        (
            "input_tokens".into(),
            option(
                usage
                    .input_tokens
                    .map(|value| HostValue::UInt(value.into())),
            ),
        ),
        (
            "output_tokens".into(),
            option(
                usage
                    .output_tokens
                    .map(|value| HostValue::UInt(value.into())),
            ),
        ),
        (
            "cost".into(),
            option(usage.cost.map(|cost| {
                HostValue::Record(vec![
                    ("micros".into(), HostValue::UInt(cost.micros)),
                    ("currency".into(), HostValue::String(cost.currency)),
                ])
            })),
        ),
    ])
}
