use etas_effects::{Effect, EffectCoverage};
use etas_types::FlowType;

pub(super) fn validate_snapshot_effects(
    checked: &etas_frontend::CheckedProgram,
    root: etas_frontend::CheckedCallableRoot,
    signature: &FlowType,
    bindings: &crate::control::GenericBindings,
) -> Result<(), String> {
    let summary = checked
        .checked_callable_effects(root)
        .map_err(|error| format!("checkpoint callable has no checked effect summary: {error}"))?;
    let Some(expected) = &signature.effects else {
        return Ok(());
    };
    let mut substitution = bindings.substitution(&checked.type_store);
    let expected = substitution
        .substitute_effect_row(expected.clone())
        .map_err(|error| error.to_string())?;
    let allowed = etas_effects::validate::resolve_checked_type_row(
        &checked.type_store,
        &checked.effect_registry,
        &expected,
    )?;
    let mut actual = summary.escaping_effects.clone();
    let mut effects = etas_effects::EffectSet::new();
    for effect in actual.effects.iter() {
        effects.insert(match effect {
            Effect::Error(ty) => Effect::Error(
                substitution
                    .substitute(*ty)
                    .map_err(|error| error.to_string())?,
            ),
            Effect::Applied { tag, args } => Effect::Applied {
                tag: *tag,
                args: args
                    .iter()
                    .map(|ty| {
                        substitution
                            .substitute(*ty)
                            .map_err(|error| error.to_string())
                    })
                    .collect::<Result<_, _>>()?,
            },
            Effect::AppliedAction(action) => {
                let mut action = action.clone();
                for arg in &mut action.args {
                    if let etas_types::EffectArgRef::Type(ty) = arg {
                        *ty = substitution
                            .substitute(*ty)
                            .map_err(|error| error.to_string())?;
                    }
                }
                Effect::AppliedAction(action)
            }
            other => other.clone(),
        });
    }
    actual.effects = effects;
    if let Some(open) = actual.open {
        for (name, row) in &bindings.rows {
            if etas_effects::effect_var_id_from_name(name) == open {
                let row = substitution
                    .substitute_effect_row(row.clone())
                    .map_err(|error| error.to_string())?;
                let bound = etas_effects::validate::resolve_checked_type_row(
                    &checked.type_store,
                    &checked.effect_registry,
                    &row,
                )?;
                actual.effects.union_assign(&bound.effects);
                actual.open = bound.open;
                break;
            }
        }
    }
    if !(EffectCoverage {
        registry: &checked.effect_registry,
        types: &checked.type_store,
    })
    .row_covers(&allowed, &actual)
    {
        return Err(
            "checkpoint native callable effects exceed its checked function interface".into(),
        );
    }
    Ok(())
}
