use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use etas_frontend::CheckedProject;
use etas_types::{Type, TypeId};

use super::{CheckedTypeIdentity, DeclarationIdentity};

pub(crate) struct CheckedTypeCatalog {
    pub(super) program: CheckedProject,
    pub(super) compilation: [u8; 32],
    declarations: HashMap<TypeId, (DeclarationIdentity, usize)>,
}

impl std::fmt::Debug for CheckedTypeCatalog {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("CheckedTypeCatalog")
            .field("declarations", &self.declarations.len())
            .finish_non_exhaustive()
    }
}

impl CheckedTypeCatalog {
    pub(crate) fn build(program: &CheckedProject) -> Result<Self, String> {
        let compilation = compilation_identity(program);
        let mut declarations = HashMap::new();
        for (owner, layout) in &program.types.enum_layouts {
            if !matches!(program.type_store.get(*owner), Some(Type::Enum(_))) {
                return Err("checked enum layout has no matching declaration".into());
            }
            let mut names = std::collections::HashSet::new();
            for variant in &layout.variants {
                if !names.insert(&variant.name) {
                    return Err("checked enum layout has duplicate constructors".into());
                }
                if let Some(fields) = &variant.field_names {
                    let unique = fields.iter().collect::<std::collections::HashSet<_>>();
                    if fields.len() != variant.fields.len() || unique.len() != fields.len() {
                        return Err("checked named constructor has an invalid field layout".into());
                    }
                }
            }
        }
        for (ty, data) in program.type_store.iter() {
            let arity = match data {
                Type::Nominal(declaration) => declaration.params.len(),
                Type::Enum(_) => {
                    // Interned intermediate nodes are not declarations. Only
                    // checked layouts authenticate an enum owner; resolving an
                    // unregistered owner still fails in declaration().
                    let Some(layout) = program.types.enum_layouts.get(&ty) else {
                        continue;
                    };
                    layout.type_params.len()
                }
                Type::Named(_) => {
                    if let Some(parameters) = program.types.type_generics.get(&ty)
                        && program.types.known_std_types.opaque_values.contains(&ty)
                    {
                        parameters.len()
                    } else if let Some(role) = program
                        .types
                        .known_std_types
                        .runtime_types
                        .iter()
                        .find_map(|(role, candidate)| (*candidate == ty).then_some(role))
                    {
                        role.type_parameter_count()
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };
            declarations.insert(
                ty,
                (
                    DeclarationIdentity {
                        compilation,
                        declaration: ty.0,
                    },
                    arity,
                ),
            );
        }
        Ok(Self {
            program: program.clone(),
            compilation,
            declarations,
        })
    }

    pub(super) fn declaration(
        &self,
        template: TypeId,
        arity: usize,
    ) -> Result<DeclarationIdentity, String> {
        let (identity, expected) = self
            .declarations
            .get(&template)
            .ok_or("type has no authenticated checked declaration")?;
        if *expected != arity {
            return Err(format!(
                "checked declaration requires {expected} arguments, got {arity}"
            ));
        }
        Ok(*identity)
    }

    pub(crate) fn declaration_template(
        &self,
        identity: DeclarationIdentity,
    ) -> Result<TypeId, String> {
        if identity.compilation != self.compilation {
            return Err("type declaration belongs to a different checked compilation".into());
        }
        let template = TypeId(identity.declaration);
        match self.declarations.get(&template) {
            Some((checked, _)) if *checked == identity => Ok(template),
            _ => Err("type declaration is absent from the checked catalog".into()),
        }
    }

    pub(crate) fn validate_identity(&self, identity: &CheckedTypeIdentity) -> Result<(), String> {
        // Only this module constructs the opaque, immutable expression. A wire
        // artifact must authenticate each declaration before constructing it;
        // it cannot deserialize this sealed value directly.
        if identity.compilation() != self.compilation {
            return Err("type identity belongs to a different checked compilation".into());
        }
        if let Some(declaration) = identity.declaration() {
            let template = self.declaration_template(declaration)?;
            self.declaration(template, identity.arguments().len())?;
        }
        Ok(())
    }
}

struct CompilationHash(blake3::Hasher);
impl Hasher for CompilationHash {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(&(bytes.len() as u64).to_le_bytes());
        self.0.update(bytes);
    }
    fn finish(&self) -> u64 {
        let hash = self.0.finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("eight hash bytes"))
    }
}

/// Compilation-local declaration identities are re-authenticated at restore and
/// revision boundaries. They are not portable package DefinitionKeys, and must
/// never be reconstructed from a nominal display name or a value's shape.
fn compilation_identity(program: &CheckedProject) -> [u8; 32] {
    let mut hash = CompilationHash(blake3::Hasher::new_derive_key(
        "etas.checked-runtime-type-universe.v1",
    ));
    program.compiler_version.hash(&mut hash);
    program.project_environment_fingerprint.hash(&mut hash);
    program.dependency_metadata_fingerprints.hash(&mut hash);
    program.std_registry.version().contract.hash(&mut hash);
    for source in &program.sources.sources {
        source.id.hash(&mut hash);
        source.path.hash(&mut hash);
        source.text().hash(&mut hash);
    }
    for (id, ty) in program.type_store.iter() {
        id.hash(&mut hash);
        ty.hash(&mut hash);
    }
    let mut layouts = program.types.enum_layouts.iter().collect::<Vec<_>>();
    layouts.sort_unstable_by_key(|(owner, _)| **owner);
    for (owner, layout) in layouts {
        owner.hash(&mut hash);
        layout.type_params.hash(&mut hash);
        for variant in &layout.variants {
            variant.name.hash(&mut hash);
            variant.fields.hash(&mut hash);
            variant.field_names.hash(&mut hash);
        }
    }
    *hash.0.finalize().as_bytes()
}
