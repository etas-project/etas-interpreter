use super::{CheckedActionInvocation, GenericBindings};
use etas_frontend::CheckedProgram;
use etas_hir::{HirHandlerArmId, HirOwner};
use etas_types::{CallableGenericParamKind, Type};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
};

/// An arm's current request, checked in the same type universe as its locals.
/// Shared frame aliases keep the request even without a named continuation.
#[derive(Clone, Debug)]
pub(crate) struct HandlerActivation {
    pub arm: HirHandlerArmId,
    pub request: CheckedActionInvocation,
    pub bindings: GenericBindings,
}

impl HandlerActivation {
    pub fn checked(
        checked: &CheckedProgram,
        arm: HirHandlerArmId,
        request: CheckedActionInvocation,
        outer: &GenericBindings,
    ) -> Result<Self, String> {
        request.validate_origin()?;
        if !std::sync::Arc::ptr_eq(checked, &request.program) {
            return Err("Handler request belongs to a different checked program".into());
        }
        let fact = checked
            .types
            .handler_actions
            .get(&arm)
            .ok_or("Handler has no checked request signature")?;
        let effect = checked
            .effects
            .handler_action_effects
            .get(&arm)
            .ok_or("Handler has no checked action identity")?;
        let mut effect = effect.clone();
        if let etas_effects::Effect::AppliedAction(instance) = &mut effect {
            for argument in &mut instance.args {
                if let etas_types::EffectArgRef::Type(ty) = argument {
                    *ty = outer
                        .substitution(&checked.type_store)
                        .substitute(*ty)
                        .map_err(|error| error.to_string())?;
                }
            }
        }
        if !(etas_effects::EffectCoverage {
            registry: &checked.effect_registry,
            types: &checked.type_store,
        })
        .covers(&effect, &request.effect)
        {
            return Err("Handler activation differs from its checked action selector".into());
        }
        let signature = checked
            .types
            .action_signatures
            .get(&fact.action_symbol)
            .ok_or("Handler has no checked action declaration")?;
        let mut allowed = signature
            .generic_params
            .iter()
            .filter(|parameter| parameter.kind == CallableGenericParamKind::Type)
            .map(|parameter| parameter.subject)
            .collect::<BTreeSet<_>>();
        if let Some(deferred) = checked.types.deferred_handler_instantiations.get(&arm) {
            allowed.extend(
                deferred
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.parametric)
                    .map(|parameter| parameter.rigid),
            );
        }
        if fact.params.len() != request.parameters.len() {
            return Err("Handler request parameter arity differs".into());
        }
        let pairs = fact
            .params
            .iter()
            .copied()
            .zip(request.parameters.iter().copied())
            .chain(std::iter::once((fact.output, request.output)))
            .collect::<Vec<_>>();
        let mut bindings = outer.clone();
        let mut pending = pairs.clone();
        let mut visited = HashSet::new();
        while let Some((expected, actual)) = pending.pop() {
            if !visited.insert((expected, actual)) {
                continue;
            }
            if allowed.contains(&expected) {
                if bindings
                    .parameters
                    .insert(expected, actual)
                    .is_some_and(|previous| previous != actual)
                {
                    return Err("Handler generic parameter has conflicting request types".into());
                }
                continue;
            }
            if expected == actual {
                continue;
            }
            match (
                checked.type_store.get(expected),
                checked.type_store.get(actual),
            ) {
                (Some(Type::Array(a)), Some(Type::Array(b)))
                | (Some(Type::List(a)), Some(Type::List(b)))
                | (Some(Type::Set(a)), Some(Type::Set(b)))
                | (Some(Type::Slice(a)), Some(Type::Slice(b)))
                | (Some(Type::Option(a)), Some(Type::Option(b)))
                | (Some(Type::Schema(a)), Some(Type::Schema(b)))
                | (Some(Type::Message(a)), Some(Type::Message(b)))
                | (Some(Type::MemoryRegion(a)), Some(Type::MemoryRegion(b)))
                | (Some(Type::MemorySelection(a)), Some(Type::MemorySelection(b)))
                | (Some(Type::Range { index: a }), Some(Type::Range { index: b }))
                | (Some(Type::Refined { base: a, .. }), Some(Type::Refined { base: b, .. }))
                | (Some(Type::Trust { inner: a, .. }), Some(Type::Trust { inner: b, .. })) => {
                    pending.push((*a, *b))
                }
                (Some(Type::Map { key: a, value: b }), Some(Type::Map { key: c, value: d }))
                | (
                    Some(Type::Store { key: a, value: b }),
                    Some(Type::Store { key: c, value: d }),
                )
                | (Some(Type::Result { ok: a, err: b }), Some(Type::Result { ok: c, err: d })) => {
                    pending.extend([(*a, *c), (*b, *d)])
                }
                (Some(Type::Tuple(a)), Some(Type::Tuple(b)))
                | (Some(Type::Applied { args: a, .. }), Some(Type::Applied { args: b, .. })) => {
                    pending.extend(a.iter().copied().zip(b.iter().copied()))
                }
                (Some(Type::Record(a)), Some(Type::Record(b))) => {
                    for field in &a.fields {
                        if let Some(other) = b.fields.iter().find(|other| other.name == field.name)
                        {
                            pending.push((field.ty, other.ty));
                        }
                    }
                }
                (Some(Type::Function(a)), Some(Type::Function(b))) => {
                    pending.extend(a.input.iter().copied().zip(b.input.iter().copied()));
                    pending.push((a.output, b.output));
                }
                (Some(Type::Task(a)), Some(Type::Task(b))) => pending.push((a.output, b.output)),
                (Some(Type::Continuation(a)), Some(Type::Continuation(b))) => {
                    pending.push((a.action_output, b.action_output));
                    if let (Some(a), Some(b)) = (a.handled_answer, b.handled_answer) {
                        pending.push((a, b));
                    }
                }
                _ => (),
            }
        }
        let mut inference_names = bindings.types.clone();
        for parameter in &signature.generic_params {
            if let Some(actual) = bindings.parameters.get(&parameter.subject) {
                inference_names.insert(parameter.name.clone(), *actual);
                // Source binders use their checked identity. Legacy anonymous
                // templates still require their declaration-local spelling.
                if !GenericBindings::is_checked_parameter(checked, parameter.subject) {
                    bindings.types.insert(parameter.name.clone(), *actual);
                }
            }
        }
        let row_parameters = signature
            .generic_params
            .iter()
            .filter(|parameter| parameter.kind == CallableGenericParamKind::Effect)
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        if !row_parameters.is_empty() {
            let mut rows = bindings
                .rows
                .iter()
                .map(|(name, row)| (name.clone(), row.clone()))
                .collect();
            for (expected, actual) in &pairs {
                let result = etas_types::solver::effect_row::infer_bindings_from_types(
                    &checked.type_store,
                    *expected,
                    *actual,
                    &inference_names,
                    &Default::default(),
                    &row_parameters,
                    &mut rows,
                );
                if !result.matches
                    || !result.deferred.is_empty()
                    || !result.deferred_fixed.is_empty()
                {
                    return Err("Handler request effect arguments are not closed".into());
                }
            }
            bindings.rows = rows.into_iter().collect();
        }
        for (expected, actual) in pairs {
            if bindings
                .substitution(&checked.type_store)
                .substitute(expected)
                .map_err(|error| error.to_string())?
                != actual
            {
                return Err(
                    "Handler request differs from its instantiated payload or result ABI".into(),
                );
            }
        }
        Ok(Self {
            arm,
            request,
            bindings,
        })
    }
}

pub(crate) fn enclosing_handler(
    checked: &CheckedProgram,
    mut owner: HirOwner,
) -> Option<HirHandlerArmId> {
    let index = checked.hir.index();
    loop {
        owner = match owner {
            HirOwner::HandlerArm(arm) => return Some(arm),
            HirOwner::Body(body) => match body.kind {
                etas_hir::HirBodyKind::HandlerArm(arm) => return Some(arm),
                _ => *index.body_owner.get(&body)?,
            },
            HirOwner::Block(block) => *index.block_owner.get(&block)?,
            HirOwner::Stmt(stmt) => HirOwner::Block(*index.stmt_block.get(&stmt)?),
            HirOwner::Expr(expr) => *index.expr_owner.get(&expr)?,
            HirOwner::Pattern(pat) => *index.pat_owner.get(&pat)?,
            HirOwner::Item(_) | HirOwner::Module(_) => return None,
        };
    }
}

pub(crate) fn activation_bindings<'a>(
    base: &'a GenericBindings,
    activation: Option<&HandlerActivation>,
) -> Cow<'a, GenericBindings> {
    match activation {
        None => Cow::Borrowed(base),
        Some(activation) => {
            let mut result = base.clone();
            result.types.extend(
                activation
                    .bindings
                    .types
                    .iter()
                    .map(|(name, ty)| (name.clone(), *ty)),
            );
            result.parameters.extend(
                activation
                    .bindings
                    .parameters
                    .iter()
                    .map(|(parameter, ty)| (*parameter, *ty)),
            );
            result.rows.extend(
                activation
                    .bindings
                    .rows
                    .iter()
                    .map(|(name, row)| (name.clone(), row.clone())),
            );
            Cow::Owned(result)
        }
    }
}

impl PartialEq for HandlerActivation {
    fn eq(&self, other: &Self) -> bool {
        self.arm == other.arm
            && self.bindings == other.bindings
            && std::sync::Arc::ptr_eq(&self.request.program, &other.request.program)
            && self.request.effect == other.request.effect
            && self.request.parameters == other.request.parameters
            && self.request.output == other.request.output
            && self.request.returns_never == other.request.returns_never
    }
}

/// These primitives consume the checked, parametric Continuation ABI. The
/// scheduler closes its result type from the owned token, preserving the
/// Handler's answer type rather than constructing a new static Continuation.
pub(crate) fn parametric_continuation_call(checked: &CheckedProgram, owner: HirOwner) -> bool {
    let HirOwner::Expr(expr) = owner else {
        return false;
    };
    let descriptor = checked
        .types
        .std_intrinsic_calls
        .get(&expr)
        .and_then(|fact| {
            checked
                .std_registry
                .intrinsic(etas_std::StdIntrinsicId(fact.intrinsic))
        })
        .or_else(|| {
            let etas_hir::HirExpr::Path(path) = checked.hir.exprs.get(expr)? else {
                return None;
            };
            let etas_hir::ResolveResult::Resolved(symbol) = path.resolution else {
                return None;
            };
            let etas_hir::SymbolDef::ImportAlias { path, .. } = &checked.symbols.get(symbol)?.def
            else {
                return None;
            };
            checked
                .std_registry
                .lookup_qualified(path)?
                .intrinsic
                .as_ref()
        });
    matches!(
        descriptor.map(|descriptor| &descriptor.lowering),
        Some(etas_std::LoweringHint::Task(
            etas_std::StdTaskIntrinsic::Park | etas_std::StdTaskIntrinsic::Requeue
        ))
    )
}
