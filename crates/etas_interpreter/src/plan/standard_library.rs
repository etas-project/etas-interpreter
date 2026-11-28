use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
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
use etas_types::{FlowType, ItemSignature, Type, TypeId, ty::TypeLookupIndex};

use super::{PreparedProgram, code::RevisionTypeBridge};
use crate::{
    control::CallTarget,
    value::{CheckedExecutionProgram, InterpValue},
};

#[derive(Debug)]
pub(crate) struct StandardLibraryProgram {
    prepared: Arc<PreparedProgram>,
    bridge: Arc<RevisionTypeBridge>,
    bindings: LibraryBindings,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LibraryBindings {
    aliases: BTreeMap<StdSymbolId, etas_code::CallableId>,
    callables: BTreeMap<etas_code::CallableId, LibraryCallable>,
    values: BTreeMap<etas_code::DefinitionKey, LibraryValue>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct LibraryCallable {
    #[serde(skip)]
    plan: OnceLock<Result<Arc<super::InterpreterPlan>, String>>,
    item: HirItemId,
    ty: TypeId,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct LibraryValue {
    item: HirItemId,
    symbol: etas_hir::SymbolId,
    ty: TypeId,
}

/// Task-local values can contain Rc. Keep projected declarations with the run,
/// keyed by their owning library and checked identity, before call-site binding.
#[derive(Default)]
pub(crate) struct LibraryTargets {
    owner: Option<Arc<StandardLibraryProgram>>,
    targets: BTreeMap<etas_code::CallableId, CallTarget>,
}

impl LibraryTargets {
    pub(crate) fn project(
        &mut self,
        library: &Arc<StandardLibraryProgram>,
        id: &etas_code::CallableId,
    ) -> Result<CallTarget, String> {
        if !self
            .owner
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, library))
        {
            self.targets.clear();
            self.owner = Some(library.clone());
        }
        if let Some(target) = self.targets.get(id) {
            return Ok(target.clone());
        }
        let target = library.project_uncached(id)?;
        self.targets.insert(id.clone(), target.clone());
        Ok(target)
    }
}

impl StandardLibraryProgram {
    pub(crate) fn prepared(&self) -> &Arc<PreparedProgram> {
        &self.prepared
    }
    pub(crate) fn bridge(&self) -> &Arc<RevisionTypeBridge> {
        &self.bridge
    }
    pub(crate) fn parameter_default(
        &self,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
    ) -> Option<&etas_frontend::code::parameter_defaults::ParameterDefaultUnit> {
        self.prepared.arguments.default_unit(reference)
    }

    pub(crate) fn default_program(
        &self,
        reference: &etas_code::descriptor::ParameterDefaultInfo,
    ) -> Result<CheckedExecutionProgram, String> {
        let plan = self
            .prepared
            .select_parameter_default(reference)
            .map_err(|errors| {
                format!("std default expression has no checked execution plan: {errors:?}")
            })?;
        Ok(CheckedExecutionProgram {
            checked: self.prepared.program.clone(),
            plan: Arc::new(plan),
            definitions: None,
            revision: None,
            foreign: None,
        })
    }
    pub(super) fn prepare(program: &CheckedProgram) -> Result<Option<Arc<Self>>, String> {
        let Some(standard) = &program.standard_library else {
            return Ok(None);
        };
        let library = &standard.program;
        // An imported declaration does not require an executable SDK when the
        // checked consumer has no standard-library execution roots.
        if library.reachability.analysis_items.is_empty() {
            return Ok(None);
        }
        let profile = &standard.profile;
        let cache = etas_frontend::RunPreparationCache::for_owner(program);
        if let Some(cache) = cache {
            let _span = profile.span("std.runtime.restore_plan", "std");
            if let Some((layouts, tables, bindings)) =
                cache.decode_plan(true, super::pipeline::decode)
            {
                let prepared = super::pipeline::restore_layouts(
                    library,
                    layouts,
                    library.residual_plans().binding(),
                )
                .map_err(|errors| format!("std body preparation failed: {errors:?}"))?;
                let bridge = Arc::new(RevisionTypeBridge::from_installed_tables(
                    program, standard, tables,
                ));
                profile.counter("std.runtime.plan_cache_hit", 1);
                return Ok(Some(Arc::new(Self {
                    prepared,
                    bridge,
                    bindings,
                })));
            }
            profile.counter("std.runtime.plan_cache_hit", 0);
        }
        let (caller, bodies) = {
            let _phase = profile.span("std.runtime.catalogs", "std");
            (catalog(program)?, catalog(library)?)
        };
        let installed = {
            let _phase = profile.span("std.runtime.decode_layouts", "std");
            standard
                .installed_layouts
                .as_deref()
                .map(super::installed::decode)
                .transpose()?
        };
        let bridge = {
            let _phase = profile.span("std.runtime.type_bridge", "std");
            Arc::new(
                RevisionTypeBridge::for_standard_library(
                    &caller,
                    &bodies,
                    standard,
                    installed.as_ref().map(|layout| &layout.type_fingerprints),
                )
                .map_err(|e| e.to_string())?,
            )
        };
        let prepared = {
            let _phase = profile.span("std.runtime.prepare", "std");
            super::pipeline::prepare_program_with_layouts(
                library,
                library.residual_plans().binding(),
                installed,
            )
            .map_err(|errors| format!("std body preparation failed: {errors:?}"))?
        };
        let _phase = profile.span("std.runtime.bind_callables", "std");
        let mut callables = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        let mut identities = BTreeSet::new();
        for symbol in program.referenced_standard_symbols() {
            let Some(binding) = program.std_registry.body_binding(symbol) else {
                continue;
            };
            aliases.insert(symbol, binding.callable.clone());
            identities.insert(binding.callable.clone());
        }
        for identity in program.referenced_standard_methods() {
            let callable = etas_code::CallableId::method(identity);
            if bodies.item_owner(&callable).is_some() {
                identities.insert(callable);
            }
        }
        identities.extend(
            program
                .types
                .library_flow_calls
                .values()
                .map(|call| call.callable.clone()),
        );
        let library_lookup = TypeLookupIndex::new(&library.type_store);
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
            let graph = bodies
                .callable_descriptors(id)
                .ok_or("std body has no callable descriptors")?;
            let Some(etas_code::descriptor::TypeInfo::Function(signature_info)) =
                graph.type_info(actual.signature)
            else {
                return Err("std body descriptor has no function signature".into());
            };
            for parameter in &signature_info.parameters {
                if let Some(reference) = &parameter.default
                    && prepared.arguments.default_unit(reference).is_none()
                {
                    return Err(format!(
                        "std artifact {id:?} is missing its authored default expression revision"
                    ));
                }
            }
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
            let row = library.checked_standard_callable_row(&identity, item)?;
            let flow = FlowType {
                input: signature.params.clone(),
                output: signature.output,
                effects: Some(row),
            };
            let ty = library_lookup
                .get(&library.type_store, &Type::Function(flow))
                .ok_or_else(|| format!("std artifact {id:?} has no checked function type"))?;
            callables.insert(
                identity,
                LibraryCallable {
                    item,
                    ty,
                    plan: OnceLock::new(),
                },
            );
        }
        let value_keys = standard
            .origins
            .declarations
            .iter()
            .filter(|binding| binding.definition.kind() == etas_code::DefinitionKind::Value)
            .map(|binding| {
                (
                    binding
                        .definition
                        .module()
                        .iter()
                        .chain(binding.definition.declaration())
                        .cloned()
                        .collect::<Vec<_>>(),
                    binding.definition.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut values = BTreeMap::new();
        for (item, declaration) in library.items.iter() {
            let etas_hir::HirItem::TopLevelLet(declaration) = declaration else {
                continue;
            };
            let Some(path) = library.hir.declaration_path(declaration.symbol) else {
                continue;
            };
            let Some(identity) = value_keys.get(&path) else {
                continue;
            };
            let Some(ItemSignature::TopLevelLet(signature)) =
                library.types.item_signatures.get(&item)
            else {
                return Err("std value has no checked initializer signature".into());
            };
            values.insert(
                identity.clone(),
                LibraryValue {
                    item,
                    symbol: declaration.symbol,
                    ty: signature.ty,
                },
            );
        }
        drop(_phase);
        let bindings = LibraryBindings {
            aliases,
            callables,
            values,
        };
        if let Some(cache) = cache
            && cache.can_write()
            && prepared.diagnostics.is_empty()
        {
            let _store = profile.span("std.runtime.store_plan", "std");
            if let Some(bytes) =
                super::pipeline::encode(&(&prepared.layouts, &bridge.tables, &bindings))
            {
                cache.write_plan(true, &bytes);
            }
        }
        Ok(Some(Arc::new(Self {
            prepared,
            bridge,
            bindings,
        })))
    }

    pub(crate) fn value_program(
        &self,
        identity: &etas_code::DefinitionKey,
    ) -> Result<(etas_hir::SymbolId, TypeId, CheckedExecutionProgram), String> {
        let value = self
            .bindings
            .values
            .get(identity)
            .ok_or("std value has no checked initializer")?;
        let plan = self
            .prepared
            .select_callable(etas_frontend::CheckedCallableRoot::Item(value.item))
            .map_err(|errors| format!("std value has no checked execution plan: {errors:?}"))?;
        Ok((
            value.symbol,
            value.ty,
            CheckedExecutionProgram {
                checked: self.prepared.checked().clone(),
                plan: Arc::new(plan),
                definitions: None,
                revision: None,
                foreign: None,
            },
        ))
    }

    pub(crate) fn callable(
        self: &Arc<Self>,
        symbol: StdSymbolId,
        targets: &mut LibraryTargets,
    ) -> Result<CallTarget, String> {
        let identity = self
            .bindings
            .aliases
            .get(&symbol)
            .ok_or("std artifact has no canonical callable binding")?;
        targets.project(self, identity)
    }

    pub(crate) fn method(
        self: &Arc<Self>,
        method: &etas_code::MethodId,
        targets: &mut LibraryTargets,
    ) -> Result<CallTarget, String> {
        targets.project(self, &etas_code::CallableId::method(method.clone()))
    }

    fn project_uncached(&self, identity: &etas_code::CallableId) -> Result<CallTarget, String> {
        let binding = self
            .bindings
            .callables
            .get(identity)
            .ok_or("std artifact is missing a checked body binding")?;
        let checked = self.prepared.checked();
        let plan = binding
            .plan
            .get_or_init(|| {
                self.prepared
                    .select_callable(etas_frontend::CheckedCallableRoot::Item(binding.item))
                    .map(Arc::new)
                    .map_err(|e| format!("std body has no execution plan: {e:?}"))
            })
            .clone()?;
        let execution = CheckedExecutionProgram {
            checked: checked.clone(),
            plan,
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

pub(super) fn catalog(program: &CheckedProgram) -> Result<DefinitionCatalog, String> {
    let origins = DefinitionOrigins {
        source_modules: DefinitionOrigins::checked_source_modules(program)
            .map_err(|e| e.to_string())?,
        standard_package: standard_package_key(&program.std_registry).map_err(|e| e.to_string())?,
        external_definitions: DefinitionOrigins::checked_external_definitions(program)
            .map_err(|e| e.to_string())?,
    };
    DefinitionCatalog::build_for_execution(program.clone(), &origins, Default::default())
        .map_err(|e| e.to_string())
}
