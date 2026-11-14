use std::{collections::BTreeMap, sync::Arc};

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
    callables: BTreeMap<StdSymbolId, LibraryCallable>,
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
            RevisionTypeBridge::for_standard_library(&caller, &bodies)
                .map_err(|e| e.to_string())?,
        );
        let prepared = super::prepare_program(library)
            .map_err(|errors| format!("std body preparation failed: {errors:?}"))?;
        let mut callables = BTreeMap::new();
        for symbol in program.std_registry.symbols() {
            if !program.symbols.iter().any(|import| matches!(&import.def, etas_hir::SymbolDef::ImportAlias { path, .. } if program.std_registry.lookup_qualified(path).is_some_and(|decl| decl.id == symbol.id))) {
                continue;
            }
            let Some(binding) = program.std_registry.body_binding(symbol.id) else {
                continue;
            };
            let id = &binding.callable;
            let item = bodies.item_owner(id).ok_or_else(|| {
                format!(
                    "std artifact {} is missing its checked implementation",
                    symbol.qualified_path.join(".")
                )
            })?;
            let signature = match library.types.item_signatures.get(&item) {
                Some(ItemSignature::Flow(signature)) => signature,
                _ => {
                    return Err(format!(
                        "std artifact {} body has no checked callable signature",
                        symbol.qualified_path.join(".")
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
                    "std artifact {} body differs from its public interface",
                    symbol.qualified_path.join(".")
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
                .ok_or_else(|| {
                    format!(
                        "std artifact {} has no checked function type",
                        symbol.qualified_path.join(".")
                    )
                })?;
            callables.insert(symbol.id, LibraryCallable { item, ty });
        }
        Ok(Some(Arc::new(Self {
            prepared,
            bridge,
            callables,
        })))
    }

    pub(crate) fn callable(&self, symbol: StdSymbolId) -> Result<CallTarget, String> {
        let binding = self
            .callables
            .get(&symbol)
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
        };
        let (_, value) = super::code::RevisionProjection::Original(self.bridge.clone())
            .value(
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
