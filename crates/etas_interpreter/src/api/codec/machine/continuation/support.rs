use super::*;

pub(in crate::api::codec::machine) fn frame_snapshot(frame: &Frame) -> Value {
    json!({
        "id": frame.snapshot_id(),
        "locals": frame.sorted_locals().iter().map(|(symbol, value)| {
            json!({ "symbol": symbol.0, "value": value_json(value) })
        }).collect::<Vec<_>>(),
        "type_bindings": frame.sorted_type_bindings().iter().map(|(name, ty)| {
            json!({ "name": name, "type": ty.0 })
        }).collect::<Vec<_>>()
    })
}

pub(super) fn local_place_segments_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<Vec<crate::orchestration::LocalPlaceSegmentSnapshot>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot local place segments must be an array".to_owned())?
        .iter()
        .map(|segment| match required_str(segment, "kind")? {
            "field" => Ok(crate::orchestration::LocalPlaceSegmentSnapshot::Field(
                required_str(segment, "field")?.to_owned(),
            )),
            "index" => Ok(crate::orchestration::LocalPlaceSegmentSnapshot::Index(
                required_usize(segment, "index")?,
            )),
            "map_key" => Ok(crate::orchestration::LocalPlaceSegmentSnapshot::MapKey(
                Box::new(
                    crate::api::codec::value::snapshot_from_json_with_limits(
                        limits,
                        required(segment, "key")?,
                    )
                    .map_err(|error| error.to_string())?,
                ),
            )),
            other => Err(format!("unknown machine local place segment `{other}`")),
        })
        .collect()
}

pub(super) fn local_place_component_snapshot(
    component: &crate::orchestration::LocalPlaceComponentSnapshot,
) -> Value {
    match component {
        crate::orchestration::LocalPlaceComponentSnapshot::Field(field) => {
            json!({ "kind": "field", "field": field })
        }
        crate::orchestration::LocalPlaceComponentSnapshot::Index { base, index } => json!({
            "kind": "index",
            "base": base.0,
            "index": index.0,
        }),
    }
}

pub(super) fn local_place_components_from_snapshot(
    value: &Value,
) -> Result<Vec<crate::orchestration::LocalPlaceComponentSnapshot>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot local place components must be an array".to_owned())?
        .iter()
        .map(|component| match required_str(component, "kind")? {
            "field" => Ok(crate::orchestration::LocalPlaceComponentSnapshot::Field(
                required_str(component, "field")?.to_owned(),
            )),
            "index" => Ok(crate::orchestration::LocalPlaceComponentSnapshot::Index {
                base: HirExprId(required_u32(component, "base")?),
                index: HirExprId(required_u32(component, "index")?),
            }),
            other => Err(format!("unknown machine local place component `{other}`")),
        })
        .collect()
}

pub(super) fn map_values_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<
    Vec<(
        crate::orchestration::ValueSnapshot,
        crate::orchestration::ValueSnapshot,
    )>,
    String,
> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot map values must be an array".to_owned())?
        .iter()
        .map(|entry| {
            Ok((
                crate::api::codec::value::snapshot_from_json_with_limits(
                    limits,
                    required(entry, "key")?,
                )
                .map_err(|error| error.to_string())?,
                crate::api::codec::value::snapshot_from_json_with_limits(
                    limits,
                    required(entry, "value")?,
                )
                .map_err(|error| error.to_string())?,
            ))
        })
        .collect()
}

pub(super) fn record_values_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<Vec<(String, crate::orchestration::ValueSnapshot)>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot record values must be an array".to_owned())?
        .iter()
        .map(|entry| {
            Ok((
                required_str(entry, "name")?.to_owned(),
                crate::api::codec::value::snapshot_from_json_with_limits(
                    limits,
                    required(entry, "value")?,
                )
                .map_err(|error| error.to_string())?,
            ))
        })
        .collect()
}

pub(super) fn slice_eval_snapshot(eval: &crate::orchestration::SliceExprEvalSnapshot) -> Value {
    json!({
        "expr": eval.expr.0,
        "base": eval.base.0,
        "start": eval.start.0,
        "end": eval.end.0,
        "bounds": range_bounds_name(eval.bounds),
        "span": span_snapshot(eval.span),
    })
}

pub(super) fn slice_eval_from_snapshot(
    value: &Value,
) -> Result<crate::orchestration::SliceExprEvalSnapshot, String> {
    Ok(crate::orchestration::SliceExprEvalSnapshot {
        expr: HirExprId(required_u32(value, "expr")?),
        base: HirExprId(required_u32(value, "base")?),
        start: HirExprId(required_u32(value, "start")?),
        end: HirExprId(required_u32(value, "end")?),
        bounds: range_bounds_from_name(required_str(value, "bounds")?)?,
        span: span_from_snapshot(required(value, "span")?)?,
    })
}

pub(super) fn else_branch_snapshot(branch: &HirElseBranch) -> Value {
    match branch {
        HirElseBranch::If(expr) => json!({ "kind": "if", "expr": expr.0 }),
        HirElseBranch::Block(block) => json!({ "kind": "block", "block": block.0 }),
    }
}

pub(super) fn optional_else_branch(
    value: &Value,
    field: &str,
) -> Result<Option<HirElseBranch>, String> {
    let Some(value) = value.get(field) else {
        return Err(format!("machine snapshot is missing `{field}`"));
    };
    if value.is_null() {
        return Ok(None);
    }
    match required_str(value, "kind")? {
        "if" => Ok(Some(HirElseBranch::If(HirExprId(required_u32(
            value, "expr",
        )?)))),
        "block" => Ok(Some(HirElseBranch::Block(HirBlockId(required_u32(
            value, "block",
        )?)))),
        other => Err(format!("unknown machine else branch `{other}`")),
    }
}

pub(super) fn match_arm_snapshot(arm: &HirMatchArm) -> Value {
    let (body_kind, body) = match arm.body {
        HirMatchArmBody::Expr(expr) => ("expr", expr.0),
        HirMatchArmBody::Block(block) => ("block", block.0),
    };
    json!({
        "pat": arm.pat.0,
        "body_kind": body_kind,
        "body": body,
        "scope": arm.scope.0,
        "span": span_snapshot(arm.span),
    })
}

pub(super) fn match_arms_from_snapshot(value: &Value) -> Result<Vec<HirMatchArm>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot match arms must be an array".to_owned())?
        .iter()
        .map(|arm| {
            let body = required_u32(arm, "body")?;
            Ok(HirMatchArm {
                pat: HirPatId(required_u32(arm, "pat")?),
                body: match required_str(arm, "body_kind")? {
                    "expr" => HirMatchArmBody::Expr(HirExprId(body)),
                    "block" => HirMatchArmBody::Block(HirBlockId(body)),
                    other => return Err(format!("unknown machine match arm body `{other}`")),
                },
                scope: ScopeId(required_u32(arm, "scope")?),
                span: span_from_snapshot(required(arm, "span")?)?,
            })
        })
        .collect()
}

pub(super) fn arg_snapshot(arg: &HirArg) -> Value {
    match arg {
        HirArg::Positional(expr) => json!({ "kind": "positional", "expr": expr.0 }),
        HirArg::Named { name, value, span } => json!({
            "kind": "named",
            "name": name,
            "value": value.0,
            "span": span_snapshot(*span),
        }),
    }
}

pub(super) fn args_from_snapshot(value: &Value) -> Result<Vec<HirArg>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot args must be an array".to_owned())?
        .iter()
        .map(|arg| match required_str(arg, "kind")? {
            "positional" => Ok(HirArg::Positional(HirExprId(required_u32(arg, "expr")?))),
            "named" => Ok(HirArg::Named {
                name: required_str(arg, "name")?.to_owned(),
                value: HirExprId(required_u32(arg, "value")?),
                span: span_from_snapshot(required(arg, "span")?)?,
            }),
            other => Err(format!("unknown machine argument `{other}`")),
        })
        .collect()
}

pub(super) fn stage_snapshot(stage: &HirStage) -> Value {
    json!({
        "expr": stage.expr.0,
        "limits": stage.limits.iter().map(|expr| expr.0).collect::<Vec<_>>(),
        "span": span_snapshot(stage.span),
    })
}

pub(super) fn stages_from_snapshot(value: &Value) -> Result<Vec<HirStage>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot stages must be an array".to_owned())?
        .iter()
        .map(|stage| {
            Ok(HirStage {
                expr: HirExprId(required_u32(stage, "expr")?),
                limits: required_u32_array(stage, "limits")?
                    .into_iter()
                    .map(HirExprId)
                    .collect(),
                span: span_from_snapshot(required(stage, "span")?)?,
            })
        })
        .collect()
}

pub(super) fn prompt_message_snapshot(message: &crate::value::PromptMessage) -> Value {
    json!({
        "role": prompt_role_name(message.role),
        "text": message.text,
        "trust": message.trust.map(trust_wrapper_name),
    })
}

pub(super) fn prompt_messages_from_snapshot(
    value: &Value,
) -> Result<crate::value::PromptValue, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot prompt messages must be an array".to_owned())?
        .iter()
        .map(|message| {
            Ok(crate::value::PromptMessage {
                role: prompt_role_from_name(required_str(message, "role")?)?,
                text: required_str(message, "text")?.into(),
                trust: optional_string(message, "trust")?
                    .map(|name| trust_wrapper_from_name(&name))
                    .transpose()?,
            })
        })
        .collect()
}

pub(super) fn static_method_kind_snapshot(
    kind: &crate::orchestration::StaticMethodKindSnapshot,
) -> Value {
    match kind {
        crate::orchestration::StaticMethodKindSnapshot::Prompt => json!({ "kind": "prompt" }),
        crate::orchestration::StaticMethodKindSnapshot::Message => json!({ "kind": "message" }),
        crate::orchestration::StaticMethodKindSnapshot::SessionConfig => {
            json!({ "kind": "session_config" })
        }
        crate::orchestration::StaticMethodKindSnapshot::Conversation => {
            json!({ "kind": "conversation" })
        }
        crate::orchestration::StaticMethodKindSnapshot::Range => json!({ "kind": "range" }),
        crate::orchestration::StaticMethodKindSnapshot::AdvancedCollection(name) => {
            json!({ "kind": "advanced_collection", "name": name })
        }
    }
}

pub(super) fn static_method_kind_from_snapshot(
    value: &Value,
) -> Result<StaticMethodKindSnapshot, String> {
    match required_str(value, "kind")? {
        "prompt" => Ok(StaticMethodKindSnapshot::Prompt),
        "message" => Ok(StaticMethodKindSnapshot::Message),
        "session_config" => Ok(StaticMethodKindSnapshot::SessionConfig),
        "conversation" => Ok(StaticMethodKindSnapshot::Conversation),
        "range" => Ok(StaticMethodKindSnapshot::Range),
        "advanced_collection" => Ok(StaticMethodKindSnapshot::AdvancedCollection(
            required_str(value, "name")?.to_owned(),
        )),
        other => Err(format!("unknown machine static method kind `{other}`")),
    }
}

pub(in crate::api::codec) fn runtime_limit_snapshot(
    limit: &crate::eval::limit::RuntimeLimit,
) -> Value {
    let value = match &limit.value {
        crate::eval::limit::RuntimeLimitValue::Count(value) => {
            json!({ "kind": "count", "value": value })
        }
        crate::eval::limit::RuntimeLimitValue::DurationMillis(value) => {
            json!({ "kind": "duration_millis", "value": value })
        }
        crate::eval::limit::RuntimeLimitValue::MoneyMicros { amount, currency } => json!({
            "kind": "money_micros",
            "amount": amount.to_string(),
            "currency": currency,
        }),
    };
    json!({
        "limit_kind": std_limit_kind_name(limit.kind),
        "value": value,
        "span": span_snapshot(limit.span),
    })
}

pub(in crate::api::codec::machine) fn runtime_limits_from_snapshot(
    value: &Value,
) -> Result<Vec<crate::eval::limit::RuntimeLimit>, String> {
    value
        .as_array()
        .ok_or_else(|| "machine snapshot runtime limits must be an array".to_owned())?
        .iter()
        .map(|limit| {
            let raw_value = required(limit, "value")?;
            let value = match required_str(raw_value, "kind")? {
                "count" => {
                    crate::eval::limit::RuntimeLimitValue::Count(required_u64(raw_value, "value")?)
                }
                "duration_millis" => crate::eval::limit::RuntimeLimitValue::DurationMillis(
                    required_u64(raw_value, "value")?,
                ),
                "money_micros" => crate::eval::limit::RuntimeLimitValue::MoneyMicros {
                    amount: required_str(raw_value, "amount")?
                        .parse::<u128>()
                        .map_err(|_| "machine money amount must be a u128".to_owned())?,
                    currency: required_str(raw_value, "currency")?.to_owned(),
                },
                other => return Err(format!("unknown machine runtime limit value `{other}`")),
            };
            Ok(crate::eval::limit::RuntimeLimit {
                kind: std_limit_kind_from_name(required_str(limit, "limit_kind")?)?,
                value,
                span: span_from_snapshot(required(limit, "span")?)?,
            })
        })
        .collect()
}

pub(super) fn unary_op_name(op: etas_hir::HirUnaryOp) -> &'static str {
    match op {
        etas_hir::HirUnaryOp::Not => "not",
        etas_hir::HirUnaryOp::Neg => "neg",
    }
}

pub(super) fn range_bounds_name(bounds: HirRangeBounds) -> &'static str {
    match bounds {
        HirRangeBounds::ClosedOpen => "closed_open",
        HirRangeBounds::OpenClosed => "open_closed",
    }
}

pub(super) fn range_bounds_from_name(name: &str) -> Result<HirRangeBounds, String> {
    match name {
        "closed_open" => Ok(HirRangeBounds::ClosedOpen),
        "open_closed" => Ok(HirRangeBounds::OpenClosed),
        _ => Err(format!("unknown machine range bounds `{name}`")),
    }
}

pub(super) fn unary_op_from_name(name: &str) -> Result<etas_hir::HirUnaryOp, String> {
    match name {
        "not" => Ok(etas_hir::HirUnaryOp::Not),
        "neg" => Ok(etas_hir::HirUnaryOp::Neg),
        _ => Err(format!("unknown machine unary operator `{name}`")),
    }
}

pub(super) fn binary_op_name(op: etas_hir::HirBinaryOp) -> &'static str {
    match op {
        etas_hir::HirBinaryOp::EqEq => "eq",
        etas_hir::HirBinaryOp::BangEq => "ne",
        etas_hir::HirBinaryOp::Lt => "lt",
        etas_hir::HirBinaryOp::LtEq => "le",
        etas_hir::HirBinaryOp::Gt => "gt",
        etas_hir::HirBinaryOp::GtEq => "ge",
        etas_hir::HirBinaryOp::Add => "add",
        etas_hir::HirBinaryOp::Sub => "sub",
        etas_hir::HirBinaryOp::Mul => "mul",
        etas_hir::HirBinaryOp::Div => "div",
        etas_hir::HirBinaryOp::Rem => "rem",
        etas_hir::HirBinaryOp::AndAnd => "and",
        etas_hir::HirBinaryOp::OrOr => "or",
    }
}

pub(super) fn binary_op_from_name(name: &str) -> Result<etas_hir::HirBinaryOp, String> {
    match name {
        "eq" => Ok(etas_hir::HirBinaryOp::EqEq),
        "ne" => Ok(etas_hir::HirBinaryOp::BangEq),
        "lt" => Ok(etas_hir::HirBinaryOp::Lt),
        "le" => Ok(etas_hir::HirBinaryOp::LtEq),
        "gt" => Ok(etas_hir::HirBinaryOp::Gt),
        "ge" => Ok(etas_hir::HirBinaryOp::GtEq),
        "add" => Ok(etas_hir::HirBinaryOp::Add),
        "sub" => Ok(etas_hir::HirBinaryOp::Sub),
        "mul" => Ok(etas_hir::HirBinaryOp::Mul),
        "div" => Ok(etas_hir::HirBinaryOp::Div),
        "rem" => Ok(etas_hir::HirBinaryOp::Rem),
        "and" => Ok(etas_hir::HirBinaryOp::AndAnd),
        "or" => Ok(etas_hir::HirBinaryOp::OrOr),
        _ => Err(format!("unknown machine binary operator `{name}`")),
    }
}
