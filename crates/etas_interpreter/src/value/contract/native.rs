use super::{Context, Seed, Target, Task, Value};
use crate::{orchestration::ValueSnapshot as S, value::InterpValue as R};
use etas_std::RuntimeTypeRole as Role;
use etas_types::{ResourceHandleType, Type, TypeId};

pub(super) fn validate<'a>(
    context: &impl Context,
    value: Value<'a>,
    expected: TypeId,
    pending: &mut Vec<Task<'a>>,
) -> Result<(), String> {
    let checked = context.checked();
    let store = &checked.type_store;
    let ty = store
        .get(expected)
        .ok_or("native input has no checked type")?;
    let role = checked.types.known_std_types.runtime_role(store, expected);
    match value {
        Value::Runtime(R::Callable(target)) => context.callable(Target::Runtime(target), expected),
        Value::Snapshot(S::Callable(target)) => {
            context.callable(Target::Snapshot(target), expected)
        }
        Value::Runtime(R::Prompt(_)) | Value::Snapshot(S::Prompt(_))
            if matches!(ty, Type::Prompt) =>
        {
            Ok(())
        }
        Value::Runtime(R::Schema(actual)) | Value::Snapshot(S::Schema(actual))
            if matches!(ty, Type::Schema(inner) if inner == actual)
                && store.get(*actual).is_some() =>
        {
            Ok(())
        }
        Value::Runtime(R::Json(value)) | Value::Snapshot(S::Json(value))
            if role == Some(Role::JsonValue) =>
        {
            crate::value::json::validate(value)
        }
        Value::Runtime(R::Provenance(_)) | Value::Snapshot(S::Provenance(_))
            if role == Some(Role::Provenance) =>
        {
            Ok(())
        }
        Value::Runtime(R::ModelResponse(_)) | Value::Snapshot(S::ModelResponse(_))
            if role == Some(Role::ModelResponse) =>
        {
            Ok(())
        }
        Value::Runtime(R::Command { .. }) | Value::Snapshot(S::Command { .. })
            if role == Some(Role::Command) =>
        {
            Ok(())
        }
        Value::Runtime(R::CommandResult { .. }) | Value::Snapshot(S::CommandResult { .. })
            if role == Some(Role::CommandResult) =>
        {
            Ok(())
        }
        Value::Runtime(R::Message(message)) => {
            let Type::Message(payload) = ty else {
                return Err("message input has a different checked type".into());
            };
            crate::value::conversation::validate_message(message, context.limits())?;
            pending.push(Task::Visit(
                Seed::new(Value::Runtime(&message.payload), *payload),
                false,
            ));
            Ok(())
        }
        Value::Snapshot(S::Message(message)) => {
            let Type::Message(payload) = ty else {
                return Err("message input has a different checked type".into());
            };
            crate::value::conversation::validate_message_snapshot(message, context.limits())?;
            pending.push(Task::Visit(
                Seed::new(Value::Snapshot(&message.payload), *payload),
                false,
            ));
            Ok(())
        }
        Value::Runtime(R::Conversation(conversation)) if role == Some(Role::Conversation) => {
            crate::value::conversation::validate(conversation, context.limits())?;
            let json = checked
                .types
                .known_std_types
                .runtime_type(Role::JsonValue)
                .ok_or("conversation has no checked JSON payload identity")?;
            pending.push(Task::Children(Box::new(conversation.messages.iter().map(
                move |message| Seed::new(Value::Runtime(&message.payload), json),
            ))));
            Ok(())
        }
        Value::Snapshot(S::Conversation(conversation)) if role == Some(Role::Conversation) => {
            crate::value::conversation::validate_snapshot(conversation, context.limits())?;
            let json = checked
                .types
                .known_std_types
                .runtime_type(Role::JsonValue)
                .ok_or("conversation has no checked JSON payload identity")?;
            pending.push(Task::Children(Box::new(conversation.messages.iter().map(
                move |message| Seed::new(Value::Snapshot(&message.payload), json),
            ))));
            Ok(())
        }
        Value::Runtime(R::WorkspacePath(path)) => {
            etas_host::sandbox::workspace::normalize_relative(&path.relative)
                .map_err(|error| error.message)?;
            workspace_path(context, expected, path.region.as_str())
        }
        Value::Snapshot(S::WorkspacePath { region, .. }) => {
            workspace_path(context, expected, region)
        }
        Value::Runtime(R::MemoryWriteIntent(intent))
        | Value::Snapshot(S::MemoryWriteIntent(intent)) => {
            let Type::Applied { args, .. } = ty else {
                return Err("memory write intent is not an applied checked type".into());
            };
            if role != Some(Role::MemoryWriteIntent)
                || expected != intent.ty
                || args.as_slice() != [intent.key_type, intent.value_type]
            {
                return Err("memory write intent has a different checked generic identity".into());
            }
            intent
                .intent()
                .validate(context.limits())
                .map_err(|error| error.message)
        }
        Value::Runtime(R::ResourceHandle {
            name,
            stable_id,
            ty: actual,
        })
        | Value::Snapshot(S::ResourceHandle {
            name,
            stable_id,
            ty: actual,
        }) => {
            if *actual != expected || stable_id.is_empty() {
                return Err("resource handle has a different checked identity".into());
            }
            let valid = match ty {
                Type::MemoryRegion(_)
                | Type::ResourceHandle(ResourceHandleType::MemoryRegion { .. }) => {
                    name == "MemoryRegion"
                }
                Type::ResourceHandle(ResourceHandleType::ExternalTool { .. }) => {
                    name == "ExternalTool"
                }
                Type::ResourceHandle(ResourceHandleType::Other { name: expected, .. }) => {
                    name == expected
                }
                _ => false,
            };
            valid
                .then_some(())
                .ok_or_else(|| "resource handle kind differs from checked facts".into())
        }
        Value::Runtime(R::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        })
        | Value::Snapshot(S::MemoryStore {
            region_stable_id,
            path,
            key_type,
            value_type,
        }) => {
            if !matches!(ty, Type::Store { key, value } if key == key_type && value == value_type) {
                return Err("memory store has different checked payload types".into());
            }
            memory_place(region_stable_id, path)
        }
        Value::Runtime(R::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate_type,
            predicate,
            ..
        }) => {
            selection(
                context,
                expected,
                *key_type,
                *value_type,
                region_stable_id,
                path,
            )?;
            if let Some(seed) = selection_query(
                kind,
                *predicate_type,
                predicate.as_deref().map(Value::Runtime),
            )? {
                pending.push(Task::Visit(seed, false));
            }
            Ok(())
        }
        Value::Snapshot(S::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate_type,
            predicate,
            ..
        }) => {
            selection(
                context,
                expected,
                *key_type,
                *value_type,
                region_stable_id,
                path,
            )?;
            if let Some(seed) = selection_query(
                kind,
                *predicate_type,
                predicate.as_deref().map(Value::Snapshot),
            )? {
                pending.push(Task::Visit(seed, false));
            }
            Ok(())
        }
        Value::Runtime(
            R::HostHandle(_)
            | R::Handler { .. }
            | R::CodeMetadata(_)
            | R::SyntaxBlock(_)
            | R::AgentDefinition(_)
            | R::CallableDefinition(_),
        )
        | Value::Snapshot(S::CodeIdentity { .. } | S::Handler { .. }) => {
            context.native(value, expected)
        }
        _ => Err("native input does not match its checked type identity".into()),
    }
}

fn workspace_path(context: &impl Context, ty: TypeId, region: &str) -> Result<(), String> {
    let checked = context.checked();
    let Type::Applied { args, .. } = checked.type_store.get(ty).ok_or("path type is missing")?
    else {
        return Err("path has no checked region argument".into());
    };
    if checked
        .types
        .known_std_types
        .runtime_role(&checked.type_store, ty)
        != Some(Role::WorkspacePath)
        || args.len() != 1
    {
        return Err("path has no canonical WorkspacePath identity".into());
    }
    let Some(Type::Nominal(nominal)) = checked.type_store.get(args[0]) else {
        return Err("path region has no checked nominal identity".into());
    };
    if nominal.name != region {
        return Err("path belongs to a different checked region".into());
    }
    Ok(())
}

fn memory_place(region: &str, path: &[String]) -> Result<(), String> {
    if region.is_empty() || path.iter().any(String::is_empty) {
        return Err("memory place has an empty identity segment".into());
    }
    Ok(())
}

fn selection(
    context: &impl Context,
    expected: TypeId,
    key: TypeId,
    value: TypeId,
    region: &str,
    path: &[String],
) -> Result<(), String> {
    let store = &context.checked().type_store;
    if !matches!(store.get(expected), Some(Type::MemorySelection(inner)) if *inner == value)
        || store.get(key).is_none()
    {
        return Err("memory selection has different checked payload types".into());
    }
    memory_place(region, path)
}

pub(crate) fn selection_query<'a>(
    kind: &crate::value::MemorySelectionKind,
    ty: Option<TypeId>,
    value: Option<Value<'a>>,
) -> Result<Option<Seed<'a>>, String> {
    match (kind, ty, value) {
        (crate::value::MemorySelectionKind::Scan, None, None) => Ok(None),
        (crate::value::MemorySelectionKind::Scan, _, _) => {
            Err("memory scan carries an unexpected query".into())
        }
        (_, Some(ty), Some(value)) => Ok(Some(Seed::new(value, ty))),
        _ => Err("memory query is missing its checked argument type or value".into()),
    }
}
