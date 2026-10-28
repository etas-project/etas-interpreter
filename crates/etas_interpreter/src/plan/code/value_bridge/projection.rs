use std::collections::HashMap;

use etas_frontend::CheckedProject;
use etas_types::{Type, TypeId, nominal_representation_parts, substitute_named_params_in_store};

use super::RevisionValueFailure as Failure;

pub(super) fn workspace_path_region(program: &CheckedProject, ty: TypeId) -> Result<&str, Failure> {
    let Some(Type::Applied { constructor, args }) = program.type_store.get(ty) else {
        return Err(Failure::UnsupportedType);
    };
    if program
        .types
        .known_std_types
        .runtime_type(etas_std::RuntimeTypeRole::WorkspacePath)
        != Some(TypeId(constructor.0))
        || args.len() != 1
    {
        return Err(Failure::UnsupportedType);
    }
    let Some(Type::Nominal(region)) = program.type_store.get(args[0]) else {
        return Err(Failure::UnsupportedType);
    };
    Ok(&region.name)
}

#[derive(Default)]
pub(crate) struct CheckedValueTypes {
    representations: HashMap<TypeId, TypeId>,
    variants: HashMap<TypeId, HashMap<String, Variant>>,
}

pub(crate) struct Variant {
    pub fields: Vec<TypeId>,
    pub names: Option<Vec<String>>,
}

impl CheckedValueTypes {
    pub fn representation(
        &mut self,
        program: &CheckedProject,
        ty: TypeId,
    ) -> Result<TypeId, Failure> {
        if let Some(representation) = self.representations.get(&ty) {
            return Ok(*representation);
        }
        let (representation, bindings) = nominal_representation_parts(&program.type_store, ty)
            .ok_or(Failure::MissingRepresentation)?;
        let representation =
            substitute_named_params_in_store(&program.type_store, representation, &bindings)
                .map_err(|reason| Failure::UnmaterializedType(reason.to_string()))?;
        self.representations.insert(ty, representation);
        Ok(representation)
    }

    pub fn variant(
        &mut self,
        program: &CheckedProject,
        ty: TypeId,
        name: &str,
    ) -> Result<&Variant, Failure> {
        let variants = self.variants.entry(ty).or_default();
        if !variants.contains_key(name) {
            let (constructor, args) =
                enum_constructor(program, ty).ok_or(Failure::MissingEnumLayout)?;
            let layout = program
                .types
                .enum_layouts
                .get(&constructor)
                .ok_or(Failure::MissingEnumLayout)?;
            if layout.type_params.len() != args.len() {
                return Err(Failure::WrongArity {
                    expected: layout.type_params.len(),
                    actual: args.len(),
                });
            }
            let variant = layout
                .variants
                .iter()
                .find(|variant| variant.name == name)
                .ok_or_else(|| Failure::UnknownVariant(name.into()))?;
            let bindings = layout
                .type_params
                .iter()
                .cloned()
                .zip(args.iter().copied())
                .collect();
            let fields = variant
                .fields
                .iter()
                .map(|ty| {
                    substitute_named_params_in_store(&program.type_store, *ty, &bindings)
                        .map_err(|reason| Failure::UnmaterializedType(reason.to_string()))
                })
                .collect::<Result<_, _>>()?;
            variants.insert(
                name.into(),
                Variant {
                    fields,
                    names: variant.field_names.clone(),
                },
            );
        }
        variants.get(name).ok_or(Failure::InconsistentLayout)
    }
}

pub(super) fn enum_constructor(
    program: &CheckedProject,
    ty: TypeId,
) -> Option<(TypeId, &[TypeId])> {
    match program.type_store.get(ty)? {
        Type::Enum(_) => Some((ty, &[])),
        Type::Applied { constructor, args }
            if matches!(
                program.type_store.get(TypeId(constructor.0)),
                Some(Type::Enum(_))
            ) =>
        {
            Some((TypeId(constructor.0), args))
        }
        _ => None,
    }
}
