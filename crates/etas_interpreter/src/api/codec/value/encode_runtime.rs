use super::*;

enum Node<'a> {
    Value(&'a InterpValue),
    Message(&'a crate::value::MessageValue),
}

pub(super) fn encode(value: &InterpValue) -> Value {
    let mut output = CheckpointDocument::from_value(Value::Null);
    let mut pending = vec![(Node::Value(value), output.value_mut())];
    while let Some((node, slot)) = pending.pop() {
        write(node, slot, |node, slot| pending.push((node, slot)));
    }
    output.into_value()
}

fn write<'a>(node: Node<'a>, slot: &'a mut Value, mut child: impl FnMut(Node<'a>, &'a mut Value)) {
    let value = match node {
        Node::Value(value) => value,
        Node::Message(message) => {
            write_message(message, slot, child);
            return;
        }
    };
    match value {
        InterpValue::Unit => *slot = json!({"kind":"unit"}),
        InterpValue::Bool(value) => *slot = json!({"kind":"bool","value":value}),
        InterpValue::Number(value) => *slot = numeric_value_json(*value),
        InterpValue::String(value) => *slot = json!({"kind":"string","value":value}),
        InterpValue::Bytes(value) => *slot = json!({"kind":"bytes","value":value}),
        InterpValue::Json(value) => *slot = super::super::json::wrapped(value),
        InterpValue::Nominal { ty, value } => {
            *slot = json!({"kind":"nominal","ty":ty.0,"value":null});
            child(Node::Value(value), &mut slot["value"]);
        }
        InterpValue::Trust { wrapper, value } => {
            *slot = json!({"kind":"trust","wrapper":value_codec::trust_wrapper_json(*wrapper),"value":null});
            child(Node::Value(value), &mut slot["value"]);
        }
        InterpValue::OptionNone => *slot = json!({"kind":"option_none"}),
        InterpValue::OptionSome(value) => {
            *slot = json!({"kind":"option_some","value":null});
            child(Node::Value(value), &mut slot["value"]);
        }
        InterpValue::Prompt(messages) => {
            *slot = json!({"kind":"prompt","messages":null});
            for (message, slot) in messages
                .iter()
                .zip(array(&mut slot["messages"], messages.len()))
            {
                *slot = json!({"role":value_codec::prompt_role_json(message.role),"text":message.text,"trust":message.trust.map(value_codec::trust_wrapper_json)});
            }
        }
        InterpValue::Message(message) => write_message(message, slot, child),
        InterpValue::Conversation(conversation) => {
            *slot = json!({"kind":"conversation","selected_context":null,"history_fence":conversation.history_fence.as_ref().map(|fence|fence.as_token()),"session":conversation.session,"messages":null,"cursor":conversation.cursor});
            if let Some(context) = conversation.selected_context.as_deref() {
                slot["selected_context"] = session::selected_context_json(context);
            }
            for (message, slot) in conversation
                .messages
                .iter()
                .zip(array(&mut slot["messages"], conversation.messages.len()))
            {
                child(Node::Message(message), slot);
            }
        }
        InterpValue::Provenance(value) => {
            *slot = json!({"kind":"provenance","value":null});
            slot["value"] = provenance_json(value);
        }
        InterpValue::ModelResponse(value) => *slot = model_response_json(value),
        InterpValue::Command {
            argv,
            env,
            cwd,
            stdin,
        } => {
            *slot = json!({"kind":"command","argv":argv,"env":null,"cwd":null,"stdin":stdin});
            if let Some(path) = cwd {
                slot["cwd"] = json!({"region":path.region.as_str(),"relative":path.relative.to_string_lossy()});
            }
            for ((key, value), slot) in env.iter().zip(array(&mut slot["env"], env.len())) {
                *slot = json!({"key":key,"value":value});
            }
        }
        InterpValue::CommandResult {
            exit_code,
            stdout,
            stderr,
        } => {
            *slot = json!({"kind":"command_result","exit_code":exit_code,"stdout":stdout,"stderr":stderr})
        }
        InterpValue::Tuple(values) => sequence("tuple", values.iter(), slot, child),
        InterpValue::Array(values) => sequence("array", values.borrow().iter(), slot, child),
        InterpValue::List(values) => sequence("list", values.iter(), slot, child),
        InterpValue::Slice(values) => sequence("slice", values.borrow().iter(), slot, child),
        InterpValue::Set(values) => sequence("set", values.borrow().iter(), slot, child),
        InterpValue::Deque(values) => sequence("deque", values.borrow().iter(), slot, child),
        InterpValue::Queue(values) => sequence("queue", values.borrow().iter(), slot, child),
        InterpValue::Stack(values) => sequence("stack", values.borrow().iter(), slot, child),
        InterpValue::OrderedSet(values) => {
            sequence("ordered_set", values.borrow().iter(), slot, child)
        }
        InterpValue::Map(values) => entries("map", "key", values.borrow(), slot, child),
        InterpValue::OrderedMap(values) => {
            entries("ordered_map", "key", values.borrow(), slot, child)
        }
        InterpValue::PriorityQueue(values) => {
            entries("priority_queue", "priority", values.borrow(), slot, child)
        }
        InterpValue::Range(value) => {
            *slot = json!({"kind":"range","start":null,"end":null,"bounds":value_codec::range_bounds_json(value.bounds)});
            let Value::Object(fields) = slot else {
                unreachable!("object initialized above")
            };
            for (name, slot) in fields {
                match name.as_str() {
                    "start" => child(Node::Value(&value.start), slot),
                    "end" => child(Node::Value(&value.end), slot),
                    _ => {}
                }
            }
        }
        InterpValue::Record(fields) => {
            *slot = json!({"kind":"record","fields":null});
            for ((name, value), slot) in fields
                .borrow()
                .iter()
                .zip(array(&mut slot["fields"], fields.borrow().len()))
            {
                *slot = json!({"name":name,"value":null});
                child(Node::Value(value), &mut slot["value"]);
            }
        }
        InterpValue::Variant { name, fields } => {
            *slot = json!({"kind":"variant","name":name,"fields":null});
            for (value, slot) in fields.iter().zip(array(&mut slot["fields"], fields.len())) {
                child(Node::Value(value), slot);
            }
        }
        InterpValue::Callable(target) => {
            *slot = json!({"kind":"callable","target":null});
            slot["target"] = machine::call_target_snapshot(target);
        }
        InterpValue::Handler {
            fact_expr,
            handlers,
        } => {
            *slot = json!({"kind":"handler","fact_expr":fact_expr.0,"handlers":null});
            for (handler, slot) in handlers
                .iter()
                .zip(array(&mut slot["handlers"], handlers.len()))
            {
                *slot = handler_arm_json(handler);
            }
        }
        InterpValue::HostHandle(handle) => {
            *slot = json!({"kind":"host_handle","handle_kind":handle.kind_name()})
        }
        InterpValue::ResourceHandle {
            name,
            stable_id,
            ty,
        } => *slot = json!({"kind":"resource_handle","name":name,"stable_id":stable_id,"ty":ty.0}),
        InterpValue::WorkspacePath(path) => {
            *slot = json!({"kind":"workspace_path","region":path.region.as_str(),"relative":path.relative.to_string_lossy()})
        }
        InterpValue::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        } => {
            *slot = json!({"kind":"memory_store","region_stable_id":region_stable_id,"path":path,"key_type":key_type.0,"value_type":value_type.0})
        }
        InterpValue::MemoryWriteIntent(value) => {
            *slot = json!({"kind":"memory_write_intent","ty":value.ty.0,"key_type":value.key_type.0,"value_type":value.value_type.0,"intent":value.encoded()})
        }
        InterpValue::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate,
            limit,
        } => {
            *slot = json!({"kind":"memory_selection","region_stable_id":region_stable_id,"path":path,"key_type":key_type.0,"value_type":value_type.0,"selection":value_codec::memory_selection_kind_json(kind),"predicate":null,"limit":limit});
            if let Some(predicate) = predicate {
                child(Node::Value(predicate), &mut slot["predicate"]);
            }
        }
    }
}

fn write_message<'a>(
    message: &'a crate::value::MessageValue,
    slot: &'a mut Value,
    mut child: impl FnMut(Node<'a>, &'a mut Value),
) {
    *slot = json!({"kind":"message","id":message.id,"from":message.from,"to":message.to,"role":value_codec::message_role_json(message.role),"session":message.session,"created_at":message.created_at,"payload":null,"provenance":null});
    if let Some(provenance) = &message.provenance {
        slot["provenance"] = provenance_json(provenance);
    }
    child(Node::Value(&message.payload), &mut slot["payload"]);
}

fn sequence<'a>(
    kind: &str,
    values: impl ExactSizeIterator<Item = &'a InterpValue>,
    slot: &'a mut Value,
    mut child: impl FnMut(Node<'a>, &'a mut Value),
) {
    *slot = json!({"kind":kind,"values":null});
    let count = values.len();
    for (value, slot) in values.zip(array(&mut slot["values"], count)) {
        child(Node::Value(value), slot);
    }
}

fn entries<'a>(
    kind: &str,
    key_name: &str,
    entries: &'a [(InterpValue, InterpValue)],
    slot: &'a mut Value,
    mut child: impl FnMut(Node<'a>, &'a mut Value),
) {
    *slot = json!({"kind":kind,"entries":null});
    for ((key, value), slot) in entries
        .iter()
        .zip(array(&mut slot["entries"], entries.len()))
    {
        *slot = json!({key_name:null,"value":null});
        let Value::Object(fields) = slot else {
            unreachable!("object initialized above")
        };
        for (name, slot) in fields {
            child(
                Node::Value(if name == key_name { key } else { value }),
                slot,
            );
        }
    }
}

fn array(slot: &mut Value, count: usize) -> std::slice::IterMut<'_, Value> {
    *slot = Value::Array((0..count).map(|_| Value::Null).collect());
    let Value::Array(values) = slot else {
        unreachable!("array initialized above")
    };
    values.iter_mut()
}
