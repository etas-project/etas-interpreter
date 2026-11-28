use etas_frontend::{
    CheckedProgram,
    code::definition::{DefinitionCatalog, NativeTypeCorrespondence},
};
use etas_types::{
    TypeId, TypeSubstitutionEngine,
    ty::{TypeLookupIndex, TypeRelocationInput},
};
use std::collections::{BTreeMap, HashMap};

use super::RevisionTypeBridgeError as Error;

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct RuntimeTypeBridge {
    pub forward: Direction,
    pub backward: Direction,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Direction {
    correspondence: NativeTypeCorrespondence,
    #[serde(skip)]
    pub(super) target_types: std::sync::OnceLock<TypeLookupIndex>,
    checked_types: HashMap<TypeId, Option<TypeId>>,
    relocation_types: HashMap<TypeId, TypeId>,
    scoped_types: BTreeMap<(etas_code::descriptor::GenericOwner, TypeId), Option<TypeId>>,
    effect_parameters: BTreeMap<etas_code::descriptor::GenericOwner, HashMap<String, String>>,
}

impl RuntimeTypeBridge {
    pub fn new(
        original: &DefinitionCatalog,
        candidate: &DefinitionCatalog,
        forward: &BTreeMap<(etas_code::descriptor::GenericOwner, TypeId), Option<TypeId>>,
        backward: &BTreeMap<(etas_code::descriptor::GenericOwner, TypeId), Option<TypeId>>,
    ) -> Result<Self, Error> {
        Ok(Self {
            forward: Direction::new(original, candidate, forward)?,
            backward: Direction::new(candidate, original, backward)?,
        })
    }
}

impl Direction {
    fn new(
        source: &DefinitionCatalog,
        target: &DefinitionCatalog,
        checked: &BTreeMap<(etas_code::descriptor::GenericOwner, TypeId), Option<TypeId>>,
    ) -> Result<Self, Error> {
        let mut scoped_types = checked.clone();
        let mut effect_parameters = BTreeMap::<_, HashMap<_, _>>::new();
        for (library, owner, reverse) in [
            (target.checked_program(), source.checked_program(), false),
            (source.checked_program(), target.checked_program(), true),
        ] {
            for identity in library.foreign_methods() {
                let (origin, _, _) = library
                    .foreign_method(identity)
                    .expect("listed checked foreign method");
                if !std::sync::Arc::ptr_eq(origin.hir.shared_program(), owner.hir.shared_program())
                {
                    continue;
                }
                let method = crate::plan::code::checked_foreign_method(library, owner, identity)
                    .map_err(Error::InvalidRuntimeInterface)?;
                for (from, to, scope) in [
                    (
                        &method.source.owner_params,
                        &method.imported.owner_params,
                        etas_code::descriptor::GenericOwner::Implementation(
                            identity.implementation().clone(),
                        ),
                    ),
                    (
                        &method.source.signature.generic_params,
                        &method.imported.signature.generic_params,
                        etas_code::descriptor::GenericOwner::Method(identity.clone()),
                    ),
                ] {
                    if from.len() != to.len() || from.iter().zip(to).any(|(a, b)| a.kind != b.kind)
                    {
                        return Err(Error::InvalidRuntimeInterface(
                            "foreign method changed its checked binder shape".into(),
                        ));
                    }
                    for (from, to) in from.iter().zip(to) {
                        let (from, to) = if reverse { (to, from) } else { (from, to) };
                        if from.kind == etas_types::CallableGenericParamKind::Type {
                            for scope in [
                                scope.clone(),
                                etas_code::descriptor::GenericOwner::Method(identity.clone()),
                            ] {
                                let key = (scope, from.subject);
                                if scoped_types
                                    .get(&key)
                                    .copied()
                                    .flatten()
                                    .is_some_and(|previous| previous != to.subject)
                                {
                                    return Err(Error::InvalidRuntimeInterface("foreign method conflicts with an existing checked binder correspondence".into()));
                                }
                                scoped_types.insert(key, Some(to.subject));
                            }
                        } else {
                            effect_parameters
                                .entry(scope.clone())
                                .or_default()
                                .insert(from.name.clone(), to.name.clone());
                        }
                    }
                }
            }
        }
        // A bundled method's impl binders are not callable-owned binders.
        // Relate them through the checked canonical method import as well.
        for (owner, importer, reverse) in [(source, target, false), (target, source, true)] {
            for imported in &importer.checked_program().types.external_methods {
                let callable = etas_code::CallableId::method(imported.identity.clone());
                let Some(item) = owner.item_owner(&callable) else {
                    continue;
                };
                let Some(authored) = owner
                    .checked_program()
                    .types
                    .type_impl_methods
                    .values()
                    .find(|method| method.body == item)
                else {
                    continue;
                };
                for (from, to) in authored.owner_params.iter().zip(&imported.owner_params) {
                    if from.kind != etas_types::CallableGenericParamKind::Type {
                        continue;
                    }
                    let (from, to) = if reverse { (to, from) } else { (from, to) };
                    for scope in [
                        etas_code::descriptor::GenericOwner::Implementation(
                            imported.identity.implementation().clone(),
                        ),
                        etas_code::descriptor::GenericOwner::Method(imported.identity.clone()),
                    ] {
                        scoped_types.insert((scope, from.subject), Some(to.subject));
                    }
                }
            }
        }
        let mut checked_types = HashMap::new();
        for ((_, ty), target) in &scoped_types {
            if let Some(target) = target {
                checked_types
                    .entry(*ty)
                    .and_modify(|previous| {
                        if *previous != Some(*target) {
                            *previous = None;
                        }
                    })
                    .or_insert(Some(*target));
            }
        }
        let correspondence =
            NativeTypeCorrespondence::build(source, target).map_err(Error::Projection)?;
        let mut relocation_types = correspondence.declarations().clone();
        for (source, target) in &checked_types {
            if let Some(target) = target {
                relocation_types.entry(*source).or_insert(*target);
            }
        }
        for parameter in &source.descriptors().data().binders {
            if parameter.kind != etas_code::descriptor::GenericParameterKind::EffectRow {
                continue;
            }
            if let Some(to) = target.descriptors().data().binders.iter().find(|to| {
                to.owner == parameter.owner
                    && to.position == parameter.position
                    && to.kind == parameter.kind
            }) {
                effect_parameters
                    .entry(parameter.owner.clone())
                    .or_default()
                    .insert(parameter.name.clone(), to.name.clone());
            }
        }
        Ok(Self {
            scoped_types,
            effect_parameters,
            correspondence,
            target_types: Default::default(),
            checked_types,
            relocation_types,
        })
    }

    pub fn get(
        &self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        if let Some(mapped) = self.correspondence.declarations().get(&ty) {
            return Ok(*mapped);
        }
        if let Some(mapped) = self.checked_types.get(&ty) {
            return mapped.ok_or_else(|| {
                Error::InvalidRuntimeInterface(
                    "runtime type has conflicting checked descriptor correspondences".into(),
                )
            });
        }
        TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source.type_store,
            target: &target.type_store,
            declaration_types: &self.relocation_types,
            target_types: self
                .target_types
                .get_or_init(|| TypeLookupIndex::new(&target.type_store)),
            effect_names: self.correspondence.effect_names(),
        })
        .substitute(ty)
        .map_err(|error| relocation_error(source, target, ty, None, error))
    }

    pub fn get_in_scope(
        &self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        scope: &etas_code::descriptor::GenericOwner,
        ty: TypeId,
    ) -> Result<TypeId, Error> {
        if let Some(mapped) = self.scoped_types.get(&(scope.clone(), ty)) {
            return mapped.ok_or_else(|| Error::MissingType {
                scope: scope.clone(),
                ty,
            });
        }
        let mut declarations = self.correspondence.declarations().clone();
        for ((owner, from), to) in &self.scoped_types {
            if owner == scope
                && let Some(to) = to
            {
                declarations.entry(*from).or_insert(*to);
            }
        }
        let empty = HashMap::new();
        TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source.type_store,
            target: &target.type_store,
            declaration_types: &declarations,
            target_types: self
                .target_types
                .get_or_init(|| TypeLookupIndex::new(&target.type_store)),
            effect_names: self.correspondence.effect_names(),
        })
        .with_relocated_effect_parameters(self.effect_parameters.get(scope).unwrap_or(&empty))
        .substitute(ty)
        .map_err(|error| relocation_error(source, target, ty, Some(scope), error))
    }

    pub fn row(
        &self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        row: etas_types::EffectRowRef,
    ) -> Result<etas_types::EffectRowRef, Error> {
        TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source.type_store,
            target: &target.type_store,
            declaration_types: &self.relocation_types,
            target_types: self
                .target_types
                .get_or_init(|| TypeLookupIndex::new(&target.type_store)),
            effect_names: self.correspondence.effect_names(),
        })
        .substitute_effect_row(row)
        .map_err(Error::RuntimeType)
    }

    pub fn get_with_parameters(
        &self,
        source: &CheckedProgram,
        target: &CheckedProgram,
        ty: TypeId,
        parameters: &[(TypeId, TypeId)],
        effect_parameters: &HashMap<String, String>,
    ) -> Result<TypeId, Error> {
        let mut declarations = self.correspondence.declarations().clone();
        for (from, to) in parameters {
            if declarations
                .insert(*from, *to)
                .is_some_and(|previous| previous != *to)
            {
                return Err(Error::InvalidRuntimeInterface(
                    "generic action parameter conflicts with a type declaration".into(),
                ));
            }
        }
        TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source.type_store,
            target: &target.type_store,
            declaration_types: &declarations,
            target_types: self
                .target_types
                .get_or_init(|| TypeLookupIndex::new(&target.type_store)),
            effect_names: self.correspondence.effect_names(),
        })
        .with_relocated_effect_parameters(effect_parameters)
        .substitute(ty)
        .map_err(Error::RuntimeType)
    }
}

fn relocation_error(
    source: &CheckedProgram,
    target: &CheckedProgram,
    ty: TypeId,
    scope: Option<&etas_code::descriptor::GenericOwner>,
    error: etas_types::TypeSubstitutionError,
) -> Error {
    let etas_types::TypeSubstitutionError::UnmaterializedType(missing) = &error else {
        return Error::RuntimeType(error);
    };
    let constructor = match missing {
        etas_types::Type::Applied { constructor, .. } => {
            target.type_store.get(TypeId(constructor.0))
        }
        _ => None,
    };
    Error::InvalidRuntimeInterface(format!(
        "type relocation {:?} -> {:?}, source {ty:?} {:?}, scope {scope:?}: {error}; target constructor {constructor:?}",
        source.compile_options.standard_library_mode,
        target.compile_options.standard_library_mode,
        source.type_store.get(ty),
    ))
}
