use super::RestoreContext;
use crate::orchestration::SnapshotBox;
use crate::orchestration::ValueSnapshot;
use crate::value::{InterpValue, RangeValue};

#[cfg(test)]
#[path = "value_capture_tests.rs"]
mod capture_tests;

impl ValueSnapshot {
    pub(crate) fn capture(value: &InterpValue) -> Result<Self, String> {
        super::value_capture::capture(value)
    }

    pub(super) fn capture_leaf(value: &InterpValue) -> Result<Self, String> {
        Ok(match value {
            InterpValue::MemoryWriteIntent(value) => Self::MemoryWriteIntent(value.clone()),
            InterpValue::Unit => Self::Unit,
            InterpValue::Bool(value) => Self::Bool(*value),
            InterpValue::Number(value) => Self::Number(*value),
            InterpValue::String(value) => Self::String(value.clone()),
            InterpValue::Bytes(value) => Self::Bytes(value.clone()),
            InterpValue::Json(value) => Self::Json(value.clone()),
            InterpValue::Prompt(messages) => Self::Prompt(messages.clone()),
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
            InterpValue::Range(value) => Self::Range {
                start: SnapshotBox::new(Self::capture(&value.start)?),
                end: SnapshotBox::new(Self::capture(&value.end)?),
                bounds: value.bounds,
            },
            InterpValue::OptionNone => Self::OptionNone,
            InterpValue::Nominal { .. }
            | InterpValue::Message(_)
            | InterpValue::Conversation(_)
            | InterpValue::Trust { .. }
            | InterpValue::OptionSome(_)
            | InterpValue::Tuple(_)
            | InterpValue::Variant { .. }
            | InterpValue::Record(_)
            | InterpValue::Array(_)
            | InterpValue::Stack(_)
            | InterpValue::List(_)
            | InterpValue::Slice(_)
            | InterpValue::Set(_)
            | InterpValue::OrderedSet(_)
            | InterpValue::Deque(_)
            | InterpValue::Queue(_)
            | InterpValue::Map(_)
            | InterpValue::OrderedMap(_)
            | InterpValue::PriorityQueue(_) => {
                return Err("compound checkpoint value must use its capture builder".into());
            }
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
                    .map(SnapshotBox::new),
                limit: *limit,
            },
        })
    }

    pub(crate) fn restore(self) -> Result<InterpValue, String> {
        self.restore_with(&mut RestoreContext::default())
    }

    pub(crate) fn restore_with(self, context: &mut RestoreContext) -> Result<InterpValue, String> {
        super::value_restore::restore(self, context)
    }

    pub(super) fn restore_leaf_with(
        self,
        context: &mut RestoreContext,
    ) -> Result<InterpValue, String> {
        Ok(match self {
            Self::MemoryWriteIntent(value) => InterpValue::MemoryWriteIntent(value),
            Self::Unit => InterpValue::Unit,
            Self::Bool(value) => InterpValue::Bool(value),
            Self::Number(value) => InterpValue::Number(value),
            Self::String(value) => InterpValue::String(value),
            Self::Bytes(value) => InterpValue::Bytes(value),
            Self::Json(value) => InterpValue::Json(value),
            Self::Prompt(messages) => InterpValue::Prompt(messages),
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
            Self::Range { start, end, bounds } => InterpValue::Range(RangeValue {
                start: Box::new(start.into_value().restore_with(context)?),
                end: Box::new(end.into_value().restore_with(context)?),
                bounds,
            }),
            Self::OptionNone => InterpValue::OptionNone,
            Self::Nominal { .. }
            | Self::Message(_)
            | Self::Conversation(_)
            | Self::Trust { .. }
            | Self::OptionSome(_)
            | Self::Tuple(_)
            | Self::Variant { .. }
            | Self::Record(_)
            | Self::Array(_)
            | Self::Stack(_)
            | Self::List(_)
            | Self::Slice(_)
            | Self::Set(_)
            | Self::OrderedSet(_)
            | Self::Deque(_)
            | Self::Queue(_)
            | Self::Map(_)
            | Self::OrderedMap(_)
            | Self::PriorityQueue(_) => {
                return Err("compound checkpoint value must use its restore builder".into());
            }
            Self::Callable(target) => {
                InterpValue::Callable(super::call_target::restore_call_target(target, context)?)
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
                    .map(|value| value.into_value().restore())
                    .transpose()?
                    .map(Box::new),
                limit,
            },
        })
    }
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
