use serde_json::{Value, json};
mod decode;
pub(in crate::api::codec) use decode::{call_target_snapshot_with_frames, call_target_with_frames};
pub(super) mod revision;
mod target;
#[cfg(test)]
mod tests;
pub(super) use decode::call_target_snapshots_from_json;
pub(crate) use decode::{call_target_from_artifact_snapshot, call_target_snapshot_from_json};

pub(in crate::api::codec) fn intrinsic_substitutions_snapshot(
    steps: &[crate::intrinsic::dispatch::IntrinsicSubstitutionStep],
) -> Value {
    json!(
        steps
            .iter()
            .map(|step| json!({
                "types": step.types.iter().map(|(name, ty)| json!({"name": name, "type": ty.0})).collect::<Vec<_>>(),
                "type_parameter_bindings": step.parameters,
                "effect_row_bindings": step.rows,
            }))
            .collect::<Vec<_>>()
    )
}
