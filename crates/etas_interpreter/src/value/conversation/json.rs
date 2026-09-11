use super::budget::{MessageView, Payload, ViewBudget};
use etas_host::StorageLimits;
use serde_json::Value;

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("missing conversation field `{key}`"))
}
fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("invalid conversation text `{key}`"))
}
fn optional<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match field(value, key)? {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        _ => Err(format!("invalid optional text `{key}`")),
    }
}
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    field(value, key)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("invalid conversation array `{key}`"))
}

// Inspect borrowed JSON before the codec clones strings or allocates message/payload vectors.
pub(crate) fn validate_json(value: &Value, limits: &StorageLimits) -> Result<(), String> {
    let messages = array(value, "messages")?;
    let mut budget = ViewBudget::new(limits, text(value, "session")?, messages.len())?;
    for key in ["cursor", "history_fence"] {
        if let Some(value) = optional(value, key)? {
            budget.charge(value.len(), 0)?;
        }
    }
    if let context @ Value::Object(_) = field(value, "selected_context")? {
        let provenance = field(context, "provenance")?
            .as_object()
            .ok_or("invalid context provenance")?;
        budget.context(
            text(context, "text")?,
            text(context, "fence")?,
            provenance.iter().map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.as_str(), v))
                    .ok_or_else(|| "invalid context provenance value".into())
            }),
        )?;
    }
    for message in messages {
        if text(message, "kind")? != "message" {
            return Err("conversation requires message entries".into());
        }
        let provenance = match field(message, "provenance")? {
            Value::Null => None,
            p => Some((optional(p, "trace_id")?, optional(p, "source")?)),
        };
        budget.message(MessageView {
            id: text(message, "id")?,
            session: optional(message, "session")?,
            from: optional(message, "from")?,
            to: optional(message, "to")?,
            created_at: text(message, "created_at")?,
            payload: field(message, "payload")?,
            provenance,
        })?;
    }
    Ok(())
}

impl Payload for Value {
    fn measure(&self, budget: &mut ViewBudget<'_>, depth: usize) -> Result<(), String> {
        budget.node(depth)?;
        match text(self, "kind")? {
            "message" => {
                let provenance = match field(self, "provenance")? {
                    Value::Null => None,
                    p => Some((optional(p, "trace_id")?, optional(p, "source")?)),
                };
                budget.envelope(
                    MessageView {
                        id: text(self, "id")?,
                        session: optional(self, "session")?,
                        from: optional(self, "from")?,
                        to: optional(self, "to")?,
                        created_at: text(self, "created_at")?,
                        payload: field(self, "payload")?,
                        provenance,
                    },
                    depth,
                )
            }
            "unit" | "bool" | "number" | "option_none" | "null" | "number_bits" => Ok(()),
            "string" => budget.charge(text(self, "value")?.len(), depth),
            "bytes" => budget.charge(array(self, "value")?.len(), depth),
            "nominal" | "trust" | "option_some" | "json" => {
                field(self, "value")?.measure(budget, depth + 1)
            }
            "variant" => {
                budget.charge(text(self, "name")?.len(), depth)?;
                for value in array(self, "fields")? {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            "tuple" | "array" | "list" | "slice" | "set" | "deque" | "queue" | "stack"
            | "ordered_set" => {
                for value in array(self, "values")? {
                    value.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            "map" | "ordered_map" | "priority_queue" => {
                let key = if text(self, "kind")? == "priority_queue" {
                    "priority"
                } else {
                    "key"
                };
                for entry in array(self, "entries")? {
                    field(entry, key)?.measure(budget, depth + 1)?;
                    field(entry, "value")?.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            "record" | "object" => {
                let (entries, key) = if text(self, "kind")? == "record" {
                    ("fields", "name")
                } else {
                    ("entries", "key")
                };
                for entry in array(self, entries)? {
                    budget.charge(text(entry, key)?.len(), depth)?;
                    field(entry, "value")?.measure(budget, depth + 1)?;
                }
                Ok(())
            }
            _ => Err("unsupported value in conversation storage payload".into()),
        }
    }
}
