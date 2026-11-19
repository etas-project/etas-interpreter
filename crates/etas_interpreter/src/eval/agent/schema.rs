use super::*;
use crate::eval::{boundary_model, host_value};
use crate::value::source_schema;
use etas_types::facts::inference::CheckedAgentInferenceFact;

impl EvalContext<'_> {
    pub(super) fn step_response_schema(
        &self,
        schema: &InterpValue,
        output: TypeId,
        span: Span,
    ) -> Result<Option<HostSchema>, ExecutionFault> {
        use crate::value::conversation::protocol::field;

        if matches!(schema, InterpValue::Schema(_)) {
            return if self.type_is_string(output) {
                Ok(None)
            } else {
                self.host_schema_for_type(output, span, 0).map(Some)
            };
        }

        let Ok(definition @ InterpValue::Json(_)) = field(schema, "definition") else {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source Schema has no checked JsonValue definition",
            ));
        };
        let invalid =
            |reason| ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, reason);
        let definition = host_value::interp_to_bounded_host_value(
            definition,
            &mut self.storage_limits.value_budget(),
        )
        .and_then(|definition| etas_host::value::host_value_to_json(&definition))
        .map_err(|error| invalid(error.message))?;
        if self.type_is_string(output)
            && definition == serde_json::json!({ "type": "string" })
            && matches!(
                field(schema, "decoder").and_then(|decoder| field(decoder, "decode")),
                Ok(InterpValue::Callable(
                    crate::control::CallTarget::DerivedResponseDecoder(_)
                ))
            )
        {
            return Ok(None);
        }
        if !(definition.is_object() || definition.is_boolean()) {
            return Err(invalid(
                "JSON Schema definition must be an object or boolean".into(),
            ));
        }
        Ok(Some(HostSchema::JsonDefinition(definition)))
    }

    pub(in crate::eval) fn inference_schema_value(
        &self,
        fact: &CheckedAgentInferenceFact,
    ) -> Result<InterpValue, String> {
        if matches!(self.checked.type_store.get(fact.schema),Some(Type::Schema(ty)) if *ty == fact.output)
        {
            return Ok(InterpValue::Schema(fact.output));
        }
        let (decoder_type, callable) = source_schema::layout(self.checked, fact.schema)?;
        let signature = source_schema::decoder_signature(self.checked, callable)?;
        if signature.output != fact.output {
            return Err("derived decoder disagrees with the checked inference output".into());
        }
        let span = self.checked.hir.exprs[fact.prompt].span(&self.checked.hir.blocks);
        let host = self
            .host_schema_for_type(fact.output, span, 0)
            .map_err(|error| error.message)?;
        let json =
            etas_host::model::host_schema_to_json_schema(&host).map_err(|error| error.message)?;
        let definition =
            host_value::host_json_support_value_from_serde(json).map_err(|error| error.message)?;
        let decode =
            InterpValue::Callable(crate::control::CallTarget::DerivedResponseDecoder(callable));
        let decoder = nominal_record(decoder_type, vec![("decode".into(), decode)]);
        Ok(nominal_record(
            fact.schema,
            vec![
                ("definition".into(), InterpValue::Json(definition)),
                ("decoder".into(), decoder),
            ],
        ))
    }

    pub(in crate::eval) fn decode_source_model_response(
        &self,
        callable: TypeId,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        let [response] = args.as_slice() else {
            return ControlSignal::invalid_arguments(
                "model response decoder requires one response",
                span,
            );
        };
        let layout = match source_schema::decoder_signature(self.checked, callable) {
            Ok(layout) => layout,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let decoded = host_value::interp_to_host_value(response)
            .and_then(|response| self.decode_response_content(response, layout.output));
        let (name, value) = match decoded {
            Ok(value) => ("Ok", value),
            Err(reason) => {
                let error = nominal_record(
                    layout.error,
                    vec![
                        ("path".into(), InterpValue::Array(Vec::new().into())),
                        (
                            "expected".into(),
                            InterpValue::String(
                                etas_types::ty::display_type(
                                    &self.checked.type_store,
                                    layout.output,
                                )
                                .into(),
                            ),
                        ),
                        ("actual".into(), InterpValue::String(reason.into())),
                    ],
                );
                ("Err", error)
            }
        };
        self.checked_result_value(layout.signature.output, name, value, span)
    }

    fn decode_response_content(
        &self,
        response: HostValue,
        output: TypeId,
    ) -> Result<InterpValue, String> {
        let result = take_field(response, "result")?;
        let output_record = match result {
            HostValue::Variant { name, fields } if name == "Ok" && fields.len() == 1 => {
                fields.into_iter().next().unwrap()
            }
            HostValue::Variant { name, fields } if name == "Err" && fields.len() == 1 => {
                return Err(
                    match take_field(fields.into_iter().next().unwrap(), "message")? {
                        HostValue::String(message) => message,
                        _ => "model request failed".into(),
                    },
                );
            }
            _ => return Err("model response has no result".into()),
        };
        let HostValue::Record(mut fields) = output_record else {
            return Err("model output requires a record".into());
        };
        if !fields.iter().any(|(key, v)| {
            key == "termination" && matches!(v,HostValue::Variant{name, ..} if name=="Complete")
        }) {
            return Err("model response did not complete".into());
        }
        let message = take_named(&mut fields, "message")?;
        let HostValue::List(content) = take_field(message, "content")? else {
            return Err("model response has no content array".into());
        };
        let mut text = String::new();
        let count = content.len();
        for item in content {
            let HostValue::Variant { name, fields } = item else {
                return Err("invalid model response content".into());
            };
            let [item]: [_; 1] = fields
                .try_into()
                .map_err(|_| "model content requires one payload")?;
            match (name.as_str(), item) {
                ("Text", HostValue::String(part)) => text.push_str(&part),
                ("Value", data) if count == 1 => {
                    let data = crate::eval::data_capture::restore_data(
                        data,
                        0,
                        &mut self.storage_limits.value_budget(),
                    )?;
                    return boundary_model::host_value_to_typed_interp_value(
                        data,
                        output,
                        self.checked,
                    );
                }
                _ => return Err("model response mixes incompatible content".into()),
            };
        }
        if self.type_is_string(output) {
            return Ok(InterpValue::String(text.into()));
        }
        let parsed = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        boundary_model::json_to_typed_interp_value_with_layouts(
            &parsed,
            output,
            &self.checked.type_store,
            &self.checked.types.enum_layouts,
        )
        .ok_or_else(|| "model response does not match the checked schema".into())
    }
}

fn nominal_record(ty: TypeId, fields: Vec<(String, InterpValue)>) -> InterpValue {
    InterpValue::Nominal {
        ty,
        value: InterpValue::Record(fields.into()).into(),
    }
}

fn take_named(fields: &mut Vec<(String, HostValue)>, key: &str) -> Result<HostValue, String> {
    let index = fields
        .iter()
        .position(|(name, _)| name == key)
        .ok_or_else(|| format!("model response lacks {key}"))?;
    Ok(fields.swap_remove(index).1)
}

fn take_field(value: HostValue, key: &str) -> Result<HostValue, String> {
    let HostValue::Record(mut fields) = value else {
        return Err(format!("model response lacks {key}"));
    };
    take_named(&mut fields, key)
}
