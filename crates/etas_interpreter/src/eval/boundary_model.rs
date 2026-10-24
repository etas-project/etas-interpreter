use super::host_value::host_to_typed_interp_value;
use super::*;
use etas_types::{PrimitiveType, Type, TypeId, TypeStore};
mod json;
use json::{
    from_host as json_host_value_to_typed_interp_value, from_serde as json_to_typed_interp_value,
};

#[cfg(test)]
mod projection_tests;

impl<'a> EvalContext<'a> {
    pub(crate) fn replayed_model_result(&self, model: &PendingModel) -> Option<InterpValue> {
        let key = self.model_boundary_key(model);
        self.completed_host_boundary_result(
            &crate::orchestration::BoundaryOccurrenceId::HostRequest(model.request.id),
            "model",
            &key,
        )
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
                Ok(InterpValue::String(text.into()))
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
                args: call.args.into(),
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
                ModelContent::Value(value) => crate::value::ModelContentValue::Value(value.into()),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::{json_host_value_to_typed_interp_value, json_to_typed_interp_value};
    use crate::value::{InterpValue, NumericValue};
    use etas_types::{
        EnumTypeRef, FieldType, NominalTypeRef, PrimitiveType, RecordType, Type, TypeConstructorId,
        TypeStore,
    };

    #[test]
    fn nested_non_finite_host_json_fails_closed() {
        let mut store = TypeStore::new();
        let float = store.intern(Type::Primitive(PrimitiveType::F64));
        let array_ty = store.intern(Type::Array(float));
        let empty_record = store.intern(Type::Record(RecordType { fields: vec![] }));
        let array =
            etas_host::HostJsonValue::Array(vec![etas_host::HostJsonValue::Number(f64::NAN)]);
        assert!(json_host_value_to_typed_interp_value(&array, array_ty, &store).is_none());

        let object = etas_host::HostJsonValue::Object(vec![(
            "bad".to_owned(),
            etas_host::HostJsonValue::Number(f64::INFINITY),
        )]);
        assert!(json_host_value_to_typed_interp_value(&object, empty_record, &store).is_none());
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
        let InterpValue::Record(ref fields) = *value else {
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
                name: "Empty".to_owned().into(),
                fields: Vec::new().into(),
            })
        );
    }
}
