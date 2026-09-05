use std::sync::Arc;

use etas_core::{SourceId, Span, TextRange, TextSize};
use etas_hir::{HirBlockId, HirPatId, HirTypeId, ScopeId, SymbolId};
use serde_json::{Value, json};

use crate::{
    api::codec::value_from_json, control::Frame, orchestration::ActiveHandlerArmRecord,
    plan::SlotLayoutTable,
};

use super::value::{
    optional_u32, required, required_str, required_string_array, required_u32, required_u32_array,
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

pub(super) fn frame_from_snapshot(
    value: &Value,
    slots: Arc<SlotLayoutTable>,
) -> Result<Frame, String> {
    if slots.slot_count() == 0 {
        return frame_from_artifact_snapshot(value);
    }
    let locals = required(value, "locals")?
        .as_array()
        .ok_or_else(|| "machine frame `locals` must be an array".to_owned())?;
    let type_bindings = type_bindings_from_snapshot(value)?;
    let mut frame = Frame::with_type_bindings(slots, type_bindings);
    for local in locals {
        let symbol = SymbolId(required_u32(local, "symbol")?);
        if frame.get(symbol).is_some() {
            return Err(format!(
                "machine frame contains duplicate local symbol {}",
                symbol.0
            ));
        }
        let value =
            value_from_json(required(local, "value")?).map_err(|error| error.to_string())?;
        frame.insert(symbol, value);
    }
    Ok(frame)
}

pub(super) fn frame_from_artifact_snapshot(value: &Value) -> Result<Frame, String> {
    let locals = required(value, "locals")?
        .as_array()
        .ok_or_else(|| "machine frame `locals` must be an array".to_owned())?
        .iter()
        .map(|local| {
            Ok((
                SymbolId(required_u32(local, "symbol")?),
                value_from_json(required(local, "value")?).map_err(|error| error.to_string())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Frame::from_snapshot_with_type_bindings(locals, type_bindings_from_snapshot(value)?)
}

fn type_bindings_from_snapshot(
    value: &Value,
) -> Result<std::collections::HashMap<String, etas_types::TypeId>, String> {
    required(value, "type_bindings")?
        .as_array()
        .ok_or_else(|| "machine frame `type_bindings` must be an array".to_owned())?
        .iter()
        .map(|binding| {
            Ok((
                required(binding, "name")?
                    .as_str()
                    .ok_or_else(|| "machine frame type binding name must be a string".to_owned())?
                    .to_owned(),
                etas_types::TypeId(required_u32(binding, "type")?),
            ))
        })
        .collect()
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
