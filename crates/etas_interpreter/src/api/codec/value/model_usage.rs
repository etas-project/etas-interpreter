use serde_json::{Value, json};

use super::{InterpreterCodecError, required_obj, required_str};

pub(in crate::api::codec) fn encode(usage: &etas_host::ModelUsage) -> Value {
    json!({
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cost": usage.cost.as_ref().map(|cost| json!({
            "micros": cost.micros.to_string(), "currency": cost.currency,
        })),
    })
}

pub(in crate::api::codec) fn decode(
    value: &Value,
) -> Result<etas_host::ModelUsage, InterpreterCodecError> {
    let token = |name| match required_obj(value, name)? {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| InterpreterCodecError::new("invalid optional model usage token count")),
    };
    Ok(etas_host::ModelUsage {
        input_tokens: token("input_tokens")?,
        output_tokens: token("output_tokens")?,
        cost: match required_obj(value, "cost")? {
            Value::Null => None,
            cost => Some(etas_host::ModelCostUsage {
                micros: required_str(cost, "micros")?
                    .parse()
                    .map_err(|_| InterpreterCodecError::new("invalid model usage cost"))?,
                currency: required_str(cost, "currency")?.into(),
            }),
        },
    })
}
