use super::*;
use crate::control::{CallSite, CallTarget, GenericBindings};
use crate::driver::scheduler::CheckedTaskStart;
use etas_types::{CallableGenericParamKind, EffectRowRef, TypeId};

impl EvalContext<'_> {
    pub(super) fn execute_source_task_start(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        bindings: &GenericBindings,
        span: Span,
    ) -> ControlSignal {
        if self.task.task_scope.0 == 0 {
            return ControlSignal::runtime_fault("task start requires an active task scope", span);
        }
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        let result_type = match self.materialized_type(signature.output, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let mut parameters = Vec::new();
        let mut rows = Vec::new();
        for parameter in &signature.generic_params {
            match parameter.kind {
                CallableGenericParamKind::Type => {
                    let value = match self.materialized_type(parameter.subject, bindings, span) {
                        Ok(ty) => ty,
                        Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                    };
                    parameters.push((parameter.subject, value));
                }
                CallableGenericParamKind::Effect => {
                    let row = EffectRowRef {
                        effects: Vec::new(),
                        tail: Some(parameter.name.clone()),
                    };
                    let value = match bindings
                        .substitution(&self.checked.type_store)
                        .substitute_effect_row(row)
                    {
                        Ok(row) => row,
                        Err(reason) => {
                            return ControlSignal::missing_checked_fact(reason.to_string(), span);
                        }
                    };
                    rows.push((parameter.name.clone(), value));
                }
            }
        }
        let target = CallTarget::Specialized {
            target: CallTarget::FlowItem(item).into(),
            type_bindings: Vec::new(),
            type_parameter_bindings: parameters,
            effect_row_bindings: rows,
        };
        let site = CallSite::source_task_callback(item, target.clone());
        self.start_checked_task(
            CheckedTaskStart {
                target,
                result_type,
            },
            site,
            args,
            span,
        )
    }
}

/// Recover the saved source primitive's checked callback and result types.
/// No legacy registry entry or synthetic intrinsic expression is involved.
pub(crate) fn source_start_types(
    checked: &etas_frontend::CheckedProject,
    mut target: &crate::orchestration::CallTargetSnapshot,
) -> Result<(HirItemId, TypeId, TypeId), String> {
    use crate::orchestration::CallTargetSnapshot as T;
    let mut bindings = GenericBindings::new();
    while let T::Specialized {
        target: inner,
        type_bindings,
        type_parameter_bindings,
        effect_row_bindings,
    } = target
    {
        let mut substitution = bindings.substitution(&checked.type_store);
        let types = type_bindings
            .iter()
            .map(|(name, ty)| Ok((name.clone(), substitution.substitute(*ty)?)))
            .collect::<Result<Vec<_>, etas_types::TypeSubstitutionError>>()
            .map_err(|error| error.to_string())?;
        let parameters = type_parameter_bindings
            .iter()
            .map(|(name, ty)| Ok((*name, substitution.substitute(*ty)?)))
            .collect::<Result<Vec<_>, etas_types::TypeSubstitutionError>>()
            .map_err(|error| error.to_string())?;
        let rows = effect_row_bindings
            .iter()
            .map(|(name, row)| {
                Ok((
                    name.clone(),
                    substitution.substitute_effect_row(row.clone())?,
                ))
            })
            .collect::<Result<Vec<_>, etas_types::TypeSubstitutionError>>()
            .map_err(|error| error.to_string())?;
        drop(substitution);
        bindings.types.extend(types);
        bindings.parameters.extend(parameters);
        bindings.rows.extend(rows);
        target = inner;
    }
    let T::FlowItem(item) = target else {
        return Err("source task callback has no source flow entry".into());
    };
    if !matches!(
        checked.hir.items.get(*item),
        Some(HirItem::Flow(etas_hir::HirFlowDecl {
            body: etas_hir::HirFlowBody::Runtime {
                role: etas_std::StdExecutionRole::TaskStart,
                ..
            },
            ..
        }))
    ) {
        return Err("source task callback has no checked task start role".into());
    }
    let Some(etas_types::ItemSignature::Flow(signature)) = checked.types.item_signatures.get(item)
    else {
        return Err("source task callback has no checked signature".into());
    };
    let [callback] = signature.params.as_slice() else {
        return Err("source task start has no callback parameter".into());
    };
    let mut substitution = bindings.substitution(&checked.type_store);
    let callback = substitution
        .substitute(*callback)
        .map_err(|error| error.to_string())?;
    let output = substitution
        .substitute(signature.output)
        .map_err(|error| error.to_string())?;
    Ok((*item, callback, output))
}
