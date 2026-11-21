//! Installation-time, rootless layouts. These are immutable descriptions, not
//! permission to execute or installed residual monitors. The driver authenticates
//! the bytes against the exact SDK and running toolchain before attaching them.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::{context::PlanContext, dispatch::MethodDispatchPlan};

const MAGIC: &[u8; 8] = b"ETASLAY3";

#[derive(Serialize, Deserialize)]
pub(super) struct InstalledDispatch {
    pub type_count: usize,
    pub method_count: usize,
    pub methods: Arc<MethodDispatchPlan>,
    pub pure_abi: Arc<crate::intrinsic::pure::PureAbiProjector>,
}

pub(super) type InstalledTypeFingerprints = std::collections::BTreeMap<
    etas_code::descriptor::GenericOwner,
    std::collections::BTreeMap<etas_types::TypeId, etas_code::CodeFingerprint>,
>;

#[derive(Serialize, Deserialize)]
pub(super) struct InstalledLayouts {
    /// SDK descriptor identities are fixed at installation. Only the caller's
    /// newly checked types need fingerprinting during startup.
    pub(super) type_fingerprints: InstalledTypeFingerprints,
    slots: super::SlotLayoutTable,
    frames: super::frames::FrameLayoutTable,
    arguments: super::arguments::CallArgumentTable,
    closures: super::ClosureLayoutTable,
    named_variants: super::NamedVariantLayoutTable,
    globals: super::GlobalTable,
    resources: super::ResourceTable,
    dispatch: Arc<InstalledDispatch>,
}

impl InstalledLayouts {
    pub(super) fn apply(mut self, context: &mut PlanContext<'_>) {
        let initialized = |symbol| match context
            .project
            .hir
            .symbols
            .get(symbol)
            .map(|symbol| &symbol.def)
        {
            Some(etas_hir::SymbolDef::TopLevelLet { item, .. }) => {
                matches!(context.project.hir.items.get(*item), Some(etas_hir::HirItem::TopLevelLet(value)) if value.value.is_some())
            }
            _ => false,
        };
        self.globals
            .constants
            .retain(|global| initialized(global.symbol));
        self.resources
            .handles
            .retain(|resource| initialized(resource.symbol));
        context.slots = Some(self.slots);
        context.frames = Some(self.frames);
        context.arguments = Some(self.arguments);
        context.closures = Some(self.closures);
        context.named_variants = Some(self.named_variants);
        context.globals = Some(self.globals);
        context.resources = Some(self.resources);
        context.installed_layouts = Some(self.dispatch);
    }
}

/// Prepare the static SDK-side layouts once, while installing the toolchain.
/// Programs with residual monitors retain their ordinary preparation path.
pub fn encode_installed_layouts(
    program: &etas_frontend::CheckedProgram,
) -> Result<Option<Vec<u8>>, String> {
    if !program.residual_plans().is_empty() {
        return Ok(None);
    }
    let prepared = super::prepare_program(program)
        .map_err(|errors| format!("cannot prepare installed std layouts: {errors:?}"))?;
    let (methods, pure_abi) = prepared.dispatch.installed_dispatch();
    let catalog = super::standard_library::catalog(program)?;
    let image = InstalledLayouts {
        type_fingerprints: {
            let fingerprints = super::code::fingerprints(&catalog).map_err(|e| e.to_string())?;
            let mut grouped = InstalledTypeFingerprints::new();
            for origin in catalog.native_type_origins() {
                grouped
                    .entry(origin.scope.clone())
                    .or_default()
                    .insert(origin.local, fingerprints[&origin.descriptor]);
            }
            grouped
        },
        slots: prepared.slots.as_ref().clone(),
        frames: prepared.frames.as_ref().clone(),
        arguments: prepared.arguments.as_ref().clone(),
        closures: prepared.closures.as_ref().clone(),
        named_variants: prepared.named_variants.as_ref().clone(),
        globals: prepared.globals.as_ref().clone(),
        resources: prepared.resources.as_ref().clone(),
        dispatch: Arc::new(InstalledDispatch {
            type_count: program.type_store.len(),
            method_count: program.types.methods.entries().count(),
            methods,
            pure_abi,
        }),
    };
    let mut payload = MAGIC.to_vec();
    payload.extend(
        bincode::serde::encode_to_vec(&image, bincode::config::standard())
            .map_err(|e| e.to_string())?,
    );
    Ok(Some(payload))
}

pub(super) fn decode(bytes: &[u8]) -> Result<InstalledLayouts, String> {
    let payload = bytes
        .strip_prefix(MAGIC)
        .ok_or("incompatible installed std layout schema")?;
    let (image, used) = bincode::serde::decode_from_slice(
        payload,
        bincode::config::standard().with_limit::<536_870_912>(),
    )
    .map_err(|e| format!("cannot decode installed std layouts: {e}"))?;
    if used != payload.len() {
        return Err("trailing bytes in installed std layouts".into());
    }
    Ok(image)
}
