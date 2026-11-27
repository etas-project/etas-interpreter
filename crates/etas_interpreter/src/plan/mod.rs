mod action_mediation;
mod arguments;
mod arithmetic;
#[cfg(test)]
mod callable_tests;
mod captures;
pub(crate) mod checked_action;
pub mod code;
mod context;
mod dispatch;
mod entry;
mod frames;
mod globals;
pub(crate) mod installed;
pub(crate) mod iteration;
mod pipeline;
pub(crate) mod readiness;
mod records;
pub(crate) mod residual;
mod resources;
mod slots;
mod standard_library;
mod types;
pub(crate) use types::ExpressionRoute;
mod validate;
mod variants;

pub use action_mediation::ActionMediationTable;
pub use captures::ClosureLayoutTable;
pub use dispatch::{BraceLiteralShape, IntrinsicDispatchTable};
pub use globals::GlobalTable;
pub use installed::encode_installed_layouts;
pub use pipeline::build_plan;
#[cfg(test)]
pub(crate) use pipeline::build_plan_for_entry;
pub(crate) use pipeline::{build_plan_with_residual_binding, prepare_program};
pub use readiness::HostRequirementTable;
pub(crate) use records::FieldAccessSite;
pub use records::RecordLayoutTable;
pub use resources::ResourceTable;
pub use slots::SlotLayoutTable;
pub use types::InterpreterPlan;
pub(crate) use types::PreparedProgram;
pub use variants::NamedVariantLayoutTable;

/// Projecting an SDK callable can retain distinct slots for the same checked function type.
pub(crate) fn same_checked_abi_type(
    store: &etas_types::TypeStore,
    left: etas_types::TypeId,
    right: etas_types::TypeId,
) -> bool {
    left == right
        || matches!((store.get(left), store.get(right)),
        (Some(etas_types::Type::Function(left)), Some(etas_types::Type::Function(right))) if left == right)
}
