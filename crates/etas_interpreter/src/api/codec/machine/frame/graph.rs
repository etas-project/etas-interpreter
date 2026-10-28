use super::super::value::{required, required_str};
use serde_json::Value;
use std::{collections::HashMap, slice};

#[derive(Clone, Copy)]
pub(in crate::api::codec) enum DecodeRoot<'a> {
    Value(&'a Value),
    Target(&'a Value),
    Frame(&'a Value),
}

pub(in crate::api::codec) struct FrameDecodeCache<F> {
    first: Option<(*const Value, F)>,
    frames: HashMap<*const Value, F>,
}

impl<F> Default for FrameDecodeCache<F> {
    fn default() -> Self {
        Self {
            first: None,
            frames: HashMap::new(),
        }
    }
}

impl<F: Clone> FrameDecodeCache<F> {
    fn contains(&self, value: &Value) -> bool {
        let key = value as *const _;
        self.first.as_ref().is_some_and(|(first, _)| *first == key)
            || self.frames.contains_key(&key)
    }
    pub(in crate::api::codec) fn get(&self, value: &Value) -> Result<F, String> {
        let key = value as *const _;
        self.first
            .as_ref()
            .filter(|(first, _)| *first == key)
            .map(|(_, value)| value)
            .or_else(|| self.frames.get(&key))
            .cloned()
            .ok_or_else(|| "captured frame has no prepared decoder definition".into())
    }

    fn insert(&mut self, input: &Value, value: F) {
        let key = input as *const _;
        if self.first.is_none() {
            self.first = Some((key, value));
        } else {
            self.frames.insert(key, value);
        }
    }
}

enum Task<'a> {
    Children(Children<'a>),
    FinishFrame(&'a Value),
}

#[derive(Default)]
struct Tasks<'a> {
    first: Option<Task<'a>>,
    rest: Vec<Task<'a>>,
}

impl<'a> Tasks<'a> {
    fn push(&mut self, task: Task<'a>) {
        if self.first.is_none() {
            self.first = Some(task);
        } else {
            self.rest.push(task);
        }
    }
    fn pop(&mut self) -> Option<Task<'a>> {
        self.rest.pop().or_else(|| self.first.take())
    }
}

pub(super) fn prepare<F: Clone>(
    root: DecodeRoot<'_>,
    decode: impl FnMut(&Value, &FrameDecodeCache<F>) -> Result<F, String>,
) -> Result<FrameDecodeCache<F>, String> {
    // This is executable checkpoint structure, not a stored message payload.
    prepare_with_budget(
        crate::api::codec::CheckpointFileLimits::default().max_nodes,
        root,
        decode,
    )
}

fn prepare_with_budget<F: Clone>(
    max_nodes: usize,
    root: DecodeRoot<'_>,
    mut decode: impl FnMut(&Value, &FrameDecodeCache<F>) -> Result<F, String>,
) -> Result<FrameDecodeCache<F>, String> {
    let mut cache = FrameDecodeCache::default();
    let mut pending = Tasks::default();
    let mut next = Some(root);
    let mut nodes = 0usize;
    loop {
        if let Some(node) = next.take() {
            nodes = nodes
                .checked_add(1)
                .filter(|count| *count <= max_nodes)
                .ok_or("checkpoint frame graph exceeds node budget")?;
            let mut children = match node {
                DecodeRoot::Frame(value) => {
                    if cache.contains(value) {
                        continue;
                    }
                    pending.push(Task::FinishFrame(value));
                    Children::Fields(array(value, "locals")?.iter(), "value")
                }
                DecodeRoot::Value(value) => value_children(value)?,
                DecodeRoot::Target(value) => target_children(value)?,
            };
            if let Some(child) = children.next()? {
                pending.push(Task::Children(children));
                next = Some(child);
                continue;
            }
        }
        loop {
            match pending.pop() {
                Some(Task::FinishFrame(value)) => {
                    let decoded = decode(value, &cache)?;
                    cache.insert(value, decoded);
                }
                Some(Task::Children(mut children)) => {
                    if let Some(child) = children.next()? {
                        pending.push(Task::Children(children));
                        next = Some(child);
                        break;
                    }
                }
                None => return Ok(cache),
            }
        }
    }
}

enum Children<'a> {
    One(Option<DecodeRoot<'a>>),
    Two(std::array::IntoIter<Option<&'a Value>, 2>),
    Values {
        values: slice::Iter<'a, Value>,
        target: bool,
        last: Option<&'a Value>,
    },
    Fields(slice::Iter<'a, Value>, &'static str),
    Pairs {
        values: slice::Iter<'a, Value>,
        key: &'static str,
        next: Option<&'a Value>,
        last: Option<&'a Value>,
    },
}

impl<'a> Children<'a> {
    fn next(&mut self) -> Result<Option<DecodeRoot<'a>>, String> {
        Ok(match self {
            Self::One(value) => value.take(),
            Self::Two(values) => values.find_map(|value| value).map(DecodeRoot::Value),
            Self::Values {
                values,
                target,
                last,
            } => values
                .next()
                .map(|value| {
                    if *target {
                        DecodeRoot::Target(value)
                    } else {
                        DecodeRoot::Value(value)
                    }
                })
                .or_else(|| last.take().map(DecodeRoot::Value)),
            Self::Fields(values, field) => values
                .next()
                .map(|value| required(value, field).map(DecodeRoot::Value))
                .transpose()?,
            Self::Pairs {
                values,
                key,
                next,
                last,
            } => {
                if let Some(value) = next.take() {
                    return Ok(Some(DecodeRoot::Value(value)));
                }
                if let Some(value) = values.next() {
                    *next = Some(required(value, "value")?);
                    Some(DecodeRoot::Value(required(value, key)?))
                } else {
                    last.take().map(DecodeRoot::Value)
                }
            }
        })
    }
}

fn array<'a>(value: &'a Value, name: &str) -> Result<&'a [Value], String> {
    required(value, name)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("checkpoint `{name}` must be an array"))
}

fn optional(value: &Value) -> Option<&Value> {
    (!value.is_null()).then_some(value)
}

fn value_children(value: &Value) -> Result<Children<'_>, String> {
    Ok(match required_str(value, "kind")? {
        "callable" => Children::One(Some(DecodeRoot::Target(required(value, "target")?))),
        "nominal" | "trust" | "option_some" => {
            Children::One(Some(DecodeRoot::Value(required(value, "value")?)))
        }
        "message" => Children::One(Some(DecodeRoot::Value(required(value, "payload")?))),
        "memory_selection" => Children::One(
            value
                .get("predicate")
                .and_then(optional)
                .map(DecodeRoot::Value),
        ),
        "range" => Children::Two(
            [
                Some(required(value, "start")?),
                Some(required(value, "end")?),
            ]
            .into_iter(),
        ),
        "tuple" | "array" | "list" | "slice" | "set" | "deque" | "queue" | "stack"
        | "ordered_set" => Children::Values {
            values: array(value, "values")?.iter(),
            target: false,
            last: value.get("comparator").and_then(optional),
        },
        "variant" => Children::Values {
            values: array(value, "fields")?.iter(),
            target: false,
            last: None,
        },
        "conversation" => Children::Values {
            values: array(value, "messages")?.iter(),
            target: false,
            last: None,
        },
        "record" => Children::Fields(array(value, "fields")?.iter(), "value"),
        kind @ ("map" | "ordered_map" | "priority_queue") => Children::Pairs {
            values: array(value, "entries")?.iter(),
            key: if kind == "priority_queue" {
                "priority"
            } else {
                "key"
            },
            next: None,
            last: value.get("comparator").and_then(optional),
        },
        // Opaque JSON/Prompt data is not executable checkpoint syntax.
        _ => Children::One(None),
    })
}

fn target_children(value: &Value) -> Result<Children<'_>, String> {
    Ok(match required_str(value, "kind")? {
        "lambda" => {
            super::required_u32(value, "expr")?;
            Children::One(Some(DecodeRoot::Frame(required(value, "captured")?)))
        }
        "scoped_callable" | "specialized" | "limited" => {
            Children::One(Some(DecodeRoot::Target(required(value, "target")?)))
        }
        "composed" => Children::Values {
            values: required(value, "targets")?
                .as_array()
                .ok_or("machine snapshot call targets must be an array")?
                .iter(),
            target: true,
            last: None,
        },
        _ => Children::One(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frame_preparation_preserves_conflicting_occurrences_and_checks_total_graph_budget() {
        let wire = json!({"kind":"composed", "targets":[
            {"kind":"lambda", "expr":1, "captured":{"id":9,"locals":[
                {"symbol":0,"value":{"kind":"bool","value":true}}
            ]}},
            {"kind":"lambda", "expr":1, "captured":{"id":9,"locals":[
                {"symbol":0,"value":{"kind":"bool","value":false}}
            ]}}
        ]});
        let first = &wire["targets"][0]["captured"];
        let second = &wire["targets"][1]["captured"];
        let decode = |frame: &Value, _: &FrameDecodeCache<bool>| {
            frame["locals"][0]["value"]["value"]
                .as_bool()
                .ok_or("missing bool".into())
        };
        let cache = prepare_with_budget(7, DecodeRoot::Target(&wire), decode).unwrap();
        assert!(cache.get(first).unwrap());
        assert!(!cache.get(second).unwrap());
        assert!(
            prepare_with_budget(6, DecodeRoot::Target(&wire), decode)
                .err()
                .unwrap()
                .contains("node budget")
        );
        assert!(prepare_with_budget(0, DecodeRoot::Target(&wire), decode).is_err());
    }
}
