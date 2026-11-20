use std::collections::{BTreeMap, HashMap};

use etas_code::{
    CodeFingerprint,
    descriptor::{GenericOwner, TypeInfoRef},
};
use etas_frontend::{
    CheckedProgram,
    code::{definition::DefinitionCatalog, service::CheckedCandidate},
};
use etas_types::{ItemSignature, TypeId};

mod actions;
mod runtime;
#[cfg(test)]
mod tests;
pub(crate) use actions::ActionPair;

#[derive(Debug)]
pub enum RevisionTypeBridgeError {
    WrongProgram,
    Projection(etas_frontend::code::descriptor::ProjectionError),
    AmbiguousType { scope: GenericOwner, ty: TypeId },
    MissingType { scope: GenericOwner, ty: TypeId },
    MissingCallable(etas_code::CallableId),
    InconsistentInterface(etas_code::CallableId),
    InconsistentDefinition(etas_code::DefinitionKey),
    AmbiguousAgentType(TypeId),
    InvalidRuntimeInterface(String),
    RuntimeType(etas_types::TypeSubstitutionError),
}

impl std::fmt::Display for RevisionTypeBridgeError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "checked revision type correspondence failed: {self:?}")
    }
}
impl std::error::Error for RevisionTypeBridgeError {}

/// Native checked correspondence under the unchanged frozen interface. The
/// descriptor fingerprints index nodes; they do not grant assignability or
/// authorize a caller-supplied graph as executable evidence.
pub struct RevisionTypeBridge {
    runtime_owner: std::sync::OnceLock<std::sync::Weak<crate::api::code::PreparedCodeRevision>>,
    standard_library: Option<std::sync::Arc<etas_frontend::StandardLibrary>>,
    original: CheckedProgram,
    candidate: CheckedProgram,
    forward: BTreeMap<(GenericOwner, TypeId), Option<TypeId>>,
    backward: BTreeMap<(GenericOwner, TypeId), Option<TypeId>>,
    original_callables: BTreeMap<etas_code::CallableId, etas_hir::HirItemId>,
    candidate_callables: BTreeMap<etas_code::CallableId, etas_hir::HirItemId>,
    original_agents: BTreeMap<TypeId, etas_hir::HirItemId>,
    candidate_agents: BTreeMap<TypeId, etas_hir::HirItemId>,
    runtime_types: runtime::RuntimeTypeBridge,
    actions: actions::ActionBridge,
}

impl std::fmt::Debug for RevisionTypeBridge {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("RevisionTypeBridge")
            .field("forward_types", &self.forward.len())
            .field("backward_types", &self.backward.len())
            .field("original_callables", &self.original_callables.len())
            .field("candidate_callables", &self.candidate_callables.len())
            .finish_non_exhaustive()
    }
}

impl RevisionTypeBridge {
    pub(crate) fn for_standard_library(
        original: &DefinitionCatalog,
        library: &DefinitionCatalog,
        owner: &std::sync::Arc<etas_frontend::StandardLibrary>,
        installed_fingerprints: Option<&BTreeMap<(GenericOwner, TypeId), CodeFingerprint>>,
    ) -> Result<Self, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(library.checked_program(), &owner.program)
            || !original
                .checked_program()
                .standard_library
                .as_ref()
                .is_some_and(|selected| std::sync::Arc::ptr_eq(selected, owner))
        {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        let mut bridge =
            Self::catalog_correspondence_prepared(original, library, installed_fingerprints)?;
        bridge.standard_library = Some(owner.clone());
        Ok(bridge)
    }
    pub(crate) fn from_catalogs(
        candidate: &CheckedCandidate,
        original: &DefinitionCatalog,
        compiled: &DefinitionCatalog,
    ) -> Result<Self, RevisionTypeBridgeError> {
        let bridge = Self::catalog_correspondence(original, compiled)?;
        if candidate.identity().kind() == etas_code::DefinitionKind::Agent {
            let component = |catalog: &DefinitionCatalog| {
                catalog
                    .agent_component(candidate.identity())
                    .ok_or_else(|| {
                        RevisionTypeBridgeError::InconsistentDefinition(
                            candidate.identity().clone(),
                        )
                    })
            };
            let (source, target) = (component(original)?, component(compiled)?);
            let owner = GenericOwner::Declaration(candidate.identity().clone());
            if bridge.to_candidate(&bridge.original, &owner, source)? != target
                || bridge.to_original(&bridge.candidate, &owner, target)? != source
            {
                return Err(RevisionTypeBridgeError::InconsistentDefinition(
                    candidate.identity().clone(),
                ));
            }
        }
        for (callable, _) in original.native_callables() {
            let owner = callable
                .as_method()
                .map(|method| GenericOwner::Method(method.clone()))
                .or_else(|| {
                    callable
                        .as_declaration()
                        .map(|declaration| GenericOwner::Declaration(declaration.clone()))
                })
                .ok_or_else(|| RevisionTypeBridgeError::MissingCallable(callable.clone()))?;
            let signature = |catalog: &DefinitionCatalog| {
                let item = catalog.item_owner(callable)?;
                match catalog.checked_program().types.item_signatures.get(&item)? {
                    ItemSignature::Flow(signature) | ItemSignature::Tool(signature) => {
                        Some(signature.clone())
                    }
                    ItemSignature::Agent(signature) => signature.callable().cloned(),
                    ItemSignature::TopLevelLet(_) => None,
                }
            };
            let left = signature(original)
                .ok_or_else(|| RevisionTypeBridgeError::MissingCallable(callable.clone()))?;
            let right = signature(compiled)
                .ok_or_else(|| RevisionTypeBridgeError::MissingCallable(callable.clone()))?;
            if left.params.len() != right.params.len()
                || left.generic_params.len() != right.generic_params.len()
            {
                return Err(RevisionTypeBridgeError::InconsistentInterface(
                    callable.clone(),
                ));
            }
            for (from, to) in left
                .params
                .iter()
                .zip(&right.params)
                .chain(std::iter::once((&left.output, &right.output)))
            {
                if bridge.to_candidate(&bridge.original, &owner, *from)? != *to
                    || bridge.to_original(&bridge.candidate, &owner, *to)? != *from
                {
                    return Err(RevisionTypeBridgeError::InconsistentInterface(
                        callable.clone(),
                    ));
                }
            }
        }
        Ok(bridge)
    }

    pub(crate) fn from_external_application(
        candidate: &CheckedCandidate,
        definitions: &etas_frontend::code::definition::NativeDefinitionStore,
        definition: &etas_code::definition::DefinitionRef,
        compiled: &DefinitionCatalog,
    ) -> Result<Self, RevisionTypeBridgeError> {
        if !candidate.matches_external_definition(definitions, definition)
            || !std::sync::Arc::ptr_eq(compiled.checked_program(), candidate.program())
        {
            return Err(RevisionTypeBridgeError::InconsistentDefinition(
                candidate.identity().clone(),
            ));
        }
        let view = definitions
            .resolve(definition)
            .map_err(|error| RevisionTypeBridgeError::InvalidRuntimeInterface(error.to_string()))?;
        let application = view.external_application().ok_or_else(|| {
            RevisionTypeBridgeError::InconsistentDefinition(candidate.identity().clone())
        })?;
        if !std::sync::Arc::ptr_eq(
            application.program(),
            definitions.catalog().checked_program(),
        ) || application.callable().as_declaration() != Some(candidate.identity())
        {
            return Err(RevisionTypeBridgeError::InconsistentDefinition(
                candidate.identity().clone(),
            ));
        }
        Self::catalog_correspondence(definitions.catalog(), compiled)
    }

    fn catalog_correspondence(
        original: &DefinitionCatalog,
        compiled: &DefinitionCatalog,
    ) -> Result<Self, RevisionTypeBridgeError> {
        Self::catalog_correspondence_prepared(original, compiled, None)
    }

    fn catalog_correspondence_prepared(
        original: &DefinitionCatalog,
        compiled: &DefinitionCatalog,
        installed: Option<&BTreeMap<(GenericOwner, TypeId), CodeFingerprint>>,
    ) -> Result<Self, RevisionTypeBridgeError> {
        let original_keys = fingerprints(original)?;
        let compiled_keys = match installed {
            Some(keys) => compiled
                .native_type_origins()
                .iter()
                .map(|origin| {
                    let fingerprint =
                        match keys.get(&(origin.scope.clone(), origin.local)) {
                            Some(fingerprint) => *fingerprint,
                            // Linking can append caller-specific generic instances.
                            // Their types are the only SDK-side additions to index.
                            None => compiled
                                .descriptors()
                                .type_fingerprint(origin.descriptor)
                                .map_err(|error| {
                                    RevisionTypeBridgeError::Projection(
                                etas_frontend::code::descriptor::ProjectionError::Descriptor(error)
                            )
                                })?,
                        };
                    Ok((origin.descriptor, fingerprint))
                })
                .collect::<Result<HashMap<_, _>, RevisionTypeBridgeError>>()?,
            None => fingerprints(compiled)?,
        };
        let forward = correspond(original, &original_keys, compiled, &compiled_keys)?;
        let backward = correspond(compiled, &compiled_keys, original, &original_keys)?;
        let runtime_types =
            runtime::RuntimeTypeBridge::new(original, compiled, &forward, &backward)?;
        let actions = actions::ActionBridge::new(original, compiled, &runtime_types)?;
        Ok(Self {
            runtime_owner: Default::default(),
            standard_library: None,
            original: original.checked_program().clone(),
            candidate: compiled.checked_program().clone(),
            forward,
            backward,
            original_callables: original
                .native_callables()
                .map(|(id, item)| (id.clone(), item))
                .collect(),
            candidate_callables: compiled
                .native_callables()
                .map(|(id, item)| (id.clone(), item))
                .collect(),
            original_agents: agent_items(original)?,
            candidate_agents: agent_items(compiled)?,
            runtime_types,
            actions,
        })
    }

    pub fn original_program(&self) -> &CheckedProgram {
        &self.original
    }

    pub(crate) fn native_method(
        &self,
        item: etas_hir::HirItemId,
        candidate: bool,
    ) -> Option<&etas_code::MethodId> {
        let callables = if candidate {
            &self.candidate_callables
        } else {
            &self.original_callables
        };
        callables
            .iter()
            .find_map(|(identity, owner)| (*owner == item).then(|| identity.as_method()).flatten())
    }

    pub(crate) fn bind_runtime_owner(
        &self,
        owner: &std::sync::Arc<crate::api::code::PreparedCodeRevision>,
    ) {
        self.runtime_owner
            .get_or_init(|| std::sync::Arc::downgrade(owner));
    }

    pub(crate) fn runtime_owner(
        &self,
    ) -> Option<std::sync::Arc<crate::api::code::PreparedCodeRevision>> {
        self.runtime_owner.get().and_then(std::sync::Weak::upgrade)
    }

    pub(crate) fn standard_library_owner(
        &self,
    ) -> Option<&std::sync::Arc<etas_frontend::StandardLibrary>> {
        self.standard_library.as_ref()
    }
    pub fn candidate_program(&self) -> &CheckedProgram {
        &self.candidate
    }

    pub(crate) fn runtime_to_candidate(
        &self,
        source: &CheckedProgram,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.original) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types.forward.get(source, &self.candidate, ty)
    }

    pub(crate) fn runtime_to_original(
        &self,
        source: &CheckedProgram,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.candidate) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types.backward.get(source, &self.original, ty)
    }

    /// Callable snapshots may retain a checked generic declaration template.
    /// Its row binders are related by that declaration's frozen interface.
    pub(crate) fn callable_projection_type(
        &self,
        source: &CheckedProgram,
        ty: TypeId,
        to_candidate: bool,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        let (expected, target, callables, direction) = if to_candidate {
            (
                &self.original,
                &self.candidate,
                &self.original_callables,
                &self.runtime_types.forward,
            )
        } else {
            (
                &self.candidate,
                &self.original,
                &self.candidate_callables,
                &self.runtime_types.backward,
            )
        };
        if !std::sync::Arc::ptr_eq(source, expected) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        let scope = checked_callable_template_scope(source, callables, ty);
        match scope {
            Some(scope) => direction.get_in_scope(source, target, &scope, ty),
            None => direction.get(source, target, ty),
        }
    }

    pub(crate) fn runtime_to_candidate_scoped(
        &self,
        source: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.original) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types
            .forward
            .get_in_scope(source, &self.candidate, scope, ty)
    }

    pub(crate) fn runtime_to_original_scoped(
        &self,
        source: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.candidate) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types
            .backward
            .get_in_scope(source, &self.original, scope, ty)
    }

    pub(crate) fn runtime_row_to_candidate(
        &self,
        source: &CheckedProgram,
        row: etas_types::EffectRowRef,
    ) -> Result<etas_types::EffectRowRef, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.original) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types.forward.row(source, &self.candidate, row)
    }

    pub(crate) fn runtime_row_to_original(
        &self,
        source: &CheckedProgram,
        row: etas_types::EffectRowRef,
    ) -> Result<etas_types::EffectRowRef, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.candidate) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.runtime_types.backward.row(source, &self.original, row)
    }

    pub(crate) fn action_to_candidate(
        &self,
        source: &CheckedProgram,
        action: &etas_effects::ActionRef,
    ) -> Result<&std::sync::Arc<ActionPair>, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.original) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.actions.to_candidate(action)
    }

    pub(crate) fn corresponding_action(
        &self,
        action: &etas_effects::ActionRef,
        forward: bool,
    ) -> Option<etas_effects::ActionRef> {
        self.actions.corresponding_action(action, forward)
    }

    pub(crate) fn action_to_original(
        &self,
        source: &CheckedProgram,
        action: &etas_effects::ActionRef,
    ) -> Result<&std::sync::Arc<ActionPair>, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.candidate) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.actions.to_original(action)
    }

    pub(crate) fn candidate_callable(
        &self,
        id: &etas_code::CallableId,
    ) -> Option<etas_hir::HirItemId> {
        self.candidate_callables.get(id).copied()
    }

    pub(crate) fn original_callable(
        &self,
        id: &etas_code::CallableId,
    ) -> Option<etas_hir::HirItemId> {
        self.original_callables.get(id).copied()
    }

    pub(crate) fn original_agent(&self, ty: TypeId) -> Option<etas_hir::HirItemId> {
        self.original_agents.get(&ty).copied()
    }

    pub(crate) fn candidate_agent(&self, ty: TypeId) -> Option<etas_hir::HirItemId> {
        self.candidate_agents.get(&ty).copied()
    }

    pub fn to_candidate(
        &self,
        source: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.original) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.forward
            .get(&(scope.clone(), ty))
            .copied()
            .flatten()
            .ok_or_else(|| RevisionTypeBridgeError::MissingType {
                scope: scope.clone(),
                ty,
            })
    }
    pub fn to_original(
        &self,
        source: &CheckedProgram,
        scope: &GenericOwner,
        ty: TypeId,
    ) -> Result<TypeId, RevisionTypeBridgeError> {
        if !std::sync::Arc::ptr_eq(source, &self.candidate) {
            return Err(RevisionTypeBridgeError::WrongProgram);
        }
        self.backward
            .get(&(scope.clone(), ty))
            .copied()
            .flatten()
            .ok_or_else(|| RevisionTypeBridgeError::MissingType {
                scope: scope.clone(),
                ty,
            })
    }
}

fn agent_items(
    catalog: &DefinitionCatalog,
) -> Result<BTreeMap<TypeId, etas_hir::HirItemId>, RevisionTypeBridgeError> {
    let mut agents = BTreeMap::new();
    for (item, signature) in catalog.checked_program().types.item_signatures.iter() {
        if let ItemSignature::Agent(signature) = signature
            && agents.insert(signature.component, *item).is_some()
        {
            return Err(RevisionTypeBridgeError::AmbiguousAgentType(
                signature.component,
            ));
        }
    }
    Ok(agents)
}

pub(crate) fn fingerprints(
    catalog: &DefinitionCatalog,
) -> Result<HashMap<TypeInfoRef, CodeFingerprint>, RevisionTypeBridgeError> {
    let mut keys = HashMap::new();
    for origin in catalog.native_type_origins() {
        if let std::collections::hash_map::Entry::Vacant(entry) = keys.entry(origin.descriptor) {
            entry.insert(
                catalog
                    .descriptors()
                    .type_fingerprint(origin.descriptor)
                    .map_err(|error| {
                        RevisionTypeBridgeError::Projection(
                            etas_frontend::code::descriptor::ProjectionError::Descriptor(error),
                        )
                    })?,
            );
        }
    }
    Ok(keys)
}

fn correspond(
    from: &DefinitionCatalog,
    from_keys: &HashMap<TypeInfoRef, CodeFingerprint>,
    to: &DefinitionCatalog,
    to_keys: &HashMap<TypeInfoRef, CodeFingerprint>,
) -> Result<BTreeMap<(GenericOwner, TypeId), Option<TypeId>>, RevisionTypeBridgeError> {
    let mut targets = BTreeMap::new();
    for origin in to.native_type_origins() {
        let key = (origin.scope.clone(), key(to_keys, origin.descriptor)?);
        if let Some(previous) = targets.insert(key, origin.local)
            && previous != origin.local
        {
            return Err(RevisionTypeBridgeError::AmbiguousType {
                scope: origin.scope.clone(),
                ty: origin.local,
            });
        }
    }
    from.native_type_origins()
        .iter()
        .map(|origin| {
            let target = targets
                .get(&(origin.scope.clone(), key(from_keys, origin.descriptor)?))
                .copied();
            Ok(((origin.scope.clone(), origin.local), target))
        })
        .collect()
}

fn key(
    keys: &HashMap<TypeInfoRef, CodeFingerprint>,
    ty: TypeInfoRef,
) -> Result<CodeFingerprint, RevisionTypeBridgeError> {
    keys.get(&ty).copied().ok_or_else(|| {
        RevisionTypeBridgeError::Projection(
            etas_frontend::code::descriptor::ProjectionError::Invalid(
                "native type origin has no checked descriptor identity".into(),
            ),
        )
    })
}

fn checked_callable_template_scope(
    source: &CheckedProgram,
    callables: &BTreeMap<etas_code::CallableId, etas_hir::HirItemId>,
    ty: TypeId,
) -> Option<GenericOwner> {
    let etas_types::Type::Function(flow) = source.type_store.get(ty)? else {
        return None;
    };
    let matches = |signature: &etas_types::CallableSignature| {
        !signature.generic_params.is_empty()
            && signature.params == flow.input
            && signature.output == flow.output
            && signature.effects == flow.effects
    };
    for (callable, item) in callables {
        let signature = match source.types.item_signatures.get(item) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => signature,
            _ => continue,
        };
        if matches(signature) {
            return callable
                .as_declaration()
                .map(|definition| GenericOwner::Declaration(definition.clone()))
                .or_else(|| {
                    callable
                        .as_method()
                        .map(|method| GenericOwner::Method(method.clone()))
                });
        }
    }
    for (symbol, signature) in &source.types.std_callable_signatures {
        if matches(signature) {
            let callable = &source.std_registry.body_binding(*symbol)?.callable;
            return callable
                .as_declaration()
                .map(|definition| GenericOwner::Declaration(definition.clone()));
        }
    }
    None
}
