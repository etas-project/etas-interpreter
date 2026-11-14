use std::sync::Arc;

use etas_frontend::CheckedProgram;
use etas_types::method::{MethodSignatureFact, MethodTarget};

pub(crate) struct CheckedForeignMethod<'a> {
    pub item: etas_hir::HirItemId,
    pub source: &'a MethodSignatureFact,
    pub imported: &'a MethodSignatureFact,
}

pub(crate) fn checked_foreign_method<'a>(
    library: &'a CheckedProgram,
    owner: &'a CheckedProgram,
    identity: &etas_code::MethodId,
) -> Result<CheckedForeignMethod<'a>, String> {
    let (origin, item, imported) = library
        .foreign_method(identity)
        .ok_or("library method has no checked source origin")?;
    if !Arc::ptr_eq(origin.hir.shared_program(), owner.hir.shared_program())
        || !Arc::ptr_eq(&origin.types, &owner.types)
        || !Arc::ptr_eq(&origin.type_store, &owner.type_store)
        || !Arc::ptr_eq(&origin.effects, &owner.effects)
    {
        return Err("library method origin differs from its live caller revision".into());
    }
    if !library
        .types
        .methods
        .entries()
        .any(|entry| entry == imported)
    {
        return Err("library method declaration differs from its checked import".into());
    }
    let source = owner.types.methods.entries().find(|entry| {
        matches!(entry.target, MethodTarget::Source { callable, .. } if callable == item)
    }).ok_or("foreign method has no checked source declaration")?;
    Ok(CheckedForeignMethod {
        item,
        source,
        imported,
    })
}
