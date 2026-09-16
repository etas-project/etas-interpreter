use super::{Node, Pending, array};
use crate::api::codec::{self, value::*, value_codec};
use crate::orchestration::{MessageSnapshot, ValueSnapshot};
use serde_json::{Value, json};

pub(super) fn encode<'a>(value: &'a ValueSnapshot, slot: &'a mut Value, pending: &mut Pending<'a>) {
    use ValueSnapshot as V;
    // Only scalar metadata goes through serde. Recursive children are filled in
    // their final JSON slots, never serialized/cloned through an ancestor.
    match value {
        V::Nominal { ty, value } => {
            *slot = json!({"kind":"nominal", "ty":ty.0, "value":null});
            pending.push((Node::Value(value), &mut slot["value"]));
        }
        V::ModelResponse(value) => {
            super::model(super::encode_model::Part::Response(value), slot, pending)
        }
        V::Trust { wrapper, value } => {
            *slot = json!({"kind":"trust", "wrapper":value_codec::trust_wrapper_json(*wrapper), "value":null});
            pending.push((Node::Value(value), &mut slot["value"]));
        }
        V::OptionSome(value) => {
            *slot = json!({"kind":"option_some", "value":null});
            pending.push((Node::Value(value), &mut slot["value"]));
        }
        V::Tuple(values)
        | V::Array(values)
        | V::List(values)
        | V::Slice(values)
        | V::Set(values)
        | V::Deque(values)
        | V::Queue(values)
        | V::Stack(values)
        | V::OrderedSet(values) => {
            let kind = match value {
                V::Tuple(_) => "tuple",
                V::Array(_) => "array",
                V::List(_) => "list",
                V::Slice(_) => "slice",
                V::Set(_) => "set",
                V::Deque(_) => "deque",
                V::Queue(_) => "queue",
                V::Stack(_) => "stack",
                V::OrderedSet(_) => "ordered_set",
                _ => unreachable!("sequence variant was matched"),
            };
            *slot = json!({"kind":kind, "values":null});
            sequence(values, &mut slot["values"], pending);
        }
        V::Variant { name, fields } => {
            *slot = json!({"kind":"variant", "name":name, "fields":null});
            sequence(fields, &mut slot["fields"], pending);
        }
        V::Map(values) | V::OrderedMap(values) | V::PriorityQueue(values) => {
            let (kind, key_name) = match value {
                V::Map(_) => ("map", "key"),
                V::OrderedMap(_) => ("ordered_map", "key"),
                V::PriorityQueue(_) => ("priority_queue", "priority"),
                _ => unreachable!("pair variant was matched"),
            };
            *slot = json!({"kind":kind, "entries":null});
            for ((key, value), entry) in
                values.iter().zip(array(&mut slot["entries"], values.len()))
            {
                *entry = json!({key_name:null, "value":null});
                if let Value::Object(fields) = entry {
                    for (name, field) in fields {
                        pending.push((
                            Node::Value(if name == key_name { key } else { value }),
                            field,
                        ));
                    }
                }
            }
        }
        V::Record(values) => {
            *slot = json!({"kind":"record", "fields":null});
            for ((name, value), entry) in
                values.iter().zip(array(&mut slot["fields"], values.len()))
            {
                *entry = json!({"name":name, "value":null});
                pending.push((Node::Value(value), &mut entry["value"]));
            }
        }
        V::Range { start, end, bounds } => {
            *slot = json!({"kind":"range", "bounds":value_codec::range_bounds_json(*bounds), "start":null,"end":null});
            if let Value::Object(fields) = slot {
                for (name, field) in fields {
                    match name.as_str() {
                        "start" => pending.push((Node::Value(start), field)),
                        "end" => pending.push((Node::Value(end), field)),
                        _ => {}
                    }
                }
            }
        }
        V::Message(value) => message(value, slot, pending),
        V::Conversation(value) => {
            *slot = json!({"kind":"conversation", "selected_context":value.selected_context.as_deref().map(codec::value::session::selected_context_json),
                "history_fence":value.history_fence.as_ref().map(|f|f.as_token()), "session":value.session, "cursor":value.cursor, "messages":null});
            for (message, slot) in value
                .messages
                .iter()
                .zip(array(&mut slot["messages"], value.messages.len()))
            {
                pending.push((Node::Message(message), slot));
            }
        }
        V::Callable(target) => {
            *slot = json!({"kind":"callable", "target":null});
            pending.push((Node::CallTarget(target), &mut slot["target"]));
        }
        V::Json(value) => {
            *slot = json!({"kind":"json","value":null});
            pending.push((Node::Json(value), &mut slot["value"]));
        }
        V::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate,
            limit,
        } => {
            *slot = json!({"kind":"memory_selection", "region_stable_id":region_stable_id, "path":path,
                "key_type":key_type.0,"value_type":value_type.0,"selection":value_codec::memory_selection_kind_json(kind),"predicate":null,"limit":limit});
            if let Some(value) = predicate {
                pending.push((Node::Value(value), &mut slot["predicate"]));
            }
        }
        _ => *slot = leaf(value),
    }
}
fn sequence<'a>(values: &'a [ValueSnapshot], slot: &'a mut Value, pending: &mut Pending<'a>) {
    pending.extend(
        values
            .iter()
            .zip(array(slot, values.len()))
            .map(|(value, slot)| (Node::Value(value), slot)),
    );
}
pub(super) fn message<'a>(
    value: &'a MessageSnapshot,
    slot: &'a mut Value,
    pending: &mut Pending<'a>,
) {
    *slot = json!({"kind":"message","id":value.id,"from":value.from,"to":value.to,
        "role":value_codec::message_role_json(value.role),"session":value.session,"created_at":value.created_at,
        "payload":null,"provenance":value.provenance.as_ref().map(provenance_json)});
    pending.push((Node::Value(&value.payload), &mut slot["payload"]));
}
fn leaf(value: &ValueSnapshot) -> Value {
    use ValueSnapshot as V;
    match value {
        V::MemoryWriteIntent(value) => {
            json!({"kind":"memory_write_intent","ty":value.ty.0,"key_type":value.key_type.0,"value_type":value.value_type.0,"intent":value.encoded()})
        }
        V::Unit => json!({"kind":"unit"}),
        V::OptionNone => json!({"kind":"option_none"}),
        V::Bool(value) => json!({"kind":"bool","value":value}),
        V::Number(value) => numeric_value_json(*value),
        V::String(value) => json!({"kind":"string","value":value}),
        V::Bytes(value) => json!({"kind":"bytes","value":value}),
        V::Prompt(messages) => {
            json!({"kind":"prompt", "messages":messages.iter().map(|message| json!({"role":value_codec::prompt_role_json(message.role),"text":message.text,"trust":message.trust.map(value_codec::trust_wrapper_json)})).collect::<Vec<_>>()})
        }
        V::Provenance(value) => json!({"kind":"provenance","value":provenance_json(value)}),
        V::Command {
            argv,
            env,
            cwd,
            stdin,
        } => {
            json!({"kind":"command","argv":argv,"env":env.iter().map(|(key,value)|json!({"key":key,"value":value})).collect::<Vec<_>>(),"cwd":cwd.as_ref().map(|path|json!({"region":path.region.as_str(),"relative":path.relative.to_string_lossy()})),"stdin":stdin})
        }
        V::CommandResult {
            exit_code,
            stdout,
            stderr,
        } => json!({"kind":"command_result","exit_code":exit_code,"stdout":stdout,"stderr":stderr}),
        V::Handler {
            fact_expr,
            handlers,
        } => {
            json!({"kind":"handler","fact_expr":fact_expr.0,"handlers":handlers.iter().map(codec::handler_arm_json).collect::<Vec<_>>()})
        }
        V::ResourceHandle {
            name,
            stable_id,
            ty,
        } => json!({"kind":"resource_handle","name":name,"stable_id":stable_id,"ty":ty.0}),
        V::WorkspacePath { region, relative } => {
            json!({"kind":"workspace_path","region":region,"relative":relative})
        }
        V::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        } => {
            json!({"kind":"memory_store","region_stable_id":region_stable_id,"path":path,"key_type":key_type.0,"value_type":value_type.0})
        }
        V::Nominal { .. }
        | V::Trust { .. }
        | V::OptionSome(_)
        | V::Tuple(_)
        | V::Array(_)
        | V::List(_)
        | V::Slice(_)
        | V::Set(_)
        | V::Deque(_)
        | V::Queue(_)
        | V::Stack(_)
        | V::OrderedSet(_)
        | V::Variant { .. }
        | V::Map(_)
        | V::OrderedMap(_)
        | V::PriorityQueue(_)
        | V::Record(_)
        | V::Range { .. }
        | V::Message(_)
        | V::Conversation(_)
        | V::Callable(_)
        | V::Json(_)
        | V::ModelResponse(_)
        | V::MemorySelection { .. } => unreachable!("compound value must fill child slots"),
    }
}
