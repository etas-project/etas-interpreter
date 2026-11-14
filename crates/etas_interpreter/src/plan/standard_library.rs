use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use etas_frontend::{
    CheckedProgram,
    code::{
        definition::DefinitionCatalog,
        identity::{DefinitionOrigins, standard_package_key},
    },
};
use etas_hir::HirItemId;
use etas_std::StdSymbolId;
use etas_types::{FlowType, ItemSignature, Type, TypeId};

use super::{PreparedProgram, code::RevisionTypeBridge};
use crate::{
    control::CallTarget,
    value::{CheckedExecutionProgram, InterpValue},
};

#[derive(Debug)]
pub(crate) struct StandardLibraryProgram {
    prepared: Arc<PreparedProgram>,
    bridge: Arc<RevisionTypeBridge>,
    aliases: BTreeMap<StdSymbolId, etas_code::CallableId>,
    callables: BTreeMap<etas_code::CallableId, LibraryCallable>,
}

#[derive(Debug)]
struct LibraryCallable {
    item: HirItemId,
    ty: TypeId,
}

impl StandardLibraryProgram {
    pub(super) fn prepare(program: &CheckedProgram) -> Result<Option<Arc<Self>>, String> {
        let Some(standard) = &program.standard_library else {
            return Ok(None);
        };
        let library = &standard.program;
        let caller = catalog(program)?;
        let bodies = catalog(library)?;
        let bridge = Arc::new(
            RevisionTypeBridge::for_standard_library(&caller, &bodies, standard)
                .map_err(|e| e.to_string())?,
        );
        let prepared = super::prepare_program(library)
            .map_err(|errors| format!("std body preparation failed: {errors:?}"))?;
        let mut callables = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for symbol in program.std_registry.symbols() {
            if !program.symbols.iter().any(|import| matches!(&import.def, etas_hir::SymbolDef::ImportAlias { path, .. } if program.std_registry.lookup_qualified(path).is_some_and(|decl| decl.id == symbol.id))) {
                continue;
            }
            let Some(binding) = program.std_registry.body_binding(symbol.id) else {
                continue;
            };
            aliases.insert(symbol.id, binding.callable.clone());
            identities.insert(binding.callable.clone());
        }
        for method in program.types.methods.entries() {
            if let etas_types::method::MethodTarget::External { identity } = &method.target {
                let callable = etas_code::CallableId::method(identity.clone());
                if bodies.item_owner(&callable).is_some() {
                    identities.insert(callable);
                }
            }
        }
        for identity in identities {
            let id = &identity;
            let item = bodies.item_owner(id).ok_or_else(|| {
                format!("std artifact {id:?} is missing its checked implementation")
            })?;
            let signature = match library.types.item_signatures.get(&item) {
                Some(ItemSignature::Flow(signature)) => signature,
                _ => {
                    return Err(format!(
                        "std artifact {id:?} body has no checked callable signature"
                    ));
                }
            };
            let expected = caller
                .callable(id)
                .ok_or("std query has no canonical callable interface")?;
            let actual = bodies
                .callable(id)
                .ok_or("std body has no canonical callable interface")?;
            if caller
                .callable_descriptors(id)
                .ok_or("std query has no callable descriptors")?
                .type_fingerprint(expected.signature)
                .map_err(|e| e.to_string())?
                != bodies
                    .callable_descriptors(id)
                    .ok_or("std body has no callable descriptors")?
                    .type_fingerprint(actual.signature)
                    .map_err(|e| e.to_string())?
            {
                return Err(format!(
                    "std artifact {id:?} body differs from its public interface"
                ));
            }
            let contract = library
                .effects
                .public_contracts
                .iter()
                .find(|contract| contract.item == item)
                .ok_or("std body has no checked public effect contract")?;
            let parameters = signature
                .generic_params
                .iter()
                .filter(|param| param.kind == etas_types::CallableGenericParamKind::Effect)
                .map(|param| param.name.clone())
                .collect::<Vec<_>>();
            let row = etas_effects::facts::checked_type_effect_row(
                &contract.public_row,
                &library.effect_registry,
                &parameters,
            )?;
            let flow = FlowType {
                input: signature.params.clone(),
                output: signature.output,
                effects: Some(row),
            };
            let ty = library
                .type_store
                .iter()
                .find_map(|(id, ty)| {
                    matches!(ty, Type::Function(candidate) if candidate == &flow).then_some(id)
                })
                .ok_or_else(|| format!("std artifact {id:?} has no checked function type"))?;
            callables.insert(identity, LibraryCallable { item, ty });
        }
        Ok(Some(Arc::new(Self {
            prepared,
            bridge,
            aliases,
            callables,
        })))
    }

    pub(crate) fn callable(&self, symbol: StdSymbolId) -> Result<CallTarget, String> {
        let identity = self
            .aliases
            .get(&symbol)
            .ok_or("std artifact has no canonical callable binding")?;
        self.project(identity)
    }

    pub(crate) fn method(&self, method: &etas_code::MethodId) -> Result<CallTarget, String> {
        self.project(&etas_code::CallableId::method(method.clone()))
    }

    fn project(&self, identity: &etas_code::CallableId) -> Result<CallTarget, String> {
        let binding = self
            .callables
            .get(identity)
            .ok_or("std artifact is missing a checked body binding")?;
        let checked = self.prepared.checked();
        let plan = self
            .prepared
            .select_callable(etas_frontend::CheckedCallableRoot::Item(binding.item))
            .map_err(|e| format!("std body has no execution plan: {e:?}"))?;
        let execution = CheckedExecutionProgram {
            checked: checked.clone(),
            plan: Arc::new(plan),
            definitions: None,
            revision: None,
            foreign: None,
        };
        let scope = match identity.as_method() {
            Some(method) => etas_code::descriptor::GenericOwner::Method(method.clone()),
            None => etas_code::descriptor::GenericOwner::Declaration(
                identity
                    .as_declaration()
                    .ok_or("std callable has no canonical owner")?
                    .clone(),
            ),
        };
        let value = self
            .bridge
            .value_to_original_scoped(
                checked,
                &scope,
                binding.ty,
                InterpValue::Callable(CallTarget::FlowItem(binding.item)),
                &execution,
            )
            .map_err(|e| e.to_string())?;
        let InterpValue::Callable(target) = value else {
            return Err("std artifact callable projection produced a non-callable value".into());
        };
        Ok(target)
    }
}

fn catalog(program: &CheckedProgram) -> Result<DefinitionCatalog, String> {
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::checked_source_modules(program)
            .map_err(|e| e.to_string())?,
        standard_package: standard_package_key(&program.std_registry).map_err(|e| e.to_string())?,
        external_definitions: DefinitionOrigins::checked_external_definitions(program)
            .map_err(|e| e.to_string())?,
    };
    DefinitionCatalog::build(program.clone(), &origins, Default::default())
        .map_err(|e| e.to_string())
}
