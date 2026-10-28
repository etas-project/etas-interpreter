use serde_json::Value;

use crate::orchestration::CheckpointAgentReference;

pub(in crate::api::codec::machine) fn decode(
    limits: &etas_host::StorageLimits,
    raw: &Value,
) -> Result<CheckpointAgentReference, String> {
    bounded_json(limits, raw)?;
    let reference: CheckpointAgentReference = serde_json::from_value(raw.clone())
        .map_err(|error| format!("invalid checkpoint agent reference: {error}"))?;
    reference.validate_resources(limits)?;
    reference.revision.validate()?;
    Ok(reference)
}

pub(in crate::api::codec::machine) fn bounded_json(
    limits: &etas_host::StorageLimits,
    raw: &Value,
) -> Result<(), String> {
    // Bound the untrusted JSON before cloning it into a typed reference. No
    // native callable is constructed here; restoration still needs the set.
    let mut pending = vec![(raw, 0usize)];
    let (mut nodes, mut bytes) = (0usize, 0usize);
    while let Some((node, depth)) = pending.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or("checkpoint agent reference exceeds node budget")?;
        if nodes > limits.max_nodes || depth > limits.max_depth {
            return Err("checkpoint agent reference exceeds node/depth budget".into());
        }
        match node {
            Value::String(text) => bytes = bytes.saturating_add(text.len()),
            Value::Array(children) => {
                if children
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(nodes)
                    > limits.max_nodes
                {
                    return Err("checkpoint agent reference exceeds node budget".into());
                }
                pending.extend(children.iter().map(|child| (child, depth + 1)));
            }
            Value::Object(fields) => {
                if fields
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(nodes)
                    > limits.max_nodes
                {
                    return Err("checkpoint agent reference exceeds node budget".into());
                }
                for (name, child) in fields {
                    bytes = bytes.saturating_add(name.len());
                    pending.push((child, depth + 1));
                }
            }
            _ => {}
        }
        if bytes > limits.max_value_bytes {
            return Err("checkpoint agent reference exceeds byte budget".into());
        }
    }
    Ok(())
}
