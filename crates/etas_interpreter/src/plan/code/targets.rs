use std::collections::BTreeMap;

use etas_code::{CallableId, descriptor::GenericOwner};
use etas_frontend::code::{definition::DefinitionCatalog, service::CheckedCandidate};
use etas_hir::HirItemId;
use etas_types::{FlowSignature, ItemSignature, TypeId};

use super::RevisionTypeBridgeError as Error;

pub(crate) struct RevisionCallable {
    pub scope: GenericOwner,
    pub original_item: HirItemId,
    pub candidate_item: HirItemId,
    pub original: FlowSignature,
    pub candidate: FlowSignature,
}

pub(crate) struct RevisionTargets {
    pub candidate_agent: HirItemId,
    pub component: TypeId,
    pub default: CallableId,
    pub callables: BTreeMap<CallableId, RevisionCallable>,
    pub frozen_callables: BTreeMap<CallableId, RevisionCallable>,
    pub frozen_agents: BTreeMap<HirItemId, CallableId>,
    pub frozen_methods: BTreeMap<HirItemId, (HirItemId, CallableId)>,
}

impl RevisionTargets {
    pub fn build(
        candidate: &CheckedCandidate,
        original: &DefinitionCatalog,
        compiled: &DefinitionCatalog,
    ) -> Result<Self, Error> {
        let missing = || Error::InconsistentAgent(candidate.identity().clone());
        let default = candidate
            .frozen()
            .contract()
            .header()
            .default_entry
            .clone()
            .ok_or_else(missing)?;
        let component = original
            .agent_component(candidate.identity())
            .ok_or_else(missing)?;
        let candidate_component = compiled
            .agent_component(candidate.identity())
            .ok_or_else(missing)?;
        let mut agents = compiled.checked_program().types.item_signatures.iter().filter_map(|(item, signature)| {
            matches!(signature, ItemSignature::Agent(signature) if signature.component == candidate_component).then_some(*item)
        });
        let candidate_agent = agents.next().ok_or_else(missing)?;
        if agents.next().is_some() {
            return Err(missing());
        }
        let mut callables = BTreeMap::new();
        for (id, _) in candidate.frozen().contract().callables() {
            let signature = |catalog: &DefinitionCatalog| {
                let item = catalog
                    .item_owner(id)
                    .ok_or_else(|| Error::MissingCallable(id.clone()))?;
                let signature = match catalog.checked_program().types.item_signatures.get(&item) {
                    Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                        signature
                    }
                    Some(ItemSignature::Agent(signature)) => signature
                        .callable()
                        .ok_or_else(|| Error::MissingCallable(id.clone()))?,
                    _ => return Err(Error::MissingCallable(id.clone())),
                };
                Ok((item, signature.clone()))
            };
            let (original_item, original) = signature(original)?;
            let (candidate_item, candidate) = signature(compiled)?;
            let scope = id
                .as_method()
                .map(|method| GenericOwner::Method(method.clone()))
                .or_else(|| {
                    id.as_declaration()
                        .map(|declaration| GenericOwner::Declaration(declaration.clone()))
                })
                .ok_or_else(|| Error::MissingCallable(id.clone()))?;
            callables.insert(
                id.clone(),
                RevisionCallable {
                    scope,
                    original_item,
                    candidate_item,
                    original,
                    candidate,
                },
            );
        }
        let mut frozen_callables = BTreeMap::new();
        for (id, candidate_item) in compiled.native_callables() {
            let Some(original_item) = original.item_owner(id) else {
                if !compiled.is_dependency_source_callable(id) {
                    return Err(Error::MissingCallable(id.clone()));
                }
                if let Some((expected, expected_graph)) = original.external_callable(id) {
                    let actual = compiled
                        .callable(id)
                        .ok_or_else(|| Error::MissingCallable(id.clone()))?;
                    let actual_graph = compiled
                        .callable_descriptors(id)
                        .ok_or_else(|| Error::MissingCallable(id.clone()))?;
                    if expected_graph
                        .type_fingerprint(expected.signature)
                        .map_err(|_| Error::MissingCallable(id.clone()))?
                        != actual_graph
                            .type_fingerprint(actual.signature)
                            .map_err(|_| Error::MissingCallable(id.clone()))?
                    {
                        return Err(Error::MissingCallable(id.clone()));
                    }
                }
                // A newly linked dependency has no original HIR body. Its
                // checked candidate body executes in the candidate program;
                // it must not become a frozen-original execution target.
                continue;
            };
            let scope = match (id.as_method(), id.as_declaration()) {
                (Some(method), _) => GenericOwner::Method(method.clone()),
                (_, Some(declaration)) => GenericOwner::Declaration(declaration.clone()),
                _ => return Err(Error::MissingCallable(id.clone())),
            };
            let signature = |catalog: &DefinitionCatalog, item| {
                match catalog.checked_program().types.item_signatures.get(&item) {
                    Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                        Some(signature.clone())
                    }
                    Some(ItemSignature::Agent(signature)) => signature.callable().cloned(),
                    _ => None,
                }
                .ok_or_else(|| Error::MissingCallable(id.clone()))
            };
            frozen_callables.insert(
                id.clone(),
                RevisionCallable {
                    scope,
                    original_item,
                    candidate_item,
                    original: signature(original, original_item)?,
                    candidate: signature(compiled, candidate_item)?,
                },
            );
        }
        let mut frozen_agents = BTreeMap::new();
        for (item, declaration) in compiled.checked_program().hir.items.iter() {
            if !matches!(declaration, etas_hir::HirItem::Agent(_)) {
                continue;
            }
            if let Some(default) = compiled.native_agent_default(item) {
                if !frozen_callables.contains_key(default) {
                    return Err(Error::MissingCallable(default.clone()));
                }
                frozen_agents.insert(item, default.clone());
            }
        }
        let mut frozen_methods = BTreeMap::new();
        let callable_items = frozen_callables
            .iter()
            .map(|(id, callable)| (callable.candidate_item, id))
            .collect::<BTreeMap<_, _>>();
        for method in compiled.checked_program().types.agent_methods.values() {
            let callable = callable_items.get(&method.body).ok_or_else(&missing)?;
            let binding = (method.agent, (*callable).clone());
            if frozen_methods
                .insert(method.body, binding.clone())
                .is_some_and(|previous| previous != binding)
            {
                return Err(missing());
            }
        }
        Ok(Self {
            candidate_agent,
            component,
            default,
            callables,
            frozen_callables,
            frozen_agents,
            frozen_methods,
        })
    }
}
