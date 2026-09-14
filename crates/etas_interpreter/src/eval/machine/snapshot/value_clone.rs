use crate::orchestration::{
    ConversationSnapshot, MessageSnapshot, SnapshotBox, SnapshotChildren, ValueSnapshot,
};

impl Clone for ValueSnapshot {
    fn clone(&self) -> Self {
        let mut output = self.clone_shell();
        let mut pending = Vec::new();
        self.zip_children_mut(&mut output, &mut pending);
        while let Some((source, output)) = pending.pop() {
            *output = source.clone_shell();
            source.zip_children_mut(output, &mut pending);
        }
        output
    }
}

impl ValueSnapshot {
    // Child slots are private builder state. Every slot is filled before the
    // result escapes; no mutable backing from a running evaluator is retained.
    fn clone_shell(&self) -> Self {
        match self {
            Self::MemoryWriteIntent(v) => Self::MemoryWriteIntent(v.clone()),
            Self::Unit => Self::Unit,
            Self::Bool(v) => Self::Bool(*v),
            Self::Number(v) => Self::Number(*v),
            Self::String(v) => Self::String(v.clone()),
            Self::Bytes(v) => Self::Bytes(v.clone()),
            Self::Json(v) => Self::Json(v.clone()),
            Self::Nominal { ty, .. } => Self::Nominal {
                ty: *ty,
                value: SnapshotBox::new(Self::Unit),
            },
            Self::Trust { wrapper, .. } => Self::Trust {
                wrapper: *wrapper,
                value: SnapshotBox::new(Self::Unit),
            },
            Self::Prompt(v) => Self::Prompt(v.clone()),
            Self::Message(v) => Self::Message(message_shell(v)),
            Self::Conversation(v) => Self::Conversation(ConversationSnapshot {
                selected_context: v.selected_context.clone(),
                session: v.session.clone(),
                history_fence: v.history_fence.clone(),
                cursor: v.cursor.clone(),
                messages: v.messages.iter().map(message_shell).collect(),
            }),
            Self::Provenance(v) => Self::Provenance(v.clone()),
            Self::ModelResponse(v) => Self::ModelResponse(v.clone()),
            Self::Command {
                argv,
                env,
                cwd,
                stdin,
            } => Self::Command {
                argv: argv.clone(),
                env: env.clone(),
                cwd: cwd.clone(),
                stdin: stdin.clone(),
            },
            Self::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => Self::CommandResult {
                exit_code: *exit_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            },
            Self::Tuple(v) => Self::Tuple(slots(v.len())),
            Self::Array(v) => Self::Array(slots(v.len())),
            Self::List(v) => Self::List(slots(v.len())),
            Self::Slice(v) => Self::Slice(slots(v.len())),
            Self::Map(v) => Self::Map(pair_slots(v.len())),
            Self::Set(v) => Self::Set(slots(v.len())),
            Self::Deque(v) => Self::Deque(slots(v.len())),
            Self::Queue(v) => Self::Queue(slots(v.len())),
            Self::Stack(v) => Self::Stack(slots(v.len())),
            Self::PriorityQueue(v) => Self::PriorityQueue(pair_slots(v.len())),
            Self::OrderedMap(v) => Self::OrderedMap(pair_slots(v.len())),
            Self::OrderedSet(v) => Self::OrderedSet(slots(v.len())),
            Self::Range { bounds, .. } => Self::Range {
                start: SnapshotBox::new(Self::Unit),
                end: SnapshotBox::new(Self::Unit),
                bounds: *bounds,
            },
            Self::Record(v) => Self::Record(
                v.iter()
                    .map(|(name, _)| (name.clone(), Self::Unit))
                    .collect(),
            ),
            Self::Variant { name, fields } => Self::Variant {
                name: name.clone(),
                fields: slots(fields.len()),
            },
            Self::OptionNone => Self::OptionNone,
            Self::OptionSome(_) => Self::OptionSome(SnapshotBox::new(Self::Unit)),
            Self::Callable(v) => Self::Callable(v.clone()),
            Self::Handler {
                fact_expr,
                handlers,
            } => Self::Handler {
                fact_expr: *fact_expr,
                handlers: handlers.clone(),
            },
            Self::ResourceHandle {
                name,
                stable_id,
                ty,
            } => Self::ResourceHandle {
                name: name.clone(),
                stable_id: stable_id.clone(),
                ty: *ty,
            },
            Self::WorkspacePath { region, relative } => Self::WorkspacePath {
                region: region.clone(),
                relative: relative.clone(),
            },
            Self::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            } => Self::MemoryStore {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                key_type: *key_type,
                value_type: *value_type,
            },
            Self::MemorySelection {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
            } => Self::MemorySelection {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                key_type: *key_type,
                value_type: *value_type,
                kind: kind.clone(),
                predicate: predicate.as_ref().map(|_| SnapshotBox::new(Self::Unit)),
                limit: *limit,
            },
        }
    }

    fn zip_children_mut<'a>(
        &'a self,
        output: &'a mut Self,
        pending: &mut Vec<(&'a Self, &'a mut Self)>,
    ) {
        match (self, output) {
            (Self::Nominal { value: a, .. }, Self::Nominal { value: b, .. })
            | (Self::Trust { value: a, .. }, Self::Trust { value: b, .. })
            | (Self::OptionSome(a), Self::OptionSome(b)) => pending.push((a, b)),
            (Self::Tuple(a), Self::Tuple(b))
            | (Self::Array(a), Self::Array(b))
            | (Self::List(a), Self::List(b))
            | (Self::Slice(a), Self::Slice(b))
            | (Self::Set(a), Self::Set(b))
            | (Self::Deque(a), Self::Deque(b))
            | (Self::Queue(a), Self::Queue(b))
            | (Self::Stack(a), Self::Stack(b))
            | (Self::OrderedSet(a), Self::OrderedSet(b))
            | (Self::Variant { fields: a, .. }, Self::Variant { fields: b, .. }) => {
                pending.extend(a.iter().zip(b))
            }
            (Self::Map(a), Self::Map(b))
            | (Self::OrderedMap(a), Self::OrderedMap(b))
            | (Self::PriorityQueue(a), Self::PriorityQueue(b)) => {
                for ((ak, av), (bk, bv)) in a.iter().zip(b) {
                    pending.push((ak, bk));
                    pending.push((av, bv));
                }
            }
            (Self::Record(a), Self::Record(b)) => {
                pending.extend(a.iter().zip(b).map(|((_, a), (_, b))| (a, b)))
            }
            (
                Self::Range {
                    start: a, end: c, ..
                },
                Self::Range {
                    start: b, end: d, ..
                },
            ) => {
                pending.push((a, b));
                pending.push((c, d));
            }
            (Self::Message(a), Self::Message(b)) => pending.push((&a.payload, &mut b.payload)),
            (Self::Conversation(a), Self::Conversation(b)) => pending.extend(
                a.messages
                    .iter()
                    .zip(&mut b.messages)
                    .map(|(a, b)| (a.payload.as_ref(), b.payload.as_mut())),
            ),
            (
                Self::MemorySelection {
                    predicate: Some(a), ..
                },
                Self::MemorySelection {
                    predicate: Some(b), ..
                },
            ) => pending.push((a, b)),
            _ => {}
        }
    }
}

fn slots(count: usize) -> SnapshotChildren<ValueSnapshot> {
    std::iter::repeat_with(|| ValueSnapshot::Unit)
        .take(count)
        .collect()
}

fn pair_slots(count: usize) -> SnapshotChildren<(ValueSnapshot, ValueSnapshot)> {
    std::iter::repeat_with(|| (ValueSnapshot::Unit, ValueSnapshot::Unit))
        .take(count)
        .collect()
}

fn message_shell(v: &MessageSnapshot) -> MessageSnapshot {
    MessageSnapshot {
        id: v.id.clone(),
        from: v.from.clone(),
        to: v.to.clone(),
        role: v.role,
        session: v.session.clone(),
        created_at: v.created_at.clone(),
        payload: SnapshotBox::new(ValueSnapshot::Unit),
        provenance: v.provenance.clone(),
    }
}
