use super::*;

pub(super) fn session_config_from_value(
    value: &InterpValue,
) -> Option<crate::value::SessionConfigValue> {
    match value {
        InterpValue::Variant { name, fields }
            if name == "SessionConfig.continue_or_new" && fields.len() == 1 =>
        {
            let key = stable_interp_key(&fields[0])?;
            Some(crate::value::SessionConfigValue {
                id: format!("continue_or_new:{key}"),
                context: None,
                retention: None,
                compaction: None,
            })
        }
        InterpValue::Nominal { value, .. } => session_config_from_value(value),
        InterpValue::Record(fields) => {
            let snapshot = fields.snapshot();
            let id = snapshot.iter().find_map(|(field, value)| {
                (field == "id").then(|| match value {
                    InterpValue::String(value) => Some(value.clone()),
                    _ => None,
                })?
            })?;
            Some(crate::value::SessionConfigValue {
                id,
                context: snapshot
                    .iter()
                    .find(|(field, _)| field == "context")
                    .map(|(_, value)| Box::new(value.clone())),
                retention: snapshot
                    .iter()
                    .find(|(field, _)| field == "retention")
                    .map(|(_, value)| Box::new(value.clone())),
                compaction: snapshot
                    .iter()
                    .find(|(field, _)| field == "compaction")
                    .map(|(_, value)| Box::new(value.clone())),
            })
        }
        _ => None,
    }
}

pub(super) fn abort_message_method(span: Span, message: &'static str) -> ControlSignal {
    ControlSignal::invalid_arguments(message.to_owned(), span)
}

pub(super) fn current_message_timestamp() -> Result<String, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .map_err(|error| format!("runtime clock cannot timestamp Message.new: {error}"))
}

pub(super) fn stable_interp_key(value: &InterpValue) -> Option<String> {
    match value {
        InterpValue::String(value) => Some(value.clone()),
        InterpValue::Number(value) => Some(value.display_value()),
        InterpValue::Bool(value) => Some(value.to_string()),
        InterpValue::Nominal { value, .. } => stable_interp_key(value),
        _ => None,
    }
}

pub(super) fn collection_pop_result(
    collection: InterpValue,
    popped: Option<InterpValue>,
) -> InterpValue {
    InterpValue::Tuple(vec![
        collection,
        popped
            .map(Box::new)
            .map(InterpValue::OptionSome)
            .unwrap_or(InterpValue::OptionNone),
    ])
}

pub(super) fn unsupported_collection_method(
    span: Span,
    receiver: &str,
    method: &str,
) -> ControlSignal {
    unsupported_method(span, receiver, method)
}

pub(super) fn unsupported_method(span: Span, receiver: &str, method: &str) -> ControlSignal {
    let message = format!("unsupported {receiver} method `{method}`");
    ControlSignal::invalid_arguments(message, span)
}

pub(super) struct LocalMethodArgContinuation<'a> {
    pub(super) expr: HirExprId,
    pub(super) receiver: InterpValue,
    pub(super) method: &'a str,
    pub(super) type_args: &'a [etas_hir::HirTypeId],
    pub(super) args: Vec<HirArg>,
    pub(super) next_arg_index: usize,
    pub(super) evaluated_args: Vec<InterpValue>,
    pub(super) span: Span,
    pub(super) frame: &'a Frame,
}

pub(super) fn attach_local_method_args_continuation(
    signal: ControlSignal,
    pending: LocalMethodArgContinuation<'_>,
) -> ControlSignal {
    let continuation = Continuation::LocalMethodArgs {
        expr: pending.expr,
        receiver: pending.receiver,
        method: pending.method.to_owned(),
        type_args: pending.type_args.to_vec(),
        args: pending.args,
        next_arg_index: pending.next_arg_index,
        evaluated_args: pending.evaluated_args,
        span: pending.span,
        frame: pending.frame.clone(),
    };
    attach_continuation(signal, continuation)
}

pub(super) fn local_value_method_expected_arg_count(
    receiver: &InterpValue,
    method: &str,
) -> Option<usize> {
    let count = match receiver {
        InterpValue::Message(_) => match method {
            "cast" => 0,
            _ => return None,
        },
        InterpValue::Array(_) => match method {
            "len" | "is_empty" | "pop" => 0,
            "get" | "at" | "push" | "extend" => 1,
            _ => return None,
        },
        InterpValue::List(_) => match method {
            "len" | "is_empty" | "pop" => 0,
            "push" => 1,
            _ => return None,
        },
        InterpValue::Slice(_) => match method {
            "len" | "is_empty" | "to_array" => 0,
            "get" | "at" => 1,
            _ => return None,
        },
        InterpValue::Map(_) => match method {
            "len" | "is_empty" => 0,
            "contains_key" | "get" => 1,
            _ => return None,
        },
        InterpValue::Deque(_) => match method {
            "len" | "is_empty" | "pop_front" | "pop_back" => 0,
            "push_front" | "push_back" => 1,
            _ => return None,
        },
        InterpValue::Queue(_) | InterpValue::Stack(_) => match method {
            "len" | "is_empty" | "pop" => 0,
            "push" => 1,
            _ => return None,
        },
        InterpValue::PriorityQueue(_) => match method {
            "len" | "is_empty" | "pop" => 0,
            "push" => 2,
            _ => return None,
        },
        InterpValue::OrderedMap(_) => match method {
            "len" | "is_empty" => 0,
            "contains_key" | "get" => 1,
            "insert" => 2,
            _ => return None,
        },
        InterpValue::OrderedSet(_) => match method {
            "len" | "is_empty" => 0,
            "contains" | "insert" => 1,
            _ => return None,
        },
        _ => return None,
    };
    Some(count)
}

pub(super) fn local_method_label(receiver: &InterpValue, method: &str) -> String {
    format!("{}.{}", local_receiver_name(receiver), method)
}

pub(super) fn local_receiver_name(receiver: &InterpValue) -> &'static str {
    match receiver {
        InterpValue::Message(_) => "Message",
        InterpValue::Array(_) => "Array",
        InterpValue::List(_) => "List",
        InterpValue::Slice(_) => "Slice",
        InterpValue::Map(_) => "Map",
        InterpValue::Deque(_) => "Deque",
        InterpValue::Queue(_) => "Queue",
        InterpValue::Stack(_) => "Stack",
        InterpValue::PriorityQueue(_) => "PriorityQueue",
        InterpValue::OrderedMap(_) => "OrderedMap",
        InterpValue::OrderedSet(_) => "OrderedSet",
        _ => "runtime value",
    }
}

pub(super) fn attach_method_receiver_continuation(
    signal: ControlSignal,
    expr: HirExprId,
    method: &str,
    type_args: &[etas_hir::HirTypeId],
    args: &[HirArg],
    span: Span,
    frame: &Frame,
) -> ControlSignal {
    let continuation = Continuation::MethodReceiver {
        expr,
        method: method.to_owned(),
        type_args: type_args.to_vec(),
        args: args.to_vec(),
        span,
        frame: frame.clone(),
    };
    attach_continuation(signal, continuation)
}

pub(super) fn attach_prompt_value_method_arg_continuation(
    signal: ControlSignal,
    messages: Vec<crate::value::PromptMessage>,
    method: &str,
    role: crate::value::PromptRole,
    allow_plain_system_content: bool,
    span: Span,
) -> ControlSignal {
    let continuation = Continuation::PromptValueMethodArg {
        messages,
        method: method.to_owned(),
        role,
        allow_plain_system_content,
        span,
    };
    attach_continuation(signal, continuation)
}

pub(super) fn attach_continuation(
    signal: ControlSignal,
    continuation: Continuation,
) -> ControlSignal {
    match signal {
        ControlSignal::Expr(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Expr(pending)
        }
        ControlSignal::Call(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Call(pending)
        }
        ControlSignal::Memory(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Memory(pending)
        }
        ControlSignal::Session(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Session(pending)
        }
        ControlSignal::Perform(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Perform(pending)
        }
        ControlSignal::Console(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Console(pending)
        }
        ControlSignal::Command(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Command(pending)
        }
        ControlSignal::Model(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Model(pending)
        }
        ControlSignal::Host(mut pending) => {
            pending.continuation = compose_continuation(pending.continuation, continuation);
            ControlSignal::Host(pending)
        }
        other => other,
    }
}

pub(super) fn prompt_data_kind(value: &InterpValue) -> &'static str {
    match value {
        InterpValue::Command { .. } => "command handle",
        InterpValue::CommandResult { .. } => "command result",
        InterpValue::Callable(_) => "callable",
        InterpValue::Handler { .. } => "handler",
        InterpValue::HostHandle(_) => "host handle",
        InterpValue::ResourceHandle { .. } => "resource handle",
        InterpValue::MemoryStore { .. } => "memory store",
        InterpValue::MemorySelection { .. } => "memory selection",
        InterpValue::Provenance(_) => "provenance",
        InterpValue::ModelResponse(_) => "model response",
        InterpValue::Trust { wrapper, .. } if *wrapper == etas_types::TrustWrapper::Secret => {
            "secret"
        }
        _ => "unsupported runtime",
    }
}

pub(super) fn host_value_to_embedding(value: &HostValue) -> Option<Vec<f32>> {
    let HostValue::List(values) = value else {
        return None;
    };
    let embedding = values
        .iter()
        .map(host_number_to_f32)
        .collect::<Option<Vec<_>>>()?;
    (!embedding.is_empty()).then_some(embedding)
}

pub(super) fn host_number_to_f32(value: &HostValue) -> Option<f32> {
    match value {
        HostValue::Float(value) if value.is_finite() => Some(*value as f32),
        HostValue::Int(value) => Some(*value as f32),
        HostValue::UInt(value) => Some(*value as f32),
        _ => None,
    }
}

pub(super) fn prompt_data_contains_secret(value: &InterpValue) -> bool {
    match value {
        InterpValue::Trust { wrapper, value } => {
            *wrapper == etas_types::TrustWrapper::Secret || prompt_data_contains_secret(value)
        }
        InterpValue::Tuple(values) => values.iter().any(prompt_data_contains_secret),
        InterpValue::Array(values)
        | InterpValue::Deque(values)
        | InterpValue::Queue(values)
        | InterpValue::Stack(values) => values.snapshot().iter().any(prompt_data_contains_secret),
        InterpValue::List(values) => values.snapshot().iter().any(prompt_data_contains_secret),
        InterpValue::Slice(values) => values.snapshot().iter().any(prompt_data_contains_secret),
        InterpValue::Set(values) | InterpValue::OrderedSet(values) => {
            values.snapshot().iter().any(prompt_data_contains_secret)
        }
        InterpValue::Map(entries)
        | InterpValue::OrderedMap(entries)
        | InterpValue::PriorityQueue(entries) => entries.snapshot().iter().any(|(key, value)| {
            prompt_data_contains_secret(key) || prompt_data_contains_secret(value)
        }),
        InterpValue::Record(fields) => fields
            .snapshot()
            .iter()
            .any(|(_, value)| prompt_data_contains_secret(value)),
        InterpValue::Range(range) => {
            prompt_data_contains_secret(&range.start) || prompt_data_contains_secret(&range.end)
        }
        InterpValue::Variant { fields, .. } => fields.iter().any(prompt_data_contains_secret),
        InterpValue::OptionSome(value)
        | InterpValue::Message(crate::value::MessageValue { payload: value, .. }) => {
            prompt_data_contains_secret(value)
        }
        _ => false,
    }
}
