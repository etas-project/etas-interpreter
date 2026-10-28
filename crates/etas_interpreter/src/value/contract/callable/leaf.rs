use super::*;
use etas_frontend::CheckedCallableRoot;
use etas_types::{CallableSignature, ItemSignature, SymbolTypeFact};

fn flow(signature: &CallableSignature) -> FlowType {
    FlowType {
        input: signature.params.clone(),
        output: signature.output,
        effects: signature.effects.clone(),
    }
}

fn substitute(
    checked: &CheckedProject,
    signature: FlowType,
    bindings: &Bindings,
) -> Result<FlowType, String> {
    let mut substitution = bindings.substitution(&checked.type_store);
    Ok(FlowType {
        input: signature
            .input
            .into_iter()
            .map(|ty| {
                substitution
                    .substitute(ty)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?,
        output: substitution
            .substitute(signature.output)
            .map_err(|error| error.to_string())?,
        effects: signature
            .effects
            .map(|row| {
                substitution
                    .substitute_effect_row(row)
                    .map_err(|error| error.to_string())
            })
            .transpose()?,
    })
}

pub(super) fn interface(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    target: &Target,
    bindings: &Bindings,
    allowed: &Option<EffectRowRef>,
    specs: &etas_types::SpecFacts,
) -> Result<Interface, String> {
    let (root, signature, component, bindings) = match target {
        Target::FlowItem(item) | Target::ToolItem(item) => {
            let signature = match (target, checked.types.item_signatures.get(item)) {
                (Target::FlowItem(_), Some(ItemSignature::Flow(signature)))
                | (Target::ToolItem(_), Some(ItemSignature::Tool(signature))) => signature,
                _ => return Err("callable kind differs from its checked item".into()),
            };
            (
                CheckedCallableRoot::Item(*item),
                flow(signature),
                None,
                bindings.clone(),
            )
        }
        Target::AgentItem(item) => {
            return agent_interface(checked, *item, bindings, allowed, specs);
        }
        Target::AgentRevision(agent) => {
            if !std::sync::Arc::ptr_eq(agent.source_program(), checked) || !agent.is_default_entry()
            {
                return Err("agent input belongs to a different checked program or method".into());
            }
            return agent_interface(checked, agent.source_agent()?, bindings, allowed, specs);
        }
        Target::SpecImplMethod(symbol) => {
            let Some(etas_hir::SymbolDef::Item { item }) =
                checked.symbols.get(*symbol).map(|symbol| &symbol.def)
            else {
                return Err("spec method input has no checked item".into());
            };
            let Some(ItemSignature::Flow(signature)) = checked.types.item_signatures.get(item)
            else {
                return Err("spec method input has no checked flow signature".into());
            };
            (
                CheckedCallableRoot::Item(*item),
                flow(signature),
                None,
                bindings.clone(),
            )
        }
        Target::Lambda { expr, captured } => {
            if plan.closures.get(*expr).is_none() {
                return Err("lambda input has no checked capture layout".into());
            }
            let Some(Type::Function(signature)) = checked
                .types
                .expr_types
                .get(expr)
                .and_then(|ty| checked.type_store.get(*ty))
            else {
                return Err("lambda input has no checked signature".into());
            };
            let mut bindings = bindings.clone();
            for (name, ty) in captured.generic_bindings().iter() {
                bindings.entry(name.clone()).or_insert(*ty);
            }
            for (name, row) in &captured.generic_bindings().rows {
                bindings
                    .rows
                    .entry(name.clone())
                    .or_insert_with(|| row.clone());
            }
            (
                CheckedCallableRoot::Lambda(*expr),
                signature.clone(),
                None,
                bindings,
            )
        }
        Target::PureIntrinsic(call) => {
            let root = plan.dispatch.intrinsic_callable_root(
                crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    intrinsic: call.intrinsic,
                    dispatch: etas_std::IntrinsicDispatch::PureKernel,
                },
                &call.parameter_types,
                call.result_type,
            )?;
            (
                root,
                FlowType {
                    input: call.parameter_types.clone(),
                    output: call.result_type,
                    effects: None,
                },
                None,
                bindings.clone(),
            )
        }
        Target::StdIntrinsic(call) => {
            let root = plan.dispatch.std_callable_root(call)?;
            let mut bindings = bindings.clone();
            for step in &call.substitutions {
                bindings.extend(step.iter().cloned());
            }
            (
                root,
                FlowType {
                    input: call.parameter_types.clone(),
                    output: call.result_type,
                    effects: None,
                },
                None,
                bindings,
            )
        }
        Target::Scoped(value) => {
            if !std::sync::Arc::ptr_eq(value.presented_program(), checked) {
                return Err("scoped callable input belongs to a different checked program".into());
            }
            let signature = value.presented_signature()?;
            row_compatible(checked, signature, allowed)?;
            return Ok(Interface {
                signature: Some(signature.clone()),
                component: None,
            });
        }
        Target::EnumVariant(symbol) => {
            let Some(SymbolTypeFact::Flow { signature }) = checked.types.symbol_types.get(symbol)
            else {
                return Err("constructor input has no checked signature".into());
            };
            let span = checked
                .symbols
                .get(*symbol)
                .ok_or("constructor input symbol is missing")?
                .definition_span;
            crate::value::validate_snapshot_bounds(checked, signature, span, bindings, specs)?;
            return Ok(Interface {
                signature: Some(substitute(checked, flow(signature), bindings)?),
                component: None,
            });
        }
        Target::NominalConstructor(ty) => {
            let representation = crate::plan::code::CheckedValueTypes::default()
                .representation(checked, *ty)
                .map_err(|error| {
                    format!("constructor input has no checked representation: {error:?}")
                })?;
            return Ok(Interface {
                signature: Some(FlowType {
                    input: vec![representation],
                    output: *ty,
                    effects: None,
                }),
                component: Some(*ty),
            });
        }
        Target::Composed(_) | Target::Specialized { .. } | Target::Limited { .. } => {
            return Err("callable input has an unprocessed wrapper".into());
        }
    };
    let signature = substitute(checked, signature, &bindings)?;
    crate::value::validate_snapshot_signature(
        checked,
        root,
        &FlowType {
            effects: allowed.clone(),
            ..signature.clone()
        },
        &bindings,
        specs,
    )?;
    Ok(Interface {
        signature: Some(signature),
        component,
    })
}

fn agent_interface(
    checked: &CheckedProject,
    item: etas_hir::HirItemId,
    bindings: &Bindings,
    allowed: &Option<EffectRowRef>,
    specs: &etas_types::SpecFacts,
) -> Result<Interface, String> {
    let Some(ItemSignature::Agent(agent)) = checked.types.item_signatures.get(&item) else {
        return Err("agent input has no checked component identity".into());
    };
    let signature = agent
        .callable()
        .map(|signature| substitute(checked, flow(signature), bindings))
        .transpose()?;
    if let Some(signature) = &signature {
        crate::value::validate_snapshot_signature(
            checked,
            CheckedCallableRoot::Item(item),
            &FlowType {
                effects: allowed.clone(),
                ..signature.clone()
            },
            bindings,
            specs,
        )?;
    }
    Ok(Interface {
        signature,
        component: Some(agent.component),
    })
}

fn row_compatible(
    checked: &CheckedProject,
    signature: &FlowType,
    allowed: &Option<EffectRowRef>,
) -> Result<(), String> {
    if let Some(allowed) = allowed {
        let actual = signature
            .effects
            .as_ref()
            .ok_or("scoped callable input has no checked effect row")?;
        let row = |row| {
            etas_effects::validate::resolve_checked_type_row(
                &checked.type_store,
                &checked.effect_registry,
                row,
            )
        };
        if !(etas_effects::EffectCoverage {
            registry: &checked.effect_registry,
            types: &checked.type_store,
        })
        .row_covers(&row(allowed)?, &row(actual)?)
        {
            return Err("callable input effects exceed its checked slot".into());
        }
    }
    Ok(())
}
