use etas_core::{SourceId, Span, TextRange, TextSize};
use etas_hir::{HirBlockId, HirPatId, ScopeId, SymbolId};
use serde_json::{Value, json};

mod graph;
pub(in crate::api::codec) use graph::{DecodeRoot, FrameDecodeCache};

pub(in crate::api::codec) fn snapshot_frame_cache(
    limits: &etas_host::StorageLimits,
    root: DecodeRoot<'_>,
) -> Result<FrameDecodeCache<LocalsSnapshot>, String> {
    graph::prepare(root, |value, cache| {
        decode_snapshot_frame(limits, value, cache)
    })
}

pub(in crate::api::codec) fn runtime_frame_cache(
    limits: &etas_host::StorageLimits,
    root: DecodeRoot<'_>,
) -> Result<FrameDecodeCache<crate::control::Frame>, String> {
    graph::prepare(root, |value, cache| {
        decode_runtime_frame(limits, value, cache)
    })
}

use crate::orchestration::{ActiveHandlerArmRecord, LocalsSnapshot};

use super::value::{
    optional_u32, required, required_str, required_string_array, required_u32, required_u32_array,
    required_u64,
};

pub(super) fn handler_arm_snapshot(handler: &ActiveHandlerArmRecord) -> Value {
    json!({
        "arm": handler.arm.0,
        "effect_segments": handler.effect_segments,
        "action": handler.action,
        "action_symbol": handler.action_symbol.map(|symbol| symbol.0),
        "patterns": handler.patterns.iter().map(|pat| pat.0).collect::<Vec<_>>(),
        "body": handler.body.0,
        "scope": handler.scope.0,
        "span": span_snapshot(handler.span),
    })
}

pub(super) fn handler_arm_from_snapshot(value: &Value) -> Result<ActiveHandlerArmRecord, String> {
    Ok(ActiveHandlerArmRecord {
        arm: etas_hir::HirHandlerArmId(required_u32(value, "arm")?),
        effect_segments: required_string_array(value, "effect_segments")?,
        action: required_str(value, "action")?.to_owned(),
        action_symbol: optional_u32(value, "action_symbol")?.map(SymbolId),
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
    snapshot_frame_cache(limits, DecodeRoot::Frame(value))?.get(value)
}

fn decode_snapshot_frame(
    limits: &etas_host::StorageLimits,
    value: &Value,
    cache: &FrameDecodeCache<LocalsSnapshot>,
) -> Result<LocalsSnapshot, String> {
    let decoded = decode_locals(limits, value, |limits, value| {
        crate::api::codec::value::decode::decode_with_frames(limits, value, cache)
            .map_err(|error| error.to_string())
    })?;
    Ok(LocalsSnapshot {
        id: decoded.id,
        locals: std::rc::Rc::new(decoded.locals),
        type_bindings: decoded.type_bindings,
        effect_row_bindings: decoded.effect_row_bindings,
    })
}

fn decode_runtime_frame(
    limits: &etas_host::StorageLimits,
    value: &Value,
    cache: &FrameDecodeCache<crate::control::Frame>,
) -> Result<crate::control::Frame, String> {
    let decoded = decode_locals(limits, value, |limits, value| {
        crate::api::codec::value::decode::decode_with_frames(limits, value, cache)
            .map_err(|error| error.to_string())
    })?;
    let mut frame = crate::control::Frame::from_snapshot_with_generic_bindings(
        decoded.locals,
        crate::control::GenericBindings {
            types: decoded.type_bindings.into_iter().collect(),
            rows: decoded.effect_row_bindings.into_iter().collect(),
        },
    )?;
    frame.set_snapshot_id(decoded.id)?;
    Ok(frame)
}

struct DecodedLocals<T> {
    id: u64,
    locals: Vec<(SymbolId, T)>,
    type_bindings: Vec<(String, etas_types::TypeId)>,
    effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
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
        effect_row_bindings: effect_rows_from_snapshot(value)?,
    })
}

pub(crate) fn effect_rows_from_snapshot(
    value: &Value,
) -> Result<Vec<(String, etas_types::EffectRowRef)>, String> {
    let rows = <Vec<(String, etas_types::EffectRowRef)> as serde::Deserialize>::deserialize(
        required(value, "effect_row_bindings")?,
    )
    .map_err(|error| format!("invalid snapshot effect-row bindings: {error}"))?;
    let mut names = std::collections::HashSet::new();
    if rows
        .iter()
        .any(|(name, _)| name.is_empty() || !names.insert(name))
    {
        return Err("invalid or duplicate snapshot effect-row binding".into());
    }
    Ok(rows)
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
