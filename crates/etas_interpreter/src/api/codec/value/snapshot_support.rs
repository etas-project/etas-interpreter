use super::*;
use crate::orchestration::ValueSnapshot;

pub(super) fn decode(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<ValueSnapshot, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "message" => session::message_parts(limits, value, snapshot_from_json_with_limits)
            .map(|message| ValueSnapshot::Message(message.into())),
        "conversation" => {
            session::conversation_snapshot_from_json(limits, value).map(ValueSnapshot::Conversation)
        }
        "memory_selection" => Ok(ValueSnapshot::MemorySelection {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
            kind: value_codec::memory_selection_kind_from_json(required_str(value, "selection")?)
                .map_err(InterpreterCodecError::new)?,
            predicate: match value.get("predicate") {
                Some(Value::Null) | None => None,
                Some(value) => Some(crate::orchestration::SnapshotBox::new(decode::decode::<
                    ValueSnapshot,
                >(
                    limits, value
                )?)),
            },
            limit: optional_u32(value, "limit")?,
        }),
        "callable" => Ok(ValueSnapshot::Callable(
            machine::call_target_snapshot_from_json(limits, required_obj(value, "target")?)
                .map_err(InterpreterCodecError::new)?,
        )),
        _ => scalar::decode(limits, value).map(Into::into),
    }
}
