use etas_frontend::{
    CheckedProgram,
    code::definition::{DefinitionCatalog, NativeTypeCorrespondence},
};
use etas_types::{
    TypeId, TypeSubstitutionEngine,
    ty::{TypeLookupIndex, TypeRelocationInput},
};

use super::RevisionTypeBridgeError as Error;

pub(super) struct RuntimeTypeBridge {
    pub forward: Direction,
    pub backward: Direction,
}

pub(super) struct Direction {
    correspondence: NativeTypeCorrespondence,
    target_types: TypeLookupIndex,
}

impl RuntimeTypeBridge {
    pub fn new(original: &DefinitionCatalog, candidate: &DefinitionCatalog) -> Result<Self, Error> {
        Ok(Self {
            forward: Direction::new(original, candidate)?,
            backward: Direction::new(candidate, original)?,
        })
    }
}

impl Direction {
    fn new(source: &DefinitionCatalog, target: &DefinitionCatalog) -> Result<Self, Error> {
        let to = target.checked_program();
        Ok(Self {
            correspondence: NativeTypeCorrespondence::build(source, target)
                .map_err(Error::Projection)?,
            target_types: TypeLookupIndex::new(&to.type_store),
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
        TypeSubstitutionEngine::relocating(TypeRelocationInput {
            source: &source.type_store,
            target: &target.type_store,
            declaration_types: self.correspondence.declarations(),
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
            declaration_types: self.correspondence.declarations(),
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
