use etas_core::{SourceId, Span, TextRange, TextSize};
use etas_hir::{HirBlockId, HirPatId, HirTypeId, ScopeId, SymbolId};
use serde_json::{Value, json};

use crate::{
    api::codec::value::snapshot_from_json_with_limits,
    orchestration::{ActiveHandlerArmRecord, LocalsSnapshot},
};

use super::value::{
    optional_u32, required, required_str, required_string_array, required_u32, required_u32_array,
    required_u64,
};

pub(super) fn handler_arm_snapshot(handler: &ActiveHandlerArmRecord) -> Value {
    json!({
        "effect_segments": handler.effect_segments,
        "action": handler.action,
        "action_symbol": handler.action_symbol.map(|symbol| symbol.0),
        "type_args": handler.type_args.iter().map(|ty| ty.0).collect::<Vec<_>>(),
        "effect_type_args": handler.effect_type_args.iter().map(|ty| ty.0).collect::<Vec<_>>(),
        "patterns": handler.patterns.iter().map(|pat| pat.0).collect::<Vec<_>>(),
        "body": handler.body.0,
        "scope": handler.scope.0,
        "span": span_snapshot(handler.span),
    })
}

pub(super) fn handler_arm_from_snapshot(value: &Value) -> Result<ActiveHandlerArmRecord, String> {
    Ok(ActiveHandlerArmRecord {
        effect_segments: required_string_array(value, "effect_segments")?,
        action: required_str(value, "action")?.to_owned(),
        action_symbol: optional_u32(value, "action_symbol")?.map(SymbolId),
        type_args: required_u32_array(value, "type_args")?
            .into_iter()
            .map(HirTypeId)
            .collect(),
        effect_type_args: required_u32_array(value, "effect_type_args")?
            .into_iter()
            .map(etas_types::TypeId)
            .collect(),
        patterns: required_u32_array(value, "patterns")?
            .into_iter()
            .map(HirPatId)
            .collect(),
        body: HirBlockId(required_u32(value, "body")?),
        scope: ScopeId(required_u32(value, "scope")?),
        span: span_from_snapshot(required(value, "span")?)?,
    })
}

pub(super) fn locals_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<LocalsSnapshot, String> {
    let decoded = decode_locals(limits, value, |limits, value| {
        snapshot_from_json_with_limits(limits, value).map_err(|error| error.to_string())
    })?;
    Ok(LocalsSnapshot {
        id: decoded.id,
        locals: std::rc::Rc::new(decoded.locals),
        type_bindings: decoded.type_bindings,
    })
}

pub(super) fn runtime_frame_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<crate::control::Frame, String> {
    let decoded = decode_locals(limits, value, |limits, value| {
        crate::api::codec::value_from_json_with_limits(limits, value)
            .map_err(|error| error.to_string())
    })?;
    let mut frame = crate::control::Frame::from_snapshot_with_type_bindings(
        decoded.locals,
        decoded.type_bindings.into_iter().collect(),
    )?;
    frame.set_snapshot_id(decoded.id)?;
    Ok(frame)
}

struct DecodedLocals<T> {
    id: u64,
    locals: Vec<(SymbolId, T)>,
    type_bindings: Vec<(String, etas_types::TypeId)>,
}

fn decode_locals<T>(
    limits: &etas_host::StorageLimits,
    value: &Value,
    mut decode: impl FnMut(&etas_host::StorageLimits, &Value) -> Result<T, String>,
) -> Result<DecodedLocals<T>, String> {
    let id = required_u64(value, "id")?;
    if id == 0 {
        return Err("snapshot frame identity must be nonzero".into());
    }
    let entries = required(value, "locals")?
        .as_array()
        .ok_or_else(|| "machine frame `locals` must be an array".to_owned())?;
    let mut locals = Vec::with_capacity(entries.len());
    for local in entries {
        locals.push((
            SymbolId(required_u32(local, "symbol")?),
            decode(limits, required(local, "value")?)?,
        ));
    }
    locals.sort_unstable_by_key(|(symbol, _)| *symbol);
    if let Some(pair) = locals.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(format!(
            "machine frame contains duplicate local symbol {}",
            pair[0].0.0
        ));
    }
    let mut type_bindings = required(value, "type_bindings")?
        .as_array()
        .ok_or_else(|| "machine frame `type_bindings` must be an array".to_owned())?
        .iter()
        .map(|binding| {
            Ok((
                required_str(binding, "name")?.to_owned(),
                etas_types::TypeId(required_u32(binding, "type")?),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    type_bindings.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    if let Some(pair) = type_bindings.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(format!(
            "duplicate machine frame type binding `{}`",
            pair[0].0
        ));
    }
    Ok(DecodedLocals {
        id,
        locals,
        type_bindings,
    })
}

pub(super) fn span_snapshot(span: Span) -> Value {
    json!({
        "source": span.source.0,
        "start": span.range.start.0,
        "end": span.range.end.0,
    })
}

pub(super) fn span_from_snapshot(value: &Value) -> Result<Span, String> {
    Ok(Span::new(
        SourceId(required_u32(value, "source")?),
        TextRange::new(
            TextSize(required_u32(value, "start")?),
            TextSize(required_u32(value, "end")?),
        ),
    ))
}
