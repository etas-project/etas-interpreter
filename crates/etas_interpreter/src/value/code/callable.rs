use std::{rc::Rc, sync::Arc};

use etas_frontend::{CheckedCallableRoot, CheckedProgram};
use etas_types::{FlowType, Type, TypeId};

use super::CheckedExecutionProgram;
use crate::{
    control::{CallTarget, CallTargetLink},
    plan::code::CallableValueProjection,
    value::InterpValue,
};
#[cfg(test)]
mod application_tests;
mod checkpoint;
mod effects;
mod revision;
mod signature;

pub(crate) use signature::validate_bounds as validate_snapshot_bounds;
pub(crate) use signature::validate_snapshot_signature;

#[derive(Debug)]
struct NativeCallable {
    program: CheckedExecutionProgram,
    application: Option<etas_frontend::CheckedCallableApplication>,
    target: CallTargetLink,
    ty: TypeId,
    signature: FlowType,
}

/// A native function value with its original captured frame and code owner.
/// No executable state can be reconstructed from a descriptor or checkpoint ID.
#[derive(Clone, Debug)]
pub struct ScopedCallableValue {
    native: Rc<NativeCallable>,
    projections: Vec<Arc<CallableValueProjection>>,
}

impl ScopedCallableValue {
    pub(crate) fn checked_flow_application(
        program: &CheckedProgram,
        target: &CallTarget,
        expected: &FlowType,
    ) -> Result<etas_frontend::CheckedCallableApplication, String> {
        let mut inner = target;
        while let CallTarget::Specialized { target, .. } = inner {
            inner = target;
        }
        let CallTarget::FlowItem(item) = inner else {
            return Err(
                "reflection requires an ordinary flow, not a closure or decorated callable".into(),
            );
        };
        validate_signature(program, CheckedCallableRoot::Item(*item), target, expected)?
            .ok_or_else(|| "ordinary flow has no sealed native application".into())
    }

    pub(crate) fn native_application(&self) -> Option<&etas_frontend::CheckedCallableApplication> {
        self.native.application.as_ref()
    }

    pub(crate) fn into_unique_target(self) -> Option<CallTarget> {
        Rc::try_unwrap(self.native).ok()?.target.into_unique()
    }
    pub(crate) fn action_projections(
        &self,
    ) -> Result<Vec<crate::plan::code::RevisionProjection>, String> {
        self.projections
            .iter()
            .map(|projection| projection.action_projection())
            .filter_map(Result::transpose)
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn projection_count(&self) -> usize {
        self.projections.len()
    }
    pub(crate) fn project(
        execution: &CheckedExecutionProgram,
        target: CallTarget,
        projection: CallableValueProjection,
    ) -> Result<CallTarget, String> {
        let destination = projection.target().0.clone();
        let (source, ty) = projection.source();
        let mut value = match target {
            CallTarget::Scoped(value) => {
                if !Arc::ptr_eq(value.presented_program(), source) || value.presented_type() != ty {
                    return Err(
                        "scoped callable is not owned by the projected source interface".into(),
                    );
                }
                value
            }
            target => {
                if !Arc::ptr_eq(&execution.checked, source) {
                    return Err("unscoped callable has no native execution environment in the source program".into());
                }
                let root = callable_root(execution, &target)?;
                let application =
                    validate_signature(source, root, &target, projection.signature())?;
                let plan = execution
                    .plan
                    .select_callable(source, root)
                    .map_err(|diagnostics| {
                        format!("callable preparation failed: {diagnostics:?}")
                    })?;
                Self {
                    native: Rc::new(NativeCallable {
                        program: CheckedExecutionProgram {
                            plan: Arc::new(plan),
                            ..execution.clone()
                        },
                        application,
                        target: target.into(),
                        ty,
                        signature: projection.signature().clone(),
                    }),
                    projections: Vec::new(),
                }
            }
        };
        if value
            .projections
            .last()
            .is_some_and(|previous| projection.reverses(previous))
        {
            value.projections.pop();
        } else {
            value.projections.push(Arc::new(projection));
        }
        if value.projections.is_empty() {
            if !Arc::ptr_eq(&value.native.program.checked, &destination) {
                return Err(
                    "cancelled callable projections do not return to the native program".into(),
                );
            }
            Ok(value.target())
        } else {
            Ok(CallTarget::Scoped(value))
        }
    }
    pub(crate) fn program(&self) -> &CheckedExecutionProgram {
        &self.native.program
    }
    pub(crate) fn target(&self) -> CallTarget {
        (*self.native.target).clone()
    }
    pub(crate) fn presented_program(&self) -> &CheckedProgram {
        self.projections
            .last()
            .map_or(&self.native.program.checked, |projection| {
                projection.target().0
            })
    }
    pub(crate) fn presented_type(&self) -> TypeId {
        self.projections
            .last()
            .map_or(self.native.ty, |projection| projection.target().1)
    }

    pub(crate) fn target_ref(&self) -> &CallTarget {
        &self.native.target
    }

    pub(crate) fn presented_signature(&self) -> Result<&FlowType, String> {
        match self
            .presented_program()
            .type_store
            .get(self.presented_type())
        {
            Some(Type::Function(signature)) => Ok(signature),
            _ => Err("scoped callable has no checked presented function type".into()),
        }
    }

    pub(crate) fn native_signature(&self) -> &FlowType {
        &self.native.signature
    }

    pub(crate) fn native_generic_parameters(
        &self,
    ) -> Result<&[etas_types::CallableGenericParam], String> {
        let checked = &self.native.program.checked;
        if let Some(application) = &self.native.application {
            if !Arc::ptr_eq(application.program(), checked)
                || native_application_owner(
                    checked,
                    callable_root(&self.native.program, &self.native.target)?,
                )? != Some(application.owner())
            {
                return Err("native application belongs to a different checked callable".into());
            }
        }
        match callable_root(&self.native.program, &self.native.target)? {
            CheckedCallableRoot::Item(item) => match checked.types.item_signatures.get(&item) {
                Some(
                    etas_types::ItemSignature::Flow(signature)
                    | etas_types::ItemSignature::Tool(signature),
                ) => Ok(&signature.generic_params),
                Some(etas_types::ItemSignature::Agent(signature)) => signature
                    .callable()
                    .map(|signature| signature.generic_params.as_slice())
                    .ok_or_else(|| "scoped agent has no checked callable signature".into()),
                _ => Err("scoped callable has no checked generic interface".into()),
            },
            CheckedCallableRoot::Lambda(_) | CheckedCallableRoot::StdReference(_) => Ok(&[]),
        }
    }
    pub(crate) fn arguments_to_native(
        &self,
        program: &CheckedExecutionProgram,
        args: Vec<InterpValue>,
    ) -> Result<Vec<InterpValue>, String> {
        if !Arc::ptr_eq(&program.checked, self.presented_program())
            || args.len() != self.native.signature.input.len()
        {
            return Err(
                "scoped callable invocation disagrees with its checked program or argument count"
                    .into(),
            );
        }
        args.into_iter()
            .enumerate()
            .map(|(index, mut value)| {
                for projection in self.projections.iter().rev() {
                    value = projection
                        .argument_to_source(index, value, program)
                        .map_err(|error| error.to_string())?;
                }
                Ok(value)
            })
            .collect()
    }
    pub(crate) fn result_to_presented(
        &self,
        program: &CheckedExecutionProgram,
        mut value: InterpValue,
    ) -> Result<InterpValue, String> {
        if !Arc::ptr_eq(&program.checked, &self.native.program.checked) {
            return Err("scoped callable result belongs to a different native program".into());
        }
        for projection in &self.projections {
            value = projection
                .result_to_target(value, program)
                .map_err(|error| error.to_string())?;
        }
        Ok(value)
    }

    pub(crate) fn error_to_presented(
        &self,
        program: &CheckedExecutionProgram,
        mut ty: TypeId,
        mut value: InterpValue,
    ) -> Result<(TypeId, InterpValue), String> {
        if !Arc::ptr_eq(&program.checked, &self.native.program.checked) {
            return Err("scoped callable error belongs to a different native program".into());
        }
        for projection in &self.projections {
            (ty, value) = projection
                .error_to_target(ty, value, program)
                .map_err(|error| error.to_string())?;
        }
        Ok((ty, value))
    }
}

impl PartialEq for ScopedCallableValue {
    fn eq(&self, other: &Self) -> bool {
        crate::value::comparison::scoped_equal(self, other)
    }
}
impl Eq for ScopedCallableValue {}

impl ScopedCallableValue {
    pub(crate) fn equality_header_matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.native.program.checked, &other.native.program.checked)
            && self.projections.len() == other.projections.len()
            && self
                .projections
                .iter()
                .zip(&other.projections)
                .all(|(left, right)| {
                    let (a, aty) = left.source();
                    let (b, bty) = right.source();
                    let (c, cty) = left.target();
                    let (d, dty) = right.target();
                    Arc::ptr_eq(a, b) && aty == bty && Arc::ptr_eq(c, d) && cty == dty
                })
    }
}

fn callable_root(
    execution: &CheckedExecutionProgram,
    target: &CallTarget,
) -> Result<CheckedCallableRoot, String> {
    let program = &execution.checked;
    let mut target = target;
    loop {
        match target {
            CallTarget::PureIntrinsic(call) => {
                return execution.plan.dispatch.intrinsic_callable_root(
                    crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: call.intrinsic,
                        dispatch: etas_std::IntrinsicDispatch::PureKernel,
                    },
                    &call.parameter_types,
                    call.result_type,
                );
            }
            CallTarget::StdIntrinsic(call) => {
                return execution.plan.dispatch.std_callable_root(call);
            }
            CallTarget::Lambda { expr, .. } => return Ok(CheckedCallableRoot::Lambda(*expr)),
            CallTarget::FlowItem(item) | CallTarget::ToolItem(item) => {
                return Ok(CheckedCallableRoot::Item(*item));
            }
            CallTarget::SpecImplMethod(symbol) => {
                match program.symbols.get(*symbol).map(|symbol| &symbol.def) {
                    Some(etas_hir::SymbolDef::Item { item }) => {
                        return Ok(CheckedCallableRoot::Item(*item));
                    }
                    _ => return Err("spec method callable has no checked body identity".into()),
                }
            }
            CallTarget::Specialized { target: inner, .. }
            | CallTarget::Limited { target: inner, .. } => target = inner,
            _ => return Err("callable has no supported native source execution root".into()),
        }
    }
}

fn validate_signature(
    program: &CheckedProgram,
    root: CheckedCallableRoot,
    target: &CallTarget,
    expected: &FlowType,
) -> Result<Option<etas_frontend::CheckedCallableApplication>, String> {
    let mut bindings = crate::control::GenericBindings::new();
    let mut target = target;
    loop {
        match target {
            CallTarget::Specialized {
                target: inner,
                type_bindings,
                effect_row_bindings,
            } => {
                merge_checked_bindings(&mut bindings, type_bindings, effect_row_bindings)?;
                target = inner;
            }
            CallTarget::Limited { target: inner, .. } => target = inner,
            CallTarget::Lambda { captured, .. } => {
                merge_checked_bindings(
                    &mut bindings,
                    &captured
                        .type_bindings()
                        .iter()
                        .map(|(name, ty)| (name.clone(), *ty))
                        .collect::<Vec<_>>(),
                    &captured
                        .generic_bindings()
                        .rows
                        .iter()
                        .map(|(name, row)| (name.clone(), row.clone()))
                        .collect::<Vec<_>>(),
                )?;
                break;
            }
            _ => break,
        }
    }
    validate_signature_bindings(program, root, expected, &bindings)?;
    checked_item_application(program, root, &bindings)
}

fn checked_item_application(
    program: &CheckedProgram,
    root: CheckedCallableRoot,
    bindings: &crate::control::GenericBindings,
) -> Result<Option<etas_frontend::CheckedCallableApplication>, String> {
    let Some(item) = native_application_owner(program, root)? else {
        return Ok(None);
    };
    if !matches!(
        program.items.get(item),
        Some(etas_hir::HirItem::Flow(_) | etas_hir::HirItem::Tool(_))
    ) {
        return Ok(None);
    }
    program
        .checked_callable_application(
            item,
            &bindings
                .iter()
                .map(|(name, ty)| (name.clone(), *ty))
                .collect::<Vec<_>>(),
            &bindings
                .rows
                .iter()
                .map(|(name, row)| (name.clone(), row.clone()))
                .collect::<Vec<_>>(),
        )
        .map(Some)
        .map_err(|error| error.to_string())
}

fn native_application_owner(
    program: &CheckedProgram,
    root: CheckedCallableRoot,
) -> Result<Option<etas_hir::HirItemId>, String> {
    match root {
        CheckedCallableRoot::Item(item) => Ok(Some(item)),
        CheckedCallableRoot::Lambda(expr) => program
            .hir
            .view()
            .index()
            .expr_item
            .get(&expr)
            .copied()
            .map(Some)
            .ok_or_else(|| "native closure has no checked lexical body owner".into()),
        CheckedCallableRoot::StdReference(_) => Ok(None),
    }
}

fn merge_checked_bindings(
    bindings: &mut crate::control::GenericBindings,
    types: &[(String, TypeId)],
    rows: &[(String, etas_types::EffectRowRef)],
) -> Result<(), String> {
    for (name, ty) in types {
        if bindings
            .insert(name.clone(), *ty)
            .is_some_and(|previous| previous != *ty)
        {
            return Err("native callable has conflicting type arguments".into());
        }
    }
    for (name, row) in rows {
        if bindings
            .rows
            .insert(name.clone(), row.clone())
            .is_some_and(|previous| {
                previous.tail != row.tail
                    || previous
                        .effects
                        .iter()
                        .any(|effect| !row.effects.contains(effect))
                    || row
                        .effects
                        .iter()
                        .any(|effect| !previous.effects.contains(effect))
            })
        {
            return Err("native callable has conflicting effect-row arguments".into());
        }
    }
    Ok(())
}

fn validate_signature_bindings(
    program: &CheckedProgram,
    root: CheckedCallableRoot,
    expected: &FlowType,
    bindings: &crate::control::GenericBindings,
) -> Result<(), String> {
    let signature_matches = |params: &[TypeId], output: TypeId| {
        params.len() == expected.input.len()
            && params
                .iter()
                .zip(&expected.input)
                .all(|(actual, expected)| {
                    bindings
                        .substitution(&program.type_store)
                        .substitute(*actual)
                        .is_ok_and(|actual| actual == *expected)
                })
            && bindings
                .substitution(&program.type_store)
                .substitute(output)
                .is_ok_and(|output| output == expected.output)
    };
    let matches = match root {
        CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) => program.types.expr_types.get(&expr).and_then(|ty| program.type_store.get(*ty))
            .is_some_and(|ty| matches!(ty, Type::Function(actual) if signature_matches(&actual.input, actual.output))),
        CheckedCallableRoot::Item(item) => match program.types.item_signatures.get(&item) {
            Some(etas_types::ItemSignature::Flow(actual) | etas_types::ItemSignature::Tool(actual)) => signature_matches(&actual.params, actual.output),
            Some(etas_types::ItemSignature::Agent(agent)) => agent.callable().is_some_and(|actual| signature_matches(&actual.params, actual.output)),
            _ => false,
        },
    };
    if matches {
        Ok(())
    } else {
        Err("native callable signature disagrees with the checked value interface".into())
    }
}
