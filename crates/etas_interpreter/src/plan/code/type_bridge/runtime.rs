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

pub(super) struct RuntimeTypeBridge {
    pub forward: Direction,
    pub backward: Direction,
}

pub(super) struct Direction {
    correspondence: NativeTypeCorrespondence,
    target_types: TypeLookupIndex,
    checked_types: HashMap<TypeId, Option<TypeId>>,
    relocation_types: HashMap<TypeId, TypeId>,
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
        let to = target.checked_program();
        let mut checked_types = HashMap::new();
        for ((_, ty), target) in checked {
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
        Ok(Self {
            correspondence,
            target_types: TypeLookupIndex::new(&to.type_store),
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
            target_types: &self.target_types,
            effect_names: self.correspondence.effect_names(),
        })
        .substitute(ty)
        .map_err(Error::RuntimeType)
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
            target_types: &self.target_types,
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
            target_types: &self.target_types,
            effect_names: self.correspondence.effect_names(),
        })
        .substitute(ty)
        .map_err(Error::RuntimeType)
    }
}
