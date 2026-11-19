use super::{ExpressionProjection, PreparedExpression};
use crate::{
    control::{CheckedCallBindings, GenericBindings},
    plan::code::RevisionProjection,
    value::CheckedExecutionProgram,
};
use etas_code::descriptor::{GenericOwner, ParameterDefaultInfo, ParameterDefaultOwner};
use etas_frontend::{CheckedProgram, code::parameter_defaults::ParameterDefaultUnit};
use etas_hir::{HirExprId, HirItem};
use etas_types::{CallableGenericParam, CallableGenericParamKind, ItemSignature, SymbolTypeFact};
use std::sync::Arc;

pub(super) fn prepare(
    source: &CheckedExecutionProgram,
    reference: &ParameterDefaultInfo,
    expr: HirExprId,
    slots: &[u32],
    outer: &GenericBindings,
) -> Result<PreparedExpression, String> {
    let library = source
        .plan
        .standard_library
        .as_ref()
        .ok_or("default expression has no loaded defining program")?;
    let unit = library
        .parameter_default(reference)
        .ok_or("imported default has no matching authored expression revision")?;
    let mut destination = library.default_program(reference)?;
    let edge = RevisionProjection::Original(library.bridge().clone());
    let scope = match &reference.owner {
        ParameterDefaultOwner::Declaration(owner) => GenericOwner::Declaration(owner.clone()),
        ParameterDefaultOwner::Method(owner) => GenericOwner::Method(owner.clone()),
        ParameterDefaultOwner::SpecMethod { spec, .. } => GenericOwner::Declaration(spec.clone()),
        ParameterDefaultOwner::Action { effect, name } => GenericOwner::Action {
            effect: effect.clone(),
            name: name.clone(),
        },
    };
    let bindings = defining_bindings(
        source,
        &destination.checked,
        unit,
        reference,
        expr,
        outer,
        &edge,
        &scope,
    )?;
    let parameters = destination
        .checked
        .hir
        .preceding_parameters(unit.parameter)
        .ok_or("imported default has no defining parameter list")?;
    let mut seen = std::collections::BTreeSet::new();
    let mut substitution = bindings.substitution(&destination.checked.type_store);
    let mut arguments = Vec::with_capacity(slots.len());
    for slot in slots {
        let parameter = parameters
            .get(*slot as usize)
            .filter(|_| seen.insert(*slot))
            .ok_or("imported default has inconsistent preceding argument positions")?;
        let Some(SymbolTypeFact::Param { ty }) =
            destination.checked.types.symbol_types.get(parameter)
        else {
            return Err("imported default argument has no declared type".into());
        };
        arguments.push(
            substitution
                .substitute(*ty)
                .map_err(|error| error.to_string())?,
        );
    }
    let output = substitution
        .substitute(unit.output)
        .map_err(|error| error.to_string())?;
    if destination.checked.foreign_methods().next().is_some() {
        destination.foreign = Some(Arc::new(crate::value::ForeignExecutionLink {
            owner: source.clone(),
            projection: edge.clone(),
        }));
    }
    Ok(PreparedExpression {
        handler_projections: Vec::new(),
        destination,
        bindings,
        output,
        projection: Some(ExpressionProjection {
            edge,
            scope,
            arguments,
        }),
    })
}

fn defining_bindings(
    source: &CheckedExecutionProgram,
    native: &CheckedProgram,
    unit: &ParameterDefaultUnit,
    reference: &ParameterDefaultInfo,
    expr: HirExprId,
    outer: &GenericBindings,
    outgoing: &RevisionProjection,
    scope: &GenericOwner,
) -> Result<GenericBindings, String> {
    let inferred = CheckedCallBindings::resolve(&source.checked, expr, outer)?;
    let incoming = outgoing.reversed();
    let mut bindings = GenericBindings::new();
    // Impl binders and method binders are different scopes. Their checked
    // parameter correspondences preserve alpha-renaming across the package.
    if !matches!(reference.owner, ParameterDefaultOwner::SpecMethod { .. }) {
        for (parameter, argument) in &inferred.parameters {
            bindings.parameters.insert(
                incoming.type_in_scope(scope, *parameter)?,
                incoming.type_in_scope(scope, *argument)?,
            );
        }
        if let Some(method) = native
            .types
            .type_impl_methods
            .values()
            .find(|method| method.body == unit.owner)
        {
            for parameter in &method.owner_params {
                if let Some(argument) = bindings.parameters.get(&parameter.subject).copied() {
                    bindings.types.insert(parameter.name.clone(), argument);
                }
            }
        }
    }
    let (from, _) = presented_interface(&source.checked, reference, expr)?;
    let to = native_generics(native, unit)?;
    if from.len() != to.len() || from.iter().zip(to).any(|(from, to)| from.kind != to.kind) {
        return Err("imported default has a different checked generic interface".into());
    }
    for (from, to) in from.iter().zip(to) {
        match to.kind {
            CallableGenericParamKind::Type => {
                let argument = inferred
                    .types
                    .iter()
                    .find(|(name, _)| name == &from.name)
                    .map(|(_, argument)| *argument)
                    .ok_or("default invocation has no inferred type argument")?;
                let argument = incoming.type_in_scope(scope, argument)?;
                bindings.types.insert(to.name.clone(), argument);
                bindings.parameters.insert(to.subject, argument);
            }
            CallableGenericParamKind::Effect => {
                let row = inferred
                    .rows
                    .iter()
                    .find(|(name, _)| name == &from.name)
                    .map(|(_, row)| row.clone())
                    .ok_or("default invocation has no inferred Effect row")?;
                bindings
                    .rows
                    .insert(to.name.clone(), incoming.effect_row(row)?);
            }
        }
    }
    if let HirItem::Spec(spec) = &native.hir.items[unit.owner] {
        let signature = spec
            .items
            .iter()
            .find_map(|item| match item {
                etas_hir::HirSpecItem::FlowSignature(signature)
                    if signature.params.contains(&unit.parameter) =>
                {
                    Some(signature)
                }
                _ => None,
            })
            .ok_or("default has no defining spec method")?;
        let method = source
            .checked
            .types
            .checked_method_calls
            .get(&expr)
            .ok_or("imported spec default has no checked method evidence")?;
        let (subject, args) = match &method.evidence {
            etas_types::method::MethodEvidence::Implementation { subject, args, .. }
            | etas_types::method::MethodEvidence::Bound { subject, args, .. } => (*subject, args),
            etas_types::method::MethodEvidence::Inherent => {
                return Err("imported spec default has no spec evidence".into());
            }
        };
        if spec.type_params.len() != args.len() {
            return Err("imported spec default has a different checked spec arity".into());
        }
        let mut caller_bindings = outer.clone();
        caller_bindings.types.extend(inferred.types);
        caller_bindings.parameters.extend(inferred.parameters);
        caller_bindings.rows.extend(inferred.rows);
        let mut substitution = caller_bindings.substitution(&source.checked.type_store);
        for (symbol, argument) in spec.type_params.iter().zip(args) {
            let argument = substitution
                .substitute(*argument)
                .map_err(|error| error.to_string())?;
            crate::eval::arguments::bind_source_parameter(
                native,
                &mut bindings,
                *symbol,
                incoming.type_in_scope(scope, argument)?,
            );
        }
        if let Some(receiver) = signature.receiver_type_param {
            let subject = substitution
                .substitute(subject)
                .map_err(|error| error.to_string())?;
            crate::eval::arguments::bind_source_parameter(
                native,
                &mut bindings,
                receiver,
                incoming.type_in_scope(scope, subject)?,
            );
        }
    }
    Ok(bindings)
}

pub(super) fn presented_parameter_type(
    checked: &CheckedProgram,
    reference: &ParameterDefaultInfo,
    expr: HirExprId,
    slot: u32,
    outer: &GenericBindings,
) -> Result<etas_types::TypeId, String> {
    let (_, parameters) = presented_interface(checked, reference, expr)?;
    let ty = *parameters
        .get(slot as usize)
        .ok_or("default result has no caller parameter type")?;
    let inferred = CheckedCallBindings::resolve(checked, expr, outer)?;
    let mut bindings = outer.clone();
    bindings.types.extend(inferred.types);
    bindings.parameters.extend(inferred.parameters);
    bindings.rows.extend(inferred.rows);
    bindings
        .substitution(&checked.type_store)
        .substitute(ty)
        .map_err(|error| error.to_string())
}

fn presented_interface<'a>(
    checked: &'a CheckedProgram,
    reference: &ParameterDefaultInfo,
    expr: HirExprId,
) -> Result<(&'a [CallableGenericParam], &'a [etas_types::TypeId]), String> {
    if let Some(method) = checked.types.checked_method_calls.get(&expr) {
        return Ok((&method.signature.generic_params, &method.signature.params));
    }
    if let Some(etas_hir::HirExpr::Perform { action, .. }) = checked.hir.exprs.get(expr) {
        if let Some(selected) = checked.types.action_calls.get(&expr) {
            return Ok((
                &selected.signature.generic_params,
                &selected.signature.params,
            ));
        }
        let etas_hir::ResolveResult::Resolved(symbol) = action.action_symbol else {
            return Err("default invocation has no checked action declaration".into());
        };
        let signature = checked
            .types
            .action_signatures
            .get(&symbol)
            .ok_or("default invocation has no imported action signature")?;
        return Ok((&signature.generic_params, &signature.params));
    }
    let owns = |parameters: Option<&[etas_types::ParameterInterface]>| {
        parameters.is_some_and(|parameters| parameters.iter().any(|parameter| {
            matches!(&parameter.default, Some(etas_types::ParameterDefaultRef::External { reference: candidate }) if candidate == reference)
        }))
    };
    for fact in checked.types.symbol_types.values() {
        let signature = match fact {
            SymbolTypeFact::Flow { signature } | SymbolTypeFact::Tool { signature } => {
                Some(signature)
            }
            SymbolTypeFact::Agent { signature } => signature.callable(),
            SymbolTypeFact::EffectAction { signature }
                if owns(signature.parameter_interface.as_deref()) =>
            {
                return Ok((&signature.generic_params, &signature.params));
            }
            _ => None,
        };
        if let Some(signature) =
            signature.filter(|signature| owns(signature.parameter_interface.as_deref()))
        {
            return Ok((&signature.generic_params, &signature.params));
        }
    }
    checked
        .types
        .std_callable_signatures
        .values()
        .find(|signature| owns(signature.parameter_interface.as_deref()))
        .map(|signature| {
            (
                signature.generic_params.as_slice(),
                signature.params.as_slice(),
            )
        })
        .ok_or_else(|| "default invocation has no imported declaration interface".into())
}

fn native_generics<'a>(
    checked: &'a CheckedProgram,
    unit: &ParameterDefaultUnit,
) -> Result<&'a [CallableGenericParam], String> {
    if let Some(signature) = checked.types.item_signatures.get(&unit.owner) {
        let callable = match signature {
            ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => Some(signature),
            ItemSignature::Agent(signature) => signature.callable(),
            ItemSignature::TopLevelLet(_) => None,
        };
        if let Some(callable) = callable {
            return Ok(&callable.generic_params);
        }
    }
    match &checked.hir.items[unit.owner] {
        HirItem::Spec(spec) => {
            let symbol = spec.items.iter().find_map(|item| match item {
                etas_hir::HirSpecItem::FlowSignature(signature)
                    if signature.params.contains(&unit.parameter) =>
                {
                    Some(signature.symbol)
                }
                _ => None,
            });
            checked
                .types
                .spec_signatures
                .get(&spec.symbol)
                .and_then(|spec| {
                    spec.methods
                        .iter()
                        .find(|method| method.source_symbol() == symbol)
                })
                .and_then(|method| method.signature.as_ref())
                .map(|signature| signature.generic_params.as_slice())
        }
        HirItem::Effect(effect) => effect
            .body
            .actions()
            .iter()
            .find(|action| action.params.contains(&unit.parameter))
            .and_then(|action| checked.types.action_signatures.get(&action.symbol))
            .map(|signature| signature.generic_params.as_slice()),
        HirItem::Impl(implementation) => implementation.items.iter().find_map(|item| match item {
            etas_hir::HirImplItem::Action(action) if action.params.contains(&unit.parameter) => {
                checked
                    .types
                    .action_signatures
                    .get(&action.symbol)
                    .map(|signature| signature.generic_params.as_slice())
            }
            _ => None,
        }),
        _ => None,
    }
    .ok_or_else(|| "default expression has no defining generic signature".into())
}
