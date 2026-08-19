use super::*;
use crate::value::{ListValue, MapValue, RecordValue, SetValue};
use etas_types::{PrimitiveType, Type, TypeId};

pub(super) fn boundary_key_fragment(value: &InterpValue) -> String {
    match value {
        InterpValue::Unit => "unit".to_owned(),
        InterpValue::Bool(value) => format!("bool:{value}"),
        InterpValue::Number(value) => format!(
            "number:{}:{}",
            value.primitive().source_name(),
            value.display_value()
        ),
        InterpValue::String(value) => format!("string:{value:?}"),
        InterpValue::Bytes(value) => format!("bytes:{value:?}"),
        InterpValue::Json(value) => format!("json:{}", host_json_support_value_key(value)),
        InterpValue::Nominal { ty, value } => {
            format!("nominal:{}:{}", ty.0, boundary_key_fragment(value))
        }
        InterpValue::Trust { wrapper, value } => {
            format!("trust:{wrapper}:{}", boundary_key_fragment(value))
        }
        InterpValue::Array(values) => format!(
            "array:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Tuple(values) => format!(
            "tuple:({})",
            values
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::List(values) => format!(
            "list:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Slice(values) => format!(
            "slice:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Map(entries) => format!(
            "map:{{{}}}",
            entries
                .borrow()
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    boundary_key_fragment(key),
                    boundary_key_fragment(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Set(values) => format!(
            "set:#{{{}}}",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Deque(values) => format!(
            "deque:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Queue(values) => format!(
            "queue:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Stack(values) => format!(
            "stack:[{}]",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::PriorityQueue(entries) => format!(
            "priority_queue:[{}]",
            entries
                .borrow()
                .iter()
                .map(|(priority, value)| format!(
                    "{}:{}",
                    boundary_key_fragment(priority),
                    boundary_key_fragment(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::OrderedMap(entries) => format!(
            "ordered_map:{{{}}}",
            entries
                .borrow()
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    boundary_key_fragment(key),
                    boundary_key_fragment(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::OrderedSet(values) => format!(
            "ordered_set:#{{{}}}",
            values
                .borrow()
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Range(range) => format!(
            "range:{:?}:{}:{}",
            range.bounds,
            boundary_key_fragment(&range.start),
            boundary_key_fragment(&range.end)
        ),
        InterpValue::Record(fields) => format!(
            "record:{{{}}}",
            fields
                .borrow()
                .iter()
                .map(|(name, value)| format!("{name}:{}", boundary_key_fragment(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Variant { name, fields } => format!(
            "variant:{name}:({})",
            fields
                .iter()
                .map(boundary_key_fragment)
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::OptionNone => "option:none".to_owned(),
        InterpValue::OptionSome(value) => format!("option:some:{}", boundary_key_fragment(value)),
        InterpValue::Prompt(messages) => format!(
            "prompt:[{}]",
            messages
                .iter()
                .map(|message| {
                    format!("{:?}:{:?}:{:?}", message.role, message.trust, message.text)
                })
                .collect::<Vec<_>>()
                .join(",")
        ),
        InterpValue::Message(message) => format!(
            "message:{}:{:?}",
            boundary_key_fragment(&message.payload),
            message.provenance
        ),
        InterpValue::Conversation(conversation) => format!(
            "conversation:{}:[{}]:{:?}:{:?}",
            conversation.session,
            conversation
                .messages
                .iter()
                .map(|message| boundary_key_fragment(&InterpValue::Message(message.clone())))
                .collect::<Vec<_>>()
                .join(","),
            conversation.summary,
            conversation.cursor,
        ),
        InterpValue::Provenance(provenance) => format!(
            "provenance:{}:{}",
            provenance.trace_id.as_deref().unwrap_or("none"),
            provenance.source.as_deref().unwrap_or("none")
        ),
        InterpValue::ModelResponse(response) => {
            format!(
                "model_response:{}:{}",
                response.id,
                model_message_key(&response.message)
            )
        }
        InterpValue::Command {
            argv,
            env,
            cwd,
            stdin,
        } => format!("command:argv={argv:?}:env={env:?}:cwd={cwd:?}:stdin={stdin:?}"),
        InterpValue::CommandResult {
            exit_code,
            stdout,
            stderr,
        } => format!("command_result:{exit_code}:{stdout:?}:{stderr:?}"),
        InterpValue::Callable(target) => format!("callable:{target:?}"),
        InterpValue::Handler {
            fact_expr,
            handlers,
        } => format!("handler:{}:{}", fact_expr.0, handlers.len()),
        InterpValue::HostHandle(handle) => format!("host_handle:{}", handle.boundary_key()),
        InterpValue::ResourceHandle { stable_id, .. } => format!("resource:{stable_id}"),
        InterpValue::MemoryStore {
            region_stable_id,
            path,
            ..
        } => format!("store:{region_stable_id}:{}", path.join(".")),
        InterpValue::MemorySelection {
            region_stable_id,
            path,
            kind,
            predicate,
            limit,
            ..
        } => format!(
            "selection:{region_stable_id}:{}:{kind:?}:{}:{}",
            path.join("."),
            predicate
                .as_ref()
                .map(|value| boundary_key_fragment(value))
                .unwrap_or_else(|| "none".to_owned()),
            limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| "none".to_owned())
        ),
    }
}

pub(crate) fn interp_to_host_value(value: &InterpValue) -> Result<HostValue, String> {
    match value {
        InterpValue::Unit => Ok(HostValue::Unit),
        InterpValue::Bool(value) => Ok(HostValue::Bool(*value)),
        InterpValue::Number(value) => numeric_to_host_value(*value)
            .ok_or_else(|| "non-finite numeric value is not host-encodable".to_owned()),
        InterpValue::String(value) => Ok(HostValue::String(value.clone())),
        InterpValue::Bytes(value) => Ok(HostValue::Bytes(value.clone())),
        InterpValue::Json(value) => Ok(HostValue::Json(host_json_support_value_to_host(value))),
        InterpValue::Nominal { value, .. } => interp_to_host_value(value),
        InterpValue::Trust { value, .. } => interp_to_host_value(value),
        InterpValue::Tuple(values) => values
            .iter()
            .map(interp_to_host_value)
            .collect::<Result<Vec<_>, _>>()
            .map(HostValue::List),
        InterpValue::Array(values) => values
            .borrow()
            .iter()
            .map(interp_to_host_value)
            .collect::<Result<Vec<_>, _>>()
            .map(HostValue::List),
        InterpValue::List(values) => values
            .borrow()
            .iter()
            .map(interp_to_host_value)
            .collect::<Result<Vec<_>, _>>()
            .map(HostValue::List),
        InterpValue::Deque(values) | InterpValue::Queue(values) | InterpValue::Stack(values) => {
            values
                .borrow()
                .iter()
                .map(interp_to_host_value)
                .collect::<Result<Vec<_>, _>>()
                .map(HostValue::List)
        }
        InterpValue::Map(entries) => entries
            .borrow()
            .iter()
            .map(|(key, value)| Ok((interp_to_host_value(key)?, interp_to_host_value(value)?)))
            .collect::<Result<Vec<_>, String>>()
            .map(HostValue::Map),
        InterpValue::OrderedMap(entries) | InterpValue::PriorityQueue(entries) => entries
            .borrow()
            .iter()
            .map(|(key, value)| Ok((interp_to_host_value(key)?, interp_to_host_value(value)?)))
            .collect::<Result<Vec<_>, String>>()
            .map(HostValue::Map),
        InterpValue::Slice(_)
        | InterpValue::Set(_)
        | InterpValue::OrderedSet(_)
        | InterpValue::Range(_) => Err(format!(
            "{} is not host-encodable",
            interp_value_kind(value)
        )),
        InterpValue::Record(fields) => fields
            .borrow()
            .iter()
            .map(|(name, value)| Ok((name.clone(), interp_to_host_value(value)?)))
            .collect::<Result<Vec<_>, String>>()
            .map(HostValue::Record),
        InterpValue::Variant { name, fields } => fields
            .iter()
            .map(interp_to_host_value)
            .collect::<Result<Vec<_>, _>>()
            .map(|fields| HostValue::Variant {
                name: name.clone(),
                fields,
            }),
        InterpValue::OptionNone => Ok(HostValue::Variant {
            name: "None".to_owned(),
            fields: Vec::new(),
        }),
        InterpValue::OptionSome(value) => Ok(HostValue::Variant {
            name: "Some".to_owned(),
            fields: vec![interp_to_host_value(value)?],
        }),
        InterpValue::Message(message) => {
            super::boundary_session::message_envelope_from_message_value(message)
                .map(|message| etas_host::session::message_envelope_to_host_value(&message))
        }
        InterpValue::Conversation(conversation) => {
            let messages = conversation
                .messages
                .iter()
                .map(|message| interp_to_host_value(&InterpValue::Message(message.clone())))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HostValue::Record(vec![
                (
                    "session".to_owned(),
                    HostValue::String(conversation.session.clone()),
                ),
                ("messages".to_owned(), HostValue::List(messages)),
                (
                    "summary".to_owned(),
                    conversation
                        .summary
                        .as_ref()
                        .map(|summary| {
                            HostValue::Record(vec![
                                ("text".to_owned(), HostValue::String(summary.text.clone())),
                                (
                                    "message_count".to_owned(),
                                    HostValue::UInt(summary.message_count as u128),
                                ),
                            ])
                        })
                        .unwrap_or(HostValue::Unit),
                ),
                (
                    "cursor".to_owned(),
                    conversation
                        .cursor
                        .as_ref()
                        .map(|cursor| HostValue::String(cursor.clone()))
                        .unwrap_or(HostValue::Unit),
                ),
            ]))
        }
        InterpValue::Prompt(_) | InterpValue::Provenance(_) | InterpValue::ModelResponse(_) => Err(
            format!("{} is not host-encodable", interp_value_kind(value)),
        ),
        InterpValue::Command { .. }
        | InterpValue::CommandResult { .. }
        | InterpValue::Callable(_)
        | InterpValue::Handler { .. }
        | InterpValue::HostHandle(_)
        | InterpValue::ResourceHandle { .. }
        | InterpValue::MemoryStore { .. }
        | InterpValue::MemorySelection { .. } => Err(format!(
            "{} is not host-encodable",
            interp_value_kind(value)
        )),
    }
}

fn interp_value_kind(value: &InterpValue) -> &'static str {
    match value {
        InterpValue::Slice(_) => "slice",
        InterpValue::Set(_) => "set",
        InterpValue::OrderedSet(_) => "ordered set",
        InterpValue::Range(_) => "range",
        InterpValue::Prompt(_) => "prompt",
        InterpValue::Provenance(_) => "provenance",
        InterpValue::ModelResponse(_) => "model response",
        InterpValue::Command { .. } => "command",
        InterpValue::CommandResult { .. } => "command result",
        InterpValue::Callable(_) => "callable",
        InterpValue::Handler { .. } => "handler",
        InterpValue::HostHandle(_) => "host handle",
        InterpValue::ResourceHandle { .. } => "resource handle",
        InterpValue::MemoryStore { .. } => "memory store",
        InterpValue::MemorySelection { .. } => "memory selection",
        _ => "interpreter value",
    }
}

fn host_value_kind(value: &HostValue) -> &'static str {
    match value {
        HostValue::Unit => "unit",
        HostValue::Bool(_) => "bool",
        HostValue::Int(_) => "int",
        HostValue::UInt(_) => "uint",
        HostValue::Float(_) => "float",
        HostValue::String(_) => "string",
        HostValue::Bytes(_) => "bytes",
        HostValue::List(_) => "list",
        HostValue::Map(_) => "map",
        HostValue::Record(_) => "record",
        HostValue::Variant { .. } => "variant",
        HostValue::Json(_) => "json",
    }
}

pub(crate) fn host_to_typed_interp_value(
    value: HostValue,
    expected: TypeId,
    store: &etas_types::TypeStore,
) -> Result<InterpValue, String> {
    host_to_typed_interp_value_with_substitutions(
        value,
        expected,
        store,
        &std::collections::HashMap::new(),
    )
}

fn host_to_typed_interp_value_with_substitutions(
    value: HostValue,
    expected: TypeId,
    store: &etas_types::TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Result<InterpValue, String> {
    if let Some(Type::Named(named)) = store.get(expected)
        && let Some(expected) = substitutions.get(&named.name).copied()
    {
        return host_to_typed_interp_value_with_substitutions(
            value,
            expected,
            store,
            substitutions,
        );
    }
    if matches!(
        store.get(expected),
        Some(Type::Named(named)) if named.name == "std.json.JsonValue"
    ) {
        return host_value_to_json_interp_value(value).map_err(|error| error.message);
    }
    let value = match value {
        HostValue::Json(value) => host_json_to_plain_host_value(value)?,
        value => value,
    };
    let expected_type = store
        .get(expected)
        .ok_or_else(|| format!("checked host decode type {expected:?} is missing"))?;
    match expected_type {
        Type::Primitive(primitive) => decode_primitive_host_value(value, *primitive),
        Type::IntegerLiteral { .. } => match value {
            HostValue::Int(value) => {
                crate::value::NumericValue::from_signed(value, PrimitiveType::I32)
                    .map(InterpValue::Number)
                    .ok_or_else(|| "host integer does not fit the checked literal type".to_owned())
            }
            HostValue::UInt(value) => i128::try_from(value)
                .ok()
                .and_then(|value| {
                    crate::value::NumericValue::from_signed(value, PrimitiveType::I32)
                })
                .map(InterpValue::Number)
                .ok_or_else(|| "host unsigned integer does not fit i32 literal type".to_owned()),
            value => Err(format!(
                "expected integer host value, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Array(inner) => decode_host_list(value, *inner, store, substitutions)
            .map(ArrayValue::new)
            .map(InterpValue::Array),
        Type::List(inner) => decode_host_list(value, *inner, store, substitutions)
            .map(ListValue::new)
            .map(InterpValue::List),
        Type::Slice(inner) => decode_host_list(value, *inner, store, substitutions)
            .map(SliceValue::new)
            .map(InterpValue::Slice),
        Type::Set(inner) => decode_host_list(value, *inner, store, substitutions)
            .map(SetValue::new)
            .map(InterpValue::Set),
        Type::Map {
            key: key_type,
            value: value_type,
        } => match value {
            HostValue::Map(entries) => entries
                .into_iter()
                .enumerate()
                .map(|(index, (key, value))| {
                    Ok((
                        host_to_typed_interp_value_with_substitutions(
                            key,
                            *key_type,
                            store,
                            substitutions,
                        )
                        .map_err(|error| format!("map key {index}: {error}"))?,
                        host_to_typed_interp_value_with_substitutions(
                            value,
                            *value_type,
                            store,
                            substitutions,
                        )
                        .map_err(|error| format!("map value {index}: {error}"))?,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()
                .map(MapValue::new)
                .map(InterpValue::Map),
            value => Err(format!(
                "expected map host value, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Tuple(elems) => match value {
            HostValue::List(values) if values.len() == elems.len() => values
                .into_iter()
                .zip(elems.iter().copied())
                .enumerate()
                .map(|(index, (value, ty))| {
                    host_to_typed_interp_value_with_substitutions(value, ty, store, substitutions)
                        .map_err(|error| format!("tuple element {index}: {error}"))
                })
                .collect::<Result<Vec<_>, String>>()
                .map(InterpValue::Tuple),
            HostValue::List(values) => Err(format!(
                "host tuple has {} elements but checked tuple requires {}",
                values.len(),
                elems.len()
            )),
            value => Err(format!(
                "expected tuple host list, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Record(record) => match value {
            HostValue::Record(fields) => {
                if let Some((name, _)) = fields
                    .iter()
                    .find(|(name, _)| !record.fields.iter().any(|field| field.name == *name))
                {
                    return Err(format!("host record contains unknown field `{name}`"));
                }
                record
                    .fields
                    .iter()
                    .map(|field| {
                        let (_, value) = fields
                            .iter()
                            .find(|(name, _)| name == &field.name)
                            .ok_or_else(|| {
                                format!("host record is missing field `{}`", field.name)
                            })?;
                        Ok((
                            field.name.clone(),
                            host_to_typed_interp_value_with_substitutions(
                                value.clone(),
                                field.ty,
                                store,
                                substitutions,
                            )
                            .map_err(|error| format!("record field `{}`: {error}", field.name))?,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()
                    .map(RecordValue::new)
                    .map(InterpValue::Record)
            }
            value => Err(format!(
                "expected record host value, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Nominal(nominal) => {
            let representation = nominal.representation.ok_or_else(|| {
                format!(
                    "nominal type `{}` has no checked representation",
                    nominal.name
                )
            })?;
            host_to_typed_interp_value_with_substitutions(
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
            let Some(Type::Nominal(nominal)) = store.get(TypeId(constructor.0)) else {
                return Err(
                    "applied host decode constructor is not a checked nominal type".to_owned(),
                );
            };
            if nominal.params.len() != args.len() {
                return Err(format!(
                    "applied nominal type `{}` expects {} arguments but has {}",
                    nominal.name,
                    nominal.params.len(),
                    args.len()
                ));
            }
            let mut applied_substitutions = substitutions.clone();
            applied_substitutions.extend(nominal.params.iter().cloned().zip(args.iter().copied()));
            let representation = nominal.representation.ok_or_else(|| {
                format!(
                    "applied nominal type `{}` has no checked representation",
                    nominal.name
                )
            })?;
            host_to_typed_interp_value_with_substitutions(
                value,
                representation,
                store,
                &applied_substitutions,
            )
            .map(|value| InterpValue::Nominal {
                ty: expected,
                value: Box::new(value),
            })
        }
        Type::Option(inner) => match value {
            HostValue::Variant { name, fields } if name == "None" && fields.is_empty() => {
                Ok(InterpValue::OptionNone)
            }
            HostValue::Variant { name, mut fields } if name == "Some" && fields.len() == 1 => {
                host_to_typed_interp_value_with_substitutions(
                    fields.remove(0),
                    *inner,
                    store,
                    substitutions,
                )
                .map(Box::new)
                .map(InterpValue::OptionSome)
            }
            value => Err(format!(
                "expected None or Some host variant, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Result { ok, err } => match value {
            HostValue::Variant { name, mut fields } if name == "Ok" && fields.len() == 1 => {
                host_to_typed_interp_value_with_substitutions(
                    fields.remove(0),
                    *ok,
                    store,
                    substitutions,
                )
                .map(|value| InterpValue::Variant {
                    name: "Ok".to_owned(),
                    fields: vec![value],
                })
            }
            HostValue::Variant { name, mut fields } if name == "Err" && fields.len() == 1 => {
                host_to_typed_interp_value_with_substitutions(
                    fields.remove(0),
                    *err,
                    store,
                    substitutions,
                )
                .map(|value| InterpValue::Variant {
                    name: "Err".to_owned(),
                    fields: vec![value],
                })
            }
            value => Err(format!(
                "expected Ok or Err host variant, received {}",
                host_value_kind(&value)
            )),
        },
        Type::Trust { wrapper, inner } => {
            host_to_typed_interp_value_with_substitutions(value, *inner, store, substitutions).map(
                |value| InterpValue::Trust {
                    wrapper: *wrapper,
                    value: Box::new(value),
                },
            )
        }
        Type::Schema(inner) => {
            host_to_typed_interp_value_with_substitutions(value, *inner, store, substitutions)
        }
        Type::Message(inner) => {
            let message = etas_host::session::message_envelope_from_host_value(value)
                .map_err(|error| error.message)?;
            let payload = host_to_typed_interp_value_with_substitutions(
                message.payload,
                *inner,
                store,
                substitutions,
            )
            .map_err(|error| format!("message payload: {error}"))?;
            let provenance = match message.provenance {
                Some(value) => Some(
                    super::boundary_session::provenance_from_host(&value)
                        .map_err(|error| format!("message provenance: {error}"))?,
                ),
                None => None,
            };
            Ok(InterpValue::Message(crate::value::MessageValue {
                id: message.id,
                from: message.from,
                to: message.to,
                role: message_role_from_host(message.role),
                session: message.session.map(|session| session.id),
                created_at: message.created_at,
                payload: Box::new(payload),
                provenance,
            }))
        }
        unsupported => Err(format!(
            "checked type `{unsupported:?}` cannot be decoded from a host value"
        )),
    }
}

fn message_role_from_host(role: etas_host::SessionMessageRole) -> crate::value::MessageRoleValue {
    match role {
        etas_host::SessionMessageRole::System => crate::value::MessageRoleValue::System,
        etas_host::SessionMessageRole::User => crate::value::MessageRoleValue::User,
        etas_host::SessionMessageRole::Assistant => crate::value::MessageRoleValue::Assistant,
        etas_host::SessionMessageRole::Tool => crate::value::MessageRoleValue::Tool,
    }
}

pub(crate) fn host_value_to_json_interp_value(
    value: HostValue,
) -> Result<InterpValue, etas_host::HostError> {
    let json = etas_host::host_value_to_json(&value)?;
    Ok(InterpValue::Json(host_json_support_value_from_serde(json)?))
}

pub(crate) fn host_json_support_value_from_serde(
    value: serde_json::Value,
) -> Result<crate::value::HostJsonSupportValue, etas_host::HostError> {
    Ok(match value {
        serde_json::Value::Null => crate::value::HostJsonSupportValue::Null,
        serde_json::Value::Bool(value) => crate::value::HostJsonSupportValue::Bool(value),
        serde_json::Value::Number(value) => {
            let Some(value) = value.as_f64() else {
                return Err(etas_host::HostError::new(
                    etas_host::HostErrorCode::SchemaMismatch,
                    "JSON number cannot be represented as host f64",
                ));
            };
            crate::value::HostJsonSupportValue::NumberBits(value.to_bits())
        }
        serde_json::Value::String(value) => crate::value::HostJsonSupportValue::String(value),
        serde_json::Value::Array(values) => crate::value::HostJsonSupportValue::Array(
            values
                .into_iter()
                .map(host_json_support_value_from_serde)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        serde_json::Value::Object(entries) => crate::value::HostJsonSupportValue::Object(
            entries
                .into_iter()
                .map(|(name, value)| Ok((name, host_json_support_value_from_serde(value)?)))
                .collect::<Result<Vec<_>, etas_host::HostError>>()?,
        ),
    })
}

pub(crate) fn host_json_support_value_to_host(
    value: &crate::value::HostJsonSupportValue,
) -> etas_host::HostJsonValue {
    match value {
        crate::value::HostJsonSupportValue::Null => etas_host::HostJsonValue::Null,
        crate::value::HostJsonSupportValue::Bool(value) => etas_host::HostJsonValue::Bool(*value),
        crate::value::HostJsonSupportValue::NumberBits(value) => {
            etas_host::HostJsonValue::Number(f64::from_bits(*value))
        }
        crate::value::HostJsonSupportValue::String(value) => {
            etas_host::HostJsonValue::String(value.clone())
        }
        crate::value::HostJsonSupportValue::Array(values) => etas_host::HostJsonValue::Array(
            values.iter().map(host_json_support_value_to_host).collect(),
        ),
        crate::value::HostJsonSupportValue::Object(entries) => etas_host::HostJsonValue::Object(
            entries
                .iter()
                .map(|(name, value)| (name.clone(), host_json_support_value_to_host(value)))
                .collect(),
        ),
    }
}

fn host_json_to_plain_host_value(value: etas_host::HostJsonValue) -> Result<HostValue, String> {
    Ok(match value {
        etas_host::HostJsonValue::Null => HostValue::Unit,
        etas_host::HostJsonValue::Bool(value) => HostValue::Bool(value),
        etas_host::HostJsonValue::Number(value) if value.fract() == 0.0 => {
            let integer = value as i128;
            if integer as f64 != value {
                return Err("JSON integer is not exactly representable as i128".to_owned());
            }
            HostValue::Int(integer)
        }
        etas_host::HostJsonValue::Number(value) if value.is_finite() => HostValue::Float(value),
        etas_host::HostJsonValue::Number(_) => {
            return Err("JSON number is not finite".to_owned());
        }
        etas_host::HostJsonValue::String(value) => HostValue::String(value),
        etas_host::HostJsonValue::Array(values) => HostValue::List(
            values
                .into_iter()
                .map(host_json_to_plain_host_value)
                .collect::<Result<Vec<_>, String>>()?,
        ),
        etas_host::HostJsonValue::Object(entries) => HostValue::Record(
            entries
                .into_iter()
                .map(|(name, value)| Ok((name, host_json_to_plain_host_value(value)?)))
                .collect::<Result<Vec<_>, String>>()?,
        ),
    })
}

fn model_message_key(message: &crate::value::ModelMessageValue) -> String {
    let content = message
        .content
        .iter()
        .map(model_content_key)
        .collect::<Vec<_>>()
        .join(",");
    format!("{:?}:[{content}]", message.role)
}

fn model_content_key(content: &crate::value::ModelContentValue) -> String {
    match content {
        crate::value::ModelContentValue::Text(text) => format!("text:{text:?}"),
        crate::value::ModelContentValue::Value(value) => {
            format!("value:{}", host_support_value_key(value))
        }
    }
}

fn host_support_value_key(value: &crate::value::HostSupportValue) -> String {
    match value {
        crate::value::HostSupportValue::Unit => "unit".to_owned(),
        crate::value::HostSupportValue::Bool(value) => format!("bool:{value}"),
        crate::value::HostSupportValue::Int(value) => format!("int:{value}"),
        crate::value::HostSupportValue::UInt(value) => format!("uint:{value}"),
        crate::value::HostSupportValue::FloatBits(value) => format!("float_bits:{value}"),
        crate::value::HostSupportValue::String(value) => format!("string:{value:?}"),
        crate::value::HostSupportValue::Bytes(value) => format!("bytes:{value:?}"),
        crate::value::HostSupportValue::List(values) => format!(
            "list:[{}]",
            values
                .iter()
                .map(host_support_value_key)
                .collect::<Vec<_>>()
                .join(",")
        ),
        crate::value::HostSupportValue::Map(entries) => format!(
            "map:{{{}}}",
            entries
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    host_support_value_key(key),
                    host_support_value_key(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        crate::value::HostSupportValue::Record(fields) => format!(
            "record:{{{}}}",
            fields
                .iter()
                .map(|(name, value)| format!("{name}:{}", host_support_value_key(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        crate::value::HostSupportValue::Variant { name, fields } => format!(
            "variant:{name}:({})",
            fields
                .iter()
                .map(host_support_value_key)
                .collect::<Vec<_>>()
                .join(",")
        ),
        crate::value::HostSupportValue::Json(value) => {
            format!("json:{}", host_json_support_value_key(value))
        }
    }
}

fn host_json_support_value_key(value: &crate::value::HostJsonSupportValue) -> String {
    match value {
        crate::value::HostJsonSupportValue::Null => "null".to_owned(),
        crate::value::HostJsonSupportValue::Bool(value) => format!("bool:{value}"),
        crate::value::HostJsonSupportValue::NumberBits(value) => format!("number_bits:{value}"),
        crate::value::HostJsonSupportValue::String(value) => format!("string:{value:?}"),
        crate::value::HostJsonSupportValue::Array(values) => format!(
            "array:[{}]",
            values
                .iter()
                .map(host_json_support_value_key)
                .collect::<Vec<_>>()
                .join(",")
        ),
        crate::value::HostJsonSupportValue::Object(entries) => format!(
            "object:{{{}}}",
            entries
                .iter()
                .map(|(name, value)| format!("{name}:{}", host_json_support_value_key(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn decode_host_list(
    value: HostValue,
    elem_type: TypeId,
    store: &etas_types::TypeStore,
    substitutions: &std::collections::HashMap<String, TypeId>,
) -> Result<Vec<InterpValue>, String> {
    let HostValue::List(values) = value else {
        return Err(format!(
            "expected host list, received {}",
            host_value_kind(&value)
        ));
    };
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            host_to_typed_interp_value_with_substitutions(value, elem_type, store, substitutions)
                .map_err(|error| format!("list element {index}: {error}"))
        })
        .collect()
}

fn decode_primitive_host_value(
    value: HostValue,
    primitive: PrimitiveType,
) -> Result<InterpValue, String> {
    let kind = host_value_kind(&value);
    let decoded = match (primitive, value) {
        (PrimitiveType::Bool, HostValue::Bool(value)) => Some(InterpValue::Bool(value)),
        (PrimitiveType::String, HostValue::String(value)) => Some(InterpValue::String(value)),
        (PrimitiveType::Bytes, HostValue::Bytes(value)) => Some(InterpValue::Bytes(value)),
        (PrimitiveType::Unit, HostValue::Unit) => Some(InterpValue::Unit),
        (PrimitiveType::Char, HostValue::String(value)) => {
            let mut chars = value.chars();
            let Some(first) = chars.next() else {
                return Err("host char string is empty".to_owned());
            };
            chars
                .next()
                .is_none()
                .then_some(InterpValue::String(first.to_string()))
        }
        (
            primitive @ (PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize),
            HostValue::Int(value),
        ) => crate::value::NumericValue::from_signed(value, primitive).map(InterpValue::Number),
        (
            primitive @ (PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize),
            HostValue::UInt(value),
        ) => crate::value::NumericValue::from_unsigned(value, primitive).map(InterpValue::Number),
        (
            primitive @ (PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize),
            HostValue::Int(value),
        ) if value >= 0 => crate::value::NumericValue::from_unsigned(value as u128, primitive)
            .map(InterpValue::Number),
        (primitive @ (PrimitiveType::F32 | PrimitiveType::F64), HostValue::Float(value)) => {
            crate::value::NumericValue::from_float(value, primitive).map(InterpValue::Number)
        }
        _ => None,
    };
    decoded.ok_or_else(|| {
        format!(
            "host {kind} value is not representable as {}",
            primitive.source_name()
        )
    })
}

fn numeric_to_host_value(value: crate::value::NumericValue) -> Option<HostValue> {
    if let Some(value) = value.as_i128() {
        Some(HostValue::Int(value))
    } else if let Some(value) = value.as_u128() {
        Some(HostValue::UInt(value))
    } else {
        value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(HostValue::Float)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::NumericValue;
    use etas_types::{NominalTypeRef, Type};

    #[test]
    fn typed_host_codec_restores_every_numeric_width() {
        let mut store = etas_types::TypeStore::new();
        let cases = [
            (PrimitiveType::I8, NumericValue::I8(i8::MIN)),
            (PrimitiveType::I16, NumericValue::I16(i16::MIN)),
            (PrimitiveType::I32, NumericValue::I32(i32::MIN)),
            (PrimitiveType::I64, NumericValue::I64(i64::MIN)),
            (PrimitiveType::I128, NumericValue::I128(i128::MIN)),
            (PrimitiveType::ISize, NumericValue::ISize(i64::MIN)),
            (PrimitiveType::U8, NumericValue::U8(u8::MAX)),
            (PrimitiveType::U16, NumericValue::U16(u16::MAX)),
            (PrimitiveType::U32, NumericValue::U32(u32::MAX)),
            (PrimitiveType::U64, NumericValue::U64(u64::MAX)),
            (PrimitiveType::U128, NumericValue::U128(u128::MAX)),
            (PrimitiveType::USize, NumericValue::USize(u64::MAX)),
            (PrimitiveType::F32, NumericValue::F32((-0.0_f32).to_bits())),
            (PrimitiveType::F64, NumericValue::F64((-0.0_f64).to_bits())),
        ];

        for (primitive, number) in cases {
            let ty = store.intern(Type::Primitive(primitive));
            let host = interp_to_host_value(&InterpValue::Number(number))
                .expect("finite numeric value must be host encodable");
            assert_eq!(
                host_to_typed_interp_value(host, ty, &store),
                Ok(InterpValue::Number(number))
            );
        }
    }

    #[test]
    fn typed_host_codec_preserves_nominal_identity() {
        let mut store = etas_types::TypeStore::new();
        let string = store.intern(Type::Primitive(PrimitiveType::String));
        let nominal = store.intern(Type::Nominal(NominalTypeRef {
            name: "app.main.UserId".to_owned(),
            params: Vec::new(),
            representation: Some(string),
        }));

        assert_eq!(
            host_to_typed_interp_value(HostValue::String("u1".to_owned()), nominal, &store),
            Ok(InterpValue::Nominal {
                ty: nominal,
                value: Box::new(InterpValue::String("u1".to_owned())),
            })
        );
    }

    #[test]
    fn typed_host_codec_preserves_message_envelope_without_session() {
        let mut store = etas_types::TypeStore::new();
        let string = store.intern(Type::Primitive(PrimitiveType::String));
        let message_ty = store.intern(Type::Message(string));
        let expected = InterpValue::Message(crate::value::MessageValue {
            id: "message-1".to_owned(),
            from: Some("agent-a".to_owned()),
            to: Some("agent-b".to_owned()),
            role: crate::value::MessageRoleValue::Assistant,
            session: None,
            created_at: "2026-07-18T00:00:00Z".to_owned(),
            payload: Box::new(InterpValue::String("hello".to_owned())),
            provenance: None,
        });

        let host = interp_to_host_value(&expected).expect("Message envelope must encode");
        assert_eq!(
            host_to_typed_interp_value(host, message_ty, &store),
            Ok(expected)
        );
    }
}
