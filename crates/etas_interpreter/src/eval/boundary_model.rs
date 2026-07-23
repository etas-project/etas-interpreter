use super::host_value::host_to_typed_interp_value;
use super::*;
use etas_types::{PrimitiveType, Type, TypeId, TypeStore};

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_model_result(&self, model: &PendingModel) -> Option<InterpValue> {
        let key = self.model_boundary_key(model);
        self.completed_host_boundary_result("model", &key)
    }

    pub(crate) fn try_model_result_value(
        &self,
        model: &PendingModel,
        response: ModelResponse,
    ) -> Result<InterpValue, ModelResultError> {
        if !response.tool_calls.is_empty() && !matches!(model.decode, ModelDecode::ModelResponse) {
            return Err(ModelResultError::fatal(
                "model returned unresolved tool calls after the tool-call loop completed",
            ));
        }
        match model.decode {
            ModelDecode::String => {
                let text = response
                    .message
                    .content
                    .into_iter()
                    .filter_map(|content| match content {
                        ModelContent::Text(text) => Some(text),
                        ModelContent::Value(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                Ok(InterpValue::String(text))
            }
            ModelDecode::ModelResponse => Ok(InterpValue::ModelResponse(
                model_response_value_from_host(response),
            )),
            ModelDecode::Typed(expected) => {
                self.typed_model_result_value(model, response, expected)
            }
        }
    }

    pub(crate) fn record_model_result_error(
        &mut self,
        model: &PendingModel,
        error: ModelResultError,
    ) {
        self.diagnostics
            .push(Diagnostic::analysis(error.code, model.span, error.message));
    }

    pub(crate) fn model_boundary_key(&self, model: &PendingModel) -> String {
        let messages = model
            .request
            .messages
            .iter()
            .map(|message| format!("{:?}:{:?}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("|");
        format!(
            "model:{}:{messages}:response_schema={:?}",
            model.request.model.0, model.request.response_schema
        )
    }

    fn typed_model_result_value(
        &self,
        model: &PendingModel,
        response: ModelResponse,
        expected: TypeId,
    ) -> Result<InterpValue, ModelResultError> {
        let expected_name = expected_type_name(expected, &self.checked.type_store);
        let provider = model
            .request
            .provider
            .as_ref()
            .map(|provider| provider.0.as_str())
            .unwrap_or("unspecified-provider");
        let model_name = model.request.model.0.as_str();
        let mut structured = None;
        let mut text_parts = Vec::new();
        for content in response.message.content {
            match content {
                ModelContent::Text(text) => text_parts.push(text),
                ModelContent::Value(value) => {
                    if structured.replace(value).is_some() {
                        return Err(ModelResultError::typed_output(format!(
                            "typed model response from {provider}/{model_name} for {expected_name} contained more than one structured value"
                        )));
                    }
                }
            }
        }
        if let Some(value) = structured {
            if text_parts.iter().any(|text| !text.trim().is_empty()) {
                return Err(ModelResultError::typed_output(format!(
                    "typed model response from {provider}/{model_name} for {expected_name} mixed structured value content with text content"
                )));
            }
            return host_value_to_typed_interp_value(value, expected, &self.checked.type_store)
                .map_err(|error| {
                    ModelResultError::typed_output(format!(
                        "structured model response from {provider}/{model_name} did not match expected output {expected_name}: {error}"
                    ))
                });
        }

        let text = text_parts.join("");
        let preview = text_preview(&text);
        let json = serde_json::from_str::<serde_json::Value>(&text).map_err(|error| {
            ModelResultError::typed_output(format!(
                "typed model response from {provider}/{model_name} for {expected_name} was text and must be valid JSON: {error}; preview: {preview}"
            ))
        })?;
        json_to_typed_interp_value(&json, expected, &self.checked.type_store).ok_or_else(|| {
            ModelResultError::typed_output(format!(
                "JSON text model response from {provider}/{model_name} did not match expected output {expected_name}; preview: {preview}"
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ModelResultError {
    pub(crate) code: AnalysisDiagnosticCode,
    pub(crate) message: String,
    pub(crate) retryable_typed_output: bool,
}

impl ModelResultError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            code: AnalysisDiagnosticCode::UnhandledRuntimeError,
            message: message.into(),
            retryable_typed_output: false,
        }
    }

    fn typed_output(message: impl Into<String>) -> Self {
        Self {
            code: AnalysisDiagnosticCode::InvalidArguments,
            message: message.into(),
            retryable_typed_output: true,
        }
    }
}

fn expected_type_name(expected: TypeId, store: &TypeStore) -> String {
    store
        .get(expected)
        .map(|ty| display_type_name(ty.clone(), store))
        .unwrap_or_else(|| format!("{expected:?}"))
}

fn display_type_name(ty: Type, store: &TypeStore) -> String {
    match ty {
        Type::Primitive(primitive) => format!("{primitive:?}"),
        Type::IntegerLiteral { .. } => "integer literal".to_owned(),
        Type::Array(_) => "Array".to_owned(),
        Type::List(_) => "List".to_owned(),
        Type::Map { .. } => "Map".to_owned(),
        Type::Set(_) => "Set".to_owned(),
        Type::Range { .. } => "Range".to_owned(),
        Type::Slice(_) => "Slice".to_owned(),
        Type::Option(_) => "Option".to_owned(),
        Type::Result { .. } => "Result".to_owned(),
        Type::Record(_) => "Record".to_owned(),
        Type::Tuple(_) => "Tuple".to_owned(),
        Type::Enum(enum_ref) => enum_ref.name,
        Type::Function(_) => "Function".to_owned(),
        Type::Handler(_) => "Handler".to_owned(),
        Type::Named(named) => named.name,
        Type::Nominal(nominal) => {
            let representation = nominal
                .representation
                .and_then(|representation| store.get(representation).cloned())
                .map(|representation| display_type_name(representation, store));
            match representation {
                Some(representation) => format!("{} ({representation})", nominal.name),
                None => nominal.name,
            }
        }
        Type::Applied { .. } => "Applied".to_owned(),
        Type::Refined { base, .. } => store
            .get(base)
            .map(|base| display_type_name(base.clone(), store))
            .unwrap_or_else(|| "Refined".to_owned()),
        Type::Trust { wrapper, inner } => store
            .get(inner)
            .map(|inner| format!("{wrapper:?}[{}]", display_type_name(inner.clone(), store)))
            .unwrap_or_else(|| format!("{wrapper:?}")),
        Type::Schema(_) => "Schema".to_owned(),
        Type::Prompt => "Prompt".to_owned(),
        Type::PromptPart => "PromptPart".to_owned(),
        Type::Message(_) => "Message".to_owned(),
        Type::MemorySelection(_) => "MemorySelection".to_owned(),
        Type::Store { .. } => "Store".to_owned(),
        Type::MemoryPlace(place) => place.segments.join("."),
        Type::MemoryRegion(_) => "MemoryRegion".to_owned(),
        Type::ResourceHandle(resource) => format!("{resource:?}"),
        Type::Var(var) => format!("{var:?}"),
    }
}

fn text_preview(text: &str) -> String {
    const MAX_CHARS: usize = 160;
    let mut preview = text.chars().take(MAX_CHARS).collect::<String>();
    if text.chars().count() > MAX_CHARS {
        preview.push_str("...");
    }
    preview.escape_debug().to_string()
}

fn host_value_to_typed_interp_value(
    value: HostValue,
    expected: TypeId,
    store: &TypeStore,
) -> Result<InterpValue, String> {
    match value {
        HostValue::Json(json) => json_host_value_to_typed_interp_value(&json, expected, store)
            .ok_or_else(|| "structured JSON host value does not match checked type".to_owned()),
        value => host_to_typed_interp_value(value, expected, store),
    }
}

fn json_host_value_to_typed_interp_value(
    value: &etas_host::HostJsonValue,
    expected: TypeId,
    store: &TypeStore,
) -> Option<InterpValue> {
    let json = match value {
        etas_host::HostJsonValue::Null => serde_json::Value::Null,
        etas_host::HostJsonValue::Bool(value) => serde_json::Value::Bool(*value),
        etas_host::HostJsonValue::Number(value) => {
            serde_json::Number::from_f64(*value).map(serde_json::Value::Number)?
        }
        etas_host::HostJsonValue::String(value) => serde_json::Value::String(value.clone()),
        etas_host::HostJsonValue::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(host_json_to_serde_json)
                .collect::<Option<Vec<_>>>()?,
        ),
        etas_host::HostJsonValue::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| Some((key.clone(), host_json_to_serde_json(value)?)))
                .collect::<Option<serde_json::Map<_, _>>>()?,
        ),
    };
    json_to_typed_interp_value(&json, expected, store)
}

fn host_json_to_serde_json(value: &etas_host::HostJsonValue) -> Option<serde_json::Value> {
    Some(match value {
        etas_host::HostJsonValue::Null => serde_json::Value::Null,
        etas_host::HostJsonValue::Bool(value) => serde_json::Value::Bool(*value),
        etas_host::HostJsonValue::Number(value) => {
            serde_json::Value::Number(serde_json::Number::from_f64(*value)?)
        }
        etas_host::HostJsonValue::String(value) => serde_json::Value::String(value.clone()),
        etas_host::HostJsonValue::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(host_json_to_serde_json)
                .collect::<Option<Vec<_>>>()?,
        ),
        etas_host::HostJsonValue::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, value)| Some((key.clone(), host_json_to_serde_json(value)?)))
                .collect::<Option<serde_json::Map<_, _>>>()?,
        ),
    })
}

fn json_to_typed_interp_value(
    value: &serde_json::Value,
    expected: TypeId,
    store: &TypeStore,
) -> Option<InterpValue> {
    json_to_typed_interp_value_with_substitutions(
        value,
        expected,
        store,
        &std::collections::HashMap::new(),
    )
}

fn json_to_typed_interp_value_with_substitutions(
    value: &serde_json::Value,
    expected: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    if let Some(Type::Named(named)) = store.get(expected)
        && let Some(expected) = substitutions.get(&named.name).copied()
    {
        return json_to_typed_interp_value_with_substitutions(
            value,
            expected,
            store,
            substitutions,
        );
    }
    match store.get(expected)? {
        Type::Primitive(primitive) => json_to_primitive(value, *primitive),
        Type::Array(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(ArrayValue::new)
            .map(InterpValue::Array),
        Type::List(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(|values| InterpValue::List(values.into())),
        Type::Slice(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(SliceValue::new)
            .map(InterpValue::Slice),
        Type::Set(elem) => json_array_to_values(value, *elem, store, substitutions)
            .map(|values| InterpValue::Set(values.into())),
        Type::Map { key, value: elem } => json_to_map(value, *key, *elem, store, substitutions),
        Type::Record(record) => {
            let object = value.as_object()?;
            record
                .fields
                .iter()
                .map(|field| {
                    let field_value = object.get(&field.name)?;
                    Some((
                        field.name.clone(),
                        json_to_typed_interp_value_with_substitutions(
                            field_value,
                            field.ty,
                            store,
                            substitutions,
                        )?,
                    ))
                })
                .collect::<Option<Vec<_>>>()
                .map(|fields| InterpValue::Record(fields.into()))
        }
        Type::Nominal(nominal) => {
            let representation = nominal.representation?;
            json_to_typed_interp_value_with_substitutions(
                value,
                representation,
                store,
                substitutions,
            )
            .map(|value| InterpValue::Nominal {
                ty: expected,
                value: Box::new(value),
            })
        }
        Type::Applied { constructor, args } => {
            let Type::Nominal(nominal) = store.get(TypeId(constructor.0))? else {
                return None;
            };
            if nominal.params.len() != args.len() {
                return None;
            }
            let mut applied_substitutions = substitutions.clone();
            applied_substitutions.extend(nominal.params.iter().cloned().zip(args.iter().copied()));
            json_to_typed_interp_value_with_substitutions(
                value,
                nominal.representation?,
                store,
                &applied_substitutions,
            )
            .map(|value| InterpValue::Nominal {
                ty: expected,
                value: Box::new(value),
            })
        }
        Type::Tuple(types) => {
            let values = value.as_array()?;
            if values.len() != types.len() {
                return None;
            }
            values
                .iter()
                .zip(types.iter())
                .map(|(value, ty)| {
                    json_to_typed_interp_value_with_substitutions(value, *ty, store, substitutions)
                })
                .collect::<Option<Vec<_>>>()
                .map(InterpValue::Tuple)
        }
        Type::Option(inner) => {
            if value.is_null() {
                Some(InterpValue::OptionNone)
            } else {
                json_to_typed_interp_value_with_substitutions(value, *inner, store, substitutions)
                    .map(Box::new)
                    .map(InterpValue::OptionSome)
            }
        }
        Type::Result { ok, err } => json_to_result(value, *ok, *err, store, substitutions),
        Type::Enum(_) => json_to_enum(value),
        Type::Trust { wrapper, inner }
            if matches!(wrapper, etas_types::TrustWrapper::Untrusted) =>
        {
            json_to_typed_interp_value_with_substitutions(value, *inner, store, substitutions).map(
                |value| InterpValue::Trust {
                    wrapper: *wrapper,
                    value: Box::new(value),
                },
            )
        }
        Type::Trust { .. } => None,
        Type::Schema(inner) | Type::Message(inner) => {
            json_to_typed_interp_value_with_substitutions(value, *inner, store, substitutions)
        }
        _ => None,
    }
}

fn json_to_primitive(value: &serde_json::Value, primitive: PrimitiveType) -> Option<InterpValue> {
    match primitive {
        PrimitiveType::Bool => value.as_bool().map(InterpValue::Bool),
        PrimitiveType::String => value
            .as_str()
            .map(|value| InterpValue::String(value.to_owned())),
        PrimitiveType::Char => value
            .as_str()
            .and_then(|value| {
                let mut chars = value.chars();
                let ch = chars.next()?;
                chars.next().is_none().then_some(ch)
            })
            .map(|ch| InterpValue::String(ch.to_string())),
        PrimitiveType::Unit => value.is_null().then_some(InterpValue::Unit),
        PrimitiveType::Bytes => value
            .as_str()
            .map(|value| InterpValue::Bytes(value.as_bytes().to_vec())),
        primitive @ (PrimitiveType::I8
        | PrimitiveType::I16
        | PrimitiveType::I32
        | PrimitiveType::I64
        | PrimitiveType::I128
        | PrimitiveType::ISize) => signed_integer(value, primitive),
        primitive @ (PrimitiveType::U8
        | PrimitiveType::U16
        | PrimitiveType::U32
        | PrimitiveType::U64
        | PrimitiveType::U128
        | PrimitiveType::USize) => unsigned_integer(value, primitive),
        primitive @ (PrimitiveType::F32 | PrimitiveType::F64) => value
            .as_f64()
            .and_then(|value| crate::value::NumericValue::from_float(value, primitive))
            .map(InterpValue::Number),
        PrimitiveType::Never => None,
    }
}

fn signed_integer(value: &serde_json::Value, primitive: PrimitiveType) -> Option<InterpValue> {
    let value = value.as_i64()? as i128;
    crate::value::NumericValue::from_signed(value, primitive).map(InterpValue::Number)
}

fn unsigned_integer(value: &serde_json::Value, primitive: PrimitiveType) -> Option<InterpValue> {
    let value = value.as_u64()? as u128;
    crate::value::NumericValue::from_unsigned(value, primitive).map(InterpValue::Number)
}

fn json_array_to_values(
    value: &serde_json::Value,
    elem: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<Vec<InterpValue>> {
    value
        .as_array()?
        .iter()
        .map(|value| {
            json_to_typed_interp_value_with_substitutions(value, elem, store, substitutions)
        })
        .collect()
}

fn json_to_map(
    value: &serde_json::Value,
    key_type: TypeId,
    value_type: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    if let Some(object) = value.as_object() {
        return object
            .iter()
            .map(|(key, value)| {
                Some((
                    json_to_typed_interp_value_with_substitutions(
                        &serde_json::Value::String(key.clone()),
                        key_type,
                        store,
                        substitutions,
                    )?,
                    json_to_typed_interp_value_with_substitutions(
                        value,
                        value_type,
                        store,
                        substitutions,
                    )?,
                ))
            })
            .collect::<Option<Vec<_>>>()
            .map(MapValue::new)
            .map(InterpValue::Map);
    }
    value
        .as_array()?
        .iter()
        .map(|entry| {
            if let Some(pair) = entry.as_array() {
                let [key, value] = pair.as_slice() else {
                    return None;
                };
                return Some((
                    json_to_typed_interp_value_with_substitutions(
                        key,
                        key_type,
                        store,
                        substitutions,
                    )?,
                    json_to_typed_interp_value_with_substitutions(
                        value,
                        value_type,
                        store,
                        substitutions,
                    )?,
                ));
            }
            let object = entry.as_object()?;
            Some((
                json_to_typed_interp_value_with_substitutions(
                    object.get("key")?,
                    key_type,
                    store,
                    substitutions,
                )?,
                json_to_typed_interp_value_with_substitutions(
                    object.get("value")?,
                    value_type,
                    store,
                    substitutions,
                )?,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .map(MapValue::new)
        .map(InterpValue::Map)
}

fn json_to_result(
    value: &serde_json::Value,
    ok: TypeId,
    err: TypeId,
    store: &TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Option<InterpValue> {
    let object = value.as_object()?;
    if let Some(value) = object.get("Ok") {
        return json_to_typed_interp_value_with_substitutions(value, ok, store, substitutions).map(
            |value| InterpValue::Variant {
                name: "Ok".to_owned(),
                fields: vec![value],
            },
        );
    }
    object
        .get("Err")
        .and_then(|value| {
            json_to_typed_interp_value_with_substitutions(value, err, store, substitutions)
        })
        .map(|value| InterpValue::Variant {
            name: "Err".to_owned(),
            fields: vec![value],
        })
}

fn json_to_enum(value: &serde_json::Value) -> Option<InterpValue> {
    if let Some(name) = value.as_str() {
        return Some(InterpValue::Variant {
            name: name.to_owned(),
            fields: Vec::new(),
        });
    }
    let object = value.as_object()?;
    let mut entries = object.iter();
    let (name, fields) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    let fields = match fields {
        serde_json::Value::Array(values) if values.is_empty() => Vec::new(),
        serde_json::Value::Null => Vec::new(),
        _ => return None,
    };
    Some(InterpValue::Variant {
        name: name.clone(),
        fields,
    })
}

fn model_response_value_from_host(response: ModelResponse) -> crate::value::ModelResponseValue {
    crate::value::ModelResponseValue {
        id: response.id.0,
        message: model_message_value_from_host(response.message),
        tool_calls: response
            .tool_calls
            .into_iter()
            .map(|call| crate::value::ModelToolCallValue {
                id: call.id,
                tool: call.tool,
                args: host_support_value_from_host(call.args),
            })
            .collect(),
        usage: response.usage.map(|usage| crate::value::ModelUsageValue {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }),
    }
}

fn model_message_value_from_host(
    message: etas_host::ModelMessage,
) -> crate::value::ModelMessageValue {
    crate::value::ModelMessageValue {
        role: match message.role {
            ModelRole::System => crate::value::ModelRoleValue::System,
            ModelRole::User => crate::value::ModelRoleValue::User,
            ModelRole::Assistant => crate::value::ModelRoleValue::Assistant,
            ModelRole::Tool => crate::value::ModelRoleValue::Tool,
        },
        content: message
            .content
            .into_iter()
            .map(|content| match content {
                ModelContent::Text(text) => crate::value::ModelContentValue::Text(text),
                ModelContent::Value(value) => {
                    crate::value::ModelContentValue::Value(host_support_value_from_host(value))
                }
            })
            .collect(),
    }
}

fn host_support_value_from_host(value: HostValue) -> crate::value::HostSupportValue {
    match value {
        HostValue::Unit => crate::value::HostSupportValue::Unit,
        HostValue::Bool(value) => crate::value::HostSupportValue::Bool(value),
        HostValue::Int(value) => crate::value::HostSupportValue::Int(value.to_string()),
        HostValue::UInt(value) => crate::value::HostSupportValue::UInt(value.to_string()),
        HostValue::Float(value) => crate::value::HostSupportValue::FloatBits(value.to_bits()),
        HostValue::String(value) => crate::value::HostSupportValue::String(value),
        HostValue::Bytes(value) => crate::value::HostSupportValue::Bytes(value),
        HostValue::List(values) => crate::value::HostSupportValue::List(
            values
                .into_iter()
                .map(host_support_value_from_host)
                .collect(),
        ),
        HostValue::Map(entries) => crate::value::HostSupportValue::Map(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        host_support_value_from_host(key),
                        host_support_value_from_host(value),
                    )
                })
                .collect(),
        ),
        HostValue::Record(fields) => crate::value::HostSupportValue::Record(
            fields
                .into_iter()
                .map(|(name, value)| (name, host_support_value_from_host(value)))
                .collect(),
        ),
        HostValue::Variant { name, fields } => crate::value::HostSupportValue::Variant {
            name,
            fields: fields
                .into_iter()
                .map(host_support_value_from_host)
                .collect(),
        },
        HostValue::Json(value) => {
            crate::value::HostSupportValue::Json(host_json_support_value_from_host(value))
        }
    }
}

fn host_json_support_value_from_host(
    value: etas_host::HostJsonValue,
) -> crate::value::HostJsonSupportValue {
    match value {
        etas_host::HostJsonValue::Null => crate::value::HostJsonSupportValue::Null,
        etas_host::HostJsonValue::Bool(value) => crate::value::HostJsonSupportValue::Bool(value),
        etas_host::HostJsonValue::Number(value) => {
            crate::value::HostJsonSupportValue::NumberBits(value.to_bits())
        }
        etas_host::HostJsonValue::String(value) => {
            crate::value::HostJsonSupportValue::String(value)
        }
        etas_host::HostJsonValue::Array(values) => crate::value::HostJsonSupportValue::Array(
            values
                .into_iter()
                .map(host_json_support_value_from_host)
                .collect(),
        ),
        etas_host::HostJsonValue::Object(entries) => crate::value::HostJsonSupportValue::Object(
            entries
                .into_iter()
                .map(|(name, value)| (name, host_json_support_value_from_host(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{host_json_to_serde_json, json_to_typed_interp_value};
    use crate::value::{InterpValue, NumericValue};
    use etas_types::{
        EnumTypeRef, FieldType, NominalTypeRef, PrimitiveType, RecordType, Type, TypeConstructorId,
        TypeStore,
    };

    #[test]
    fn nested_non_finite_host_json_fails_closed() {
        let array =
            etas_host::HostJsonValue::Array(vec![etas_host::HostJsonValue::Number(f64::NAN)]);
        assert!(host_json_to_serde_json(&array).is_none());

        let object = etas_host::HostJsonValue::Object(vec![(
            "bad".to_owned(),
            etas_host::HostJsonValue::Number(f64::INFINITY),
        )]);
        assert!(host_json_to_serde_json(&object).is_none());
    }

    #[test]
    fn typed_json_preserves_applied_nominal_identity_and_substitutes_fields() {
        let mut store = TypeStore::new();
        let i64_ty = store.intern(Type::Primitive(PrimitiveType::I64));
        let parameter = store.intern(Type::Named(etas_types::NamedTypeRef {
            name: "T".to_owned(),
        }));
        let representation = store.intern(Type::Record(RecordType {
            fields: vec![FieldType {
                name: "value".to_owned(),
                ty: parameter,
            }],
        }));
        let nominal = store.intern(Type::Nominal(NominalTypeRef {
            name: "Box".to_owned(),
            params: vec!["T".to_owned()],
            representation: Some(representation),
        }));
        let applied = store.intern(Type::Applied {
            constructor: TypeConstructorId(nominal.0),
            args: vec![i64_ty],
        });

        let decoded =
            json_to_typed_interp_value(&serde_json::json!({ "value": 42 }), applied, &store);

        let Some(InterpValue::Nominal { ty, value }) = decoded else {
            panic!("expected applied nominal value");
        };
        assert_eq!(ty, applied);
        let InterpValue::Record(fields) = *value else {
            panic!("expected nominal record representation");
        };
        assert_eq!(
            fields.snapshot(),
            vec![(
                "value".to_owned(),
                InterpValue::Number(NumericValue::I64(42)),
            )]
        );
    }

    #[test]
    fn typed_json_rejects_enum_payload_without_checked_variant_field_types() {
        let mut store = TypeStore::new();
        let enum_ty = store.intern(Type::Enum(EnumTypeRef {
            name: "Outcome".to_owned(),
        }));

        assert!(
            json_to_typed_interp_value(&serde_json::json!({ "Value": [42] }), enum_ty, &store)
                .is_none()
        );
        assert_eq!(
            json_to_typed_interp_value(&serde_json::json!("Empty"), enum_ty, &store),
            Some(InterpValue::Variant {
                name: "Empty".to_owned(),
                fields: Vec::new(),
            })
        );
    }
}
