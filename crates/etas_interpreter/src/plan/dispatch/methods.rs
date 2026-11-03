use std::collections::{HashMap, HashSet};

use etas_frontend::CheckedProject;
use etas_hir::{HirExpr, HirExprId, SymbolId};
use etas_types::{
    SpecMethodIdentity, TypeId, TypeSubstitutionEngine,
    facts::CheckedMethodCallFact,
    method::{MethodEvidence, MethodReceiver, MethodSignatureFact, MethodTarget},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RequirementKey {
    Source(SymbolId),
    External { package: u32, path: Vec<String> },
}

impl From<&SpecMethodIdentity> for RequirementKey {
    fn from(member: &SpecMethodIdentity) -> Self {
        match member {
            SpecMethodIdentity::Source(symbol) => Self::Source(*symbol),
            SpecMethodIdentity::External { package, path } => Self::External {
                package: *package,
                path: path.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TargetKey {
    Source(SymbolId),
    Agent(SymbolId),
    Standard { path: Vec<String>, overload: usize },
    External(etas_code::MethodId),
    Bound(RequirementKey),
}

impl From<&MethodTarget> for TargetKey {
    fn from(target: &MethodTarget) -> Self {
        match target {
            MethodTarget::Source { symbol, .. } => Self::Source(*symbol),
            MethodTarget::Agent { symbol, .. } => Self::Agent(*symbol),
            MethodTarget::Standard {
                qualified_path,
                overload,
                ..
            } => Self::Standard {
                path: qualified_path.clone(),
                overload: *overload,
            },
            MethodTarget::External { identity } => Self::External(identity.clone()),
            MethodTarget::BoundSpec { member, .. } => Self::Bound(member.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WitnessKey {
    receiver: TypeId,
    member: RequirementKey,
    args: Vec<TypeId>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedWitness {
    pub target: MethodTarget,
    pub parameters: Vec<(String, String)>,
}

pub(crate) struct ResolvedMethodCall<'a> {
    pub target: &'a MethodTarget,
    pub bindings: crate::control::CheckedCallBindings,
}

#[derive(Debug, Default)]
pub(super) struct MethodDispatchPlan {
    calls: HashMap<HirExprId, CheckedMethodCallFact>,
    witnesses: HashMap<WitnessKey, PreparedWitness>,
}

impl MethodDispatchPlan {
    pub(super) fn prepare(project: &CheckedProject) -> Result<Self, String> {
        let mut result = Self::default();
        let mut declarations = HashMap::<TargetKey, Vec<&MethodSignatureFact>>::new();
        let requirements = project
            .types
            .spec_signatures
            .values()
            .flat_map(|spec| &spec.methods)
            .chain(
                project
                    .types
                    .external_specs
                    .signatures
                    .values()
                    .flat_map(|spec| &spec.methods),
            )
            .map(|method| (RequirementKey::from(&method.identity), method))
            .collect::<HashMap<_, _>>();
        for entry in project.types.methods.entries() {
            declarations
                .entry((&entry.target).into())
                .or_default()
                .push(entry);
            let (Some(member), MethodEvidence::Implementation { subject, args, .. }) =
                (&entry.spec_member, &entry.evidence)
            else {
                continue;
            };
            let required = requirements
                .get(&RequirementKey::from(member))
                .ok_or("method witness has no checked requirement declaration")?;
            let signature = required
                .signature_in(&project.types)
                .ok_or("method witness has no checked requirement signature")?;
            let parameters = required
                .implementation_generic_parameters(signature, &entry.signature)?
                .into_iter()
                .map(|(required, provided)| (required.name.clone(), provided.name.clone()))
                .collect();
            let key = WitnessKey {
                receiver: *subject,
                member: member.into(),
                args: args.clone(),
            };
            let witness = PreparedWitness {
                target: entry.target.clone(),
                parameters,
            };
            if result.witnesses.insert(key, witness).is_some() {
                return Err(
                    "method environment has duplicate implementation witness identities".into(),
                );
            }
        }
        let view = project.hir.view();
        for (expr, fact) in &project.types.checked_method_calls {
            let candidates = declarations
                .get(&(&fact.target).into())
                .ok_or("checked method target is absent from its declaration environment")?;
            let mut matching = candidates.iter().filter(|entry| {
                entry.target == fact.target && same_evidence_kind(&entry.evidence, &fact.evidence)
            });
            let entry = matching
                .next()
                .ok_or("checked method target has no matching implementation evidence")?;
            if matching.next().is_some() {
                return Err("checked method target has ambiguous declaration identities".into());
            }
            let (receiver, args) = match project.hir.exprs.get(*expr) {
                Some(
                    HirExpr::MethodCall { receiver, args, .. }
                    | HirExpr::SpecMethodCall { receiver, args, .. },
                ) => (*receiver, args),
                _ => {
                    return Err(
                        "checked method fact does not belong to a method call expression".into(),
                    );
                }
            };
            let owner = view
                .index()
                .expr_item
                .get(expr)
                .ok_or("checked method has no source owner")?;
            if !entry.public && entry.defining_module != project.hir.item_module(*owner) {
                return Err("checked method target is private to another module".into());
            }
            let receiver_type = project
                .types
                .expr_types
                .get(&receiver)
                .ok_or("checked method receiver is missing its value type")?;
            if fact.receiver != entry.receiver || *receiver_type != fact.receiver_type {
                return Err("checked method receiver differs from its declaration ABI".into());
            }
            validate_signature(project, entry, fact)?;
            let mut slots = HashSet::new();
            if let MethodReceiver::Value { slot } = fact.receiver {
                slots.insert(slot);
            }
            if args.len() != fact.argument_slots.len()
                || fact.argument_slots.iter().any(|slot| {
                    *slot as usize >= fact.signature.params.len() || !slots.insert(*slot)
                })
                || slots.len() != fact.signature.params.len()
            {
                return Err(
                    "checked method arguments do not cover its receiver and parameter ABI".into(),
                );
            }
            if let MethodTarget::Source {
                symbol,
                callable,
                body,
            } = fact.target
            {
                if !matches!(project.hir.items.get(callable), Some(etas_hir::HirItem::Flow(flow)) if flow.symbol == symbol)
                    || view
                        .body(body)
                        .is_none_or(|body| body.item().id() != callable)
                {
                    return Err("checked method target differs from its real source body".into());
                }
            }
            result.calls.insert(*expr, fact.clone());
        }
        Ok(result)
    }

    pub(super) fn call(&self, expr: HirExprId) -> Option<&CheckedMethodCallFact> {
        self.calls.get(&expr)
    }

    pub(super) fn witness(
        &self,
        receiver: TypeId,
        args: &[TypeId],
        member: &SpecMethodIdentity,
    ) -> Option<&PreparedWitness> {
        self.witnesses.get(&WitnessKey {
            receiver,
            member: member.into(),
            args: args.to_vec(),
        })
    }

    pub(super) fn resolve(
        &self,
        project: &CheckedProject,
        call: HirExprId,
        current: &crate::control::GenericBindings,
    ) -> Result<ResolvedMethodCall<'_>, String> {
        let fact = self
            .call(call)
            .ok_or("method call has no prepared target")?;
        let mut bindings = crate::control::CheckedCallBindings::resolve(project, call, current)?;
        let target = if let MethodTarget::BoundSpec { member, .. } = &fact.target {
            let MethodEvidence::Bound { args, .. } = &fact.evidence else {
                return Err("generic method has no checked witness recipe".into());
            };
            let mut substitution = current.substitution(&project.type_store);
            let subject = substitution
                .substitute(fact.receiver_type)
                .map_err(|error| error.to_string())?;
            let args = args
                .iter()
                .map(|ty| {
                    substitution
                        .substitute(*ty)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let witness = self
                .witness(subject, &args, member)
                .ok_or("generic method has no prepared implementation witness")?;
            for (name, _) in &mut bindings.types {
                *name = witness
                    .parameters
                    .iter()
                    .find(|(required, _)| required == name)
                    .map(|(_, actual)| actual.clone())
                    .ok_or("method witness has no type binder mapping")?;
            }
            for (name, _) in &mut bindings.rows {
                *name = witness
                    .parameters
                    .iter()
                    .find(|(required, _)| required == name)
                    .map(|(_, actual)| actual.clone())
                    .ok_or("method witness has no effect binder mapping")?;
            }
            &witness.target
        } else {
            &fact.target
        };
        Ok(ResolvedMethodCall { target, bindings })
    }
}

fn same_evidence_kind(left: &MethodEvidence, right: &MethodEvidence) -> bool {
    match (left, right) {
        (MethodEvidence::Inherent, MethodEvidence::Inherent) => true,
        (
            MethodEvidence::Implementation { spec: left, .. },
            MethodEvidence::Implementation { spec: right, .. },
        )
        | (MethodEvidence::Bound { spec: left, .. }, MethodEvidence::Bound { spec: right, .. }) => {
            left == right
        }
        _ => false,
    }
}

fn validate_signature(
    project: &CheckedProject,
    entry: &MethodSignatureFact,
    fact: &CheckedMethodCallFact,
) -> Result<(), String> {
    let types = fact.instantiation.type_bindings.iter().cloned().collect();
    let rows = fact
        .instantiation
        .effect_row_bindings
        .iter()
        .cloned()
        .collect();
    let mut parameters = fact
        .owner_bindings
        .iter()
        .copied()
        .collect::<HashMap<_, _>>();
    for parameter in &entry.signature.generic_params {
        if let Some((_, actual)) = fact
            .instantiation
            .type_bindings
            .iter()
            .find(|(name, _)| name == &parameter.name)
        {
            parameters.insert(parameter.subject, *actual);
        }
    }
    let mut substitute = TypeSubstitutionEngine::existing(&project.type_store, &types)
        .with_type_parameters(&parameters)
        .with_effect_arguments(&rows);
    let input = entry
        .signature
        .params
        .iter()
        .skip(usize::from(entry.receiver == MethodReceiver::Component))
        .map(|ty| {
            substitute
                .substitute(*ty)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output = substitute
        .substitute(entry.signature.output)
        .map_err(|error| error.to_string())?;
    if input != fact.signature.params || output != fact.signature.output {
        return Err(
            "checked method signature differs from its selected declaration substitution".into(),
        );
    }
    Ok(())
}
