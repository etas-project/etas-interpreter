use crate::orchestration::{ConversationSnapshot, MessageSnapshot, ValueSnapshot};
use crate::value::{
    ArrayValue, ConversationValue, InterpValue, ListValue, MapValue, MessageValue, RangeValue,
    RecordValue, SetValue, SliceValue,
};

impl ValueSnapshot {
    pub(crate) fn capture(value: &InterpValue) -> Result<Self, String> {
        Ok(match value {
            InterpValue::MemoryWriteIntent(value) => Self::MemoryWriteIntent(value.clone()),
            InterpValue::Unit => Self::Unit,
            InterpValue::Bool(value) => Self::Bool(*value),
            InterpValue::Number(value) => Self::Number(*value),
            InterpValue::String(value) => Self::String(value.clone()),
            InterpValue::Bytes(value) => Self::Bytes(value.clone()),
            InterpValue::Json(value) => Self::Json(value.clone()),
            InterpValue::Nominal { ty, value } => Self::Nominal {
                ty: *ty,
                value: Box::new(Self::capture(value)?),
            },
            InterpValue::Trust { wrapper, value } => Self::Trust {
                wrapper: *wrapper,
                value: Box::new(Self::capture(value)?),
            },
            InterpValue::Prompt(messages) => Self::Prompt(messages.clone()),
            InterpValue::Message(message) => Self::Message(MessageSnapshot::capture(message)?),
            InterpValue::Conversation(conversation) => {
                Self::Conversation(ConversationSnapshot::capture(conversation)?)
            }
            InterpValue::Provenance(value) => Self::Provenance(value.clone()),
            InterpValue::ModelResponse(value) => Self::ModelResponse(value.clone()),
            InterpValue::Command {
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
            InterpValue::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => Self::CommandResult {
                exit_code: *exit_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            },
            InterpValue::Tuple(values) => Self::Tuple(capture_values(values)?),
            InterpValue::Array(values) => Self::Array(capture_values(&values.snapshot())?),
            InterpValue::List(values) => Self::List(capture_values(&values.snapshot())?),
            InterpValue::Slice(values) => Self::Slice(capture_values(&values.snapshot())?),
            InterpValue::Map(values) => Self::Map(capture_pairs(&values.snapshot())?),
            InterpValue::Set(values) => Self::Set(capture_values(&values.snapshot())?),
            InterpValue::Deque(values) => Self::Deque(capture_values(&values.snapshot())?),
            InterpValue::Queue(values) => Self::Queue(capture_values(&values.snapshot())?),
            InterpValue::Stack(values) => Self::Stack(capture_values(&values.snapshot())?),
            InterpValue::PriorityQueue(values) => {
                Self::PriorityQueue(capture_pairs(&values.snapshot())?)
            }
            InterpValue::OrderedMap(values) => Self::OrderedMap(capture_pairs(&values.snapshot())?),
            InterpValue::OrderedSet(values) => {
                Self::OrderedSet(capture_values(&values.snapshot())?)
            }
            InterpValue::Range(value) => Self::Range {
                start: Box::new(Self::capture(&value.start)?),
                end: Box::new(Self::capture(&value.end)?),
                bounds: value.bounds,
            },
            InterpValue::Record(values) => Self::Record(
                values
                    .snapshot()
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), Self::capture(value)?)))
                    .collect::<Result<Vec<_>, String>>()?,
            ),
            InterpValue::Variant { name, fields } => Self::Variant {
                name: name.clone(),
                fields: capture_values(fields)?,
            },
            InterpValue::OptionNone => Self::OptionNone,
            InterpValue::OptionSome(value) => Self::OptionSome(Box::new(Self::capture(value)?)),
            InterpValue::Callable(target) => {
                Self::Callable(super::call_target::capture_call_target(target)?)
            }
            InterpValue::Handler {
                fact_expr,
                handlers,
            } => Self::Handler {
                fact_expr: *fact_expr,
                handlers: handlers.clone(),
            },
            InterpValue::HostHandle(handle) => {
                return Err(format!(
                    "live {} host handles cannot be captured in a checkpoint",
                    handle.kind_name()
                ));
            }
            InterpValue::ResourceHandle {
                name,
                stable_id,
                ty,
            } => Self::ResourceHandle {
                name: name.clone(),
                stable_id: stable_id.clone(),
                ty: *ty,
            },
            InterpValue::WorkspacePath(path) => Self::WorkspacePath {
                region: path.region.as_str().to_owned(),
                relative: path.relative.to_string_lossy().into_owned(),
            },
            InterpValue::MemoryStore {
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
            InterpValue::MemorySelection {
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
                predicate: predicate
                    .as_deref()
                    .map(Self::capture)
                    .transpose()?
                    .map(Box::new),
                limit: *limit,
            },
        })
    }

    pub(crate) fn restore(self) -> Result<InterpValue, String> {
        Ok(match self {
            Self::MemoryWriteIntent(value) => InterpValue::MemoryWriteIntent(value),
            Self::Unit => InterpValue::Unit,
            Self::Bool(value) => InterpValue::Bool(value),
            Self::Number(value) => InterpValue::Number(value),
            Self::String(value) => InterpValue::String(value),
            Self::Bytes(value) => InterpValue::Bytes(value),
            Self::Json(value) => InterpValue::Json(value),
            Self::Nominal { ty, value } => InterpValue::Nominal {
                ty,
                value: Box::new(value.restore()?),
            },
            Self::Trust { wrapper, value } => InterpValue::Trust {
                wrapper,
                value: Box::new(value.restore()?),
            },
            Self::Prompt(messages) => InterpValue::Prompt(messages),
            Self::Message(message) => InterpValue::Message(message.restore()?),
            Self::Conversation(conversation) => InterpValue::Conversation(conversation.restore()?),
            Self::Provenance(value) => InterpValue::Provenance(value),
            Self::ModelResponse(value) => InterpValue::ModelResponse(value),
            Self::Command {
                argv,
                env,
                cwd,
                stdin,
            } => InterpValue::Command {
                argv,
                env,
                cwd,
                stdin,
            },
            Self::CommandResult {
                exit_code,
                stdout,
                stderr,
            } => InterpValue::CommandResult {
                exit_code,
                stdout,
                stderr,
            },
            Self::Tuple(values) => InterpValue::Tuple(restore_values(values)?),
            Self::Array(values) => InterpValue::Array(ArrayValue::new(restore_values(values)?)),
            Self::List(values) => InterpValue::List(ListValue::new(restore_values(values)?)),
            Self::Slice(values) => InterpValue::Slice(SliceValue::new(restore_values(values)?)),
            Self::Map(values) => InterpValue::Map(MapValue::new(restore_pairs(values)?)),
            Self::Set(values) => InterpValue::Set(SetValue::new(restore_values(values)?)),
            Self::Deque(values) => InterpValue::Deque(ArrayValue::new(restore_values(values)?)),
            Self::Queue(values) => InterpValue::Queue(ArrayValue::new(restore_values(values)?)),
            Self::Stack(values) => InterpValue::Stack(ArrayValue::new(restore_values(values)?)),
            Self::PriorityQueue(values) => {
                InterpValue::PriorityQueue(MapValue::new(restore_pairs(values)?))
            }
            Self::OrderedMap(values) => {
                InterpValue::OrderedMap(MapValue::new(restore_pairs(values)?))
            }
            Self::OrderedSet(values) => {
                InterpValue::OrderedSet(SetValue::new(restore_values(values)?))
            }
            Self::Range { start, end, bounds } => InterpValue::Range(RangeValue {
                start: Box::new(start.restore()?),
                end: Box::new(end.restore()?),
                bounds,
            }),
            Self::Record(values) => InterpValue::Record(RecordValue::new(
                values
                    .into_iter()
                    .map(|(name, value)| Ok((name, value.restore()?)))
                    .collect::<Result<Vec<_>, String>>()?,
            )),
            Self::Variant { name, fields } => InterpValue::Variant {
                name,
                fields: restore_values(fields)?,
            },
            Self::OptionNone => InterpValue::OptionNone,
            Self::OptionSome(value) => InterpValue::OptionSome(Box::new(value.restore()?)),
            Self::Callable(target) => {
                InterpValue::Callable(super::call_target::restore_call_target(target)?)
            }
            Self::Handler {
                fact_expr,
                handlers,
            } => InterpValue::Handler {
                fact_expr,
                handlers,
            },
            Self::ResourceHandle {
                name,
                stable_id,
                ty,
            } => InterpValue::ResourceHandle {
                name,
                stable_id,
                ty,
            },
            Self::WorkspacePath { region, relative } => InterpValue::WorkspacePath(
                etas_host::WorkspacePathRef::new(
                    etas_host::WorkspaceRegionId::new(region).map_err(|error| error.message)?,
                    relative,
                )
                .map_err(|error| error.message)?,
            ),
            Self::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            } => InterpValue::MemoryStore {
                region_stable_id,
                path,
                key_type,
                value_type,
            },
            Self::MemorySelection {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
            } => InterpValue::MemorySelection {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate: predicate
                    .map(|value| value.restore())
                    .transpose()?
                    .map(Box::new),
                limit,
            },
        })
    }
}

impl MessageSnapshot {
    fn capture(message: &MessageValue) -> Result<Self, String> {
        Ok(Self {
            id: message.id.clone(),
            from: message.from.clone(),
            to: message.to.clone(),
            role: message.role,
            session: message.session.clone(),
            created_at: message.created_at.clone(),
            payload: Box::new(ValueSnapshot::capture(&message.payload)?),
            provenance: message.provenance.clone(),
        })
    }

    fn restore(self) -> Result<MessageValue, String> {
        Ok(MessageValue {
            id: self.id,
            from: self.from,
            to: self.to,
            role: self.role,
            session: self.session,
            created_at: self.created_at,
            payload: Box::new(self.payload.restore()?),
            provenance: self.provenance,
        })
    }
}

impl ConversationSnapshot {
    fn capture(conversation: &ConversationValue) -> Result<Self, String> {
        Ok(Self {
            selected_context: conversation.selected_context.as_deref().cloned(),
            session: conversation.session.clone(),
            history_fence: conversation.history_fence.clone(),
            messages: conversation
                .messages
                .iter()
                .map(MessageSnapshot::capture)
                .collect::<Result<Vec<_>, _>>()?,
            cursor: conversation.cursor.clone(),
        })
    }

    fn restore(self) -> Result<ConversationValue, String> {
        Ok(ConversationValue {
            selected_context: self.selected_context.map(Box::new),
            session: self.session,
            history_fence: self.history_fence,
            messages: self
                .messages
                .into_iter()
                .map(MessageSnapshot::restore)
                .collect::<Result<Vec<_>, _>>()?,
            cursor: self.cursor,
        })
    }
}

fn capture_values(values: &[InterpValue]) -> Result<Vec<ValueSnapshot>, String> {
    values.iter().map(ValueSnapshot::capture).collect()
}

fn restore_values(values: Vec<ValueSnapshot>) -> Result<Vec<InterpValue>, String> {
    values.into_iter().map(ValueSnapshot::restore).collect()
}

fn capture_pairs(
    values: &[(InterpValue, InterpValue)],
) -> Result<Vec<(ValueSnapshot, ValueSnapshot)>, String> {
    values
        .iter()
        .map(|(key, value)| Ok((ValueSnapshot::capture(key)?, ValueSnapshot::capture(value)?)))
        .collect()
}

fn restore_pairs(
    values: Vec<(ValueSnapshot, ValueSnapshot)>,
) -> Result<Vec<(InterpValue, InterpValue)>, String> {
    values
        .into_iter()
        .map(|(key, value)| Ok((key.restore()?, value.restore()?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::HostHandleValue;

    #[test]
    fn checkpoint_capture_rejects_live_host_capabilities() {
        let value = InterpValue::HostHandle(HostHandleValue::tcp_stream(
            etas_types::TypeId(1),
            etas_host::TcpStreamRef::issued(
                etas_host::StreamHandleRef::issued("tcp-live", 0),
                etas_host::ByteStreamOrigin::Tcp {
                    host: "example.test".to_owned(),
                    port: 443,
                },
            ),
        ));

        let error = ValueSnapshot::capture(&value)
            .expect_err("live stream handles must not enter checkpoint artifacts");
        assert!(error.contains("live tcp_stream host handles"), "{error}");
    }
}
