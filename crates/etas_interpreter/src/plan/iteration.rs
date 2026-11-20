use etas_frontend::CheckedProject;
use etas_hir::HirExprId;
use etas_types::{Type, TypeId, source_iteration, ty::TypeLookupIndex};

use crate::{control::CallTarget, value::InterpValue};

/// The source declaration fixes only the one-step value protocol.
#[derive(Clone, Debug)]
pub(crate) struct SourceIteratorProtocol {
    pub constructor: TypeId,
    pub value_type: TypeId,
    pub step_type: TypeId,
    pub native_option: bool,
}

impl SourceIteratorProtocol {
    pub(crate) fn checked(
        project: &CheckedProject,
        expr: HirExprId,
        lookup: Option<&TypeLookupIndex>,
    ) -> Result<Option<Self>, String> {
        let Some(value_type) = project.types.expression_producer_type(expr) else {
            return Ok(None);
        };
        let store = &project.type_store;
        let facts = &project.types;
        let layout = match lookup {
            Some(lookup) => {
                source_iteration::checked_cursor_indexed(store, facts, lookup, value_type)
            }
            None => source_iteration::checked_cursor(store, facts, value_type),
        };
        let Some(layout) = layout.map_err(|error| error.to_string())? else {
            return Ok(None);
        };
        Ok(Some(Self {
            constructor: layout.constructor,
            value_type,
            step_type: layout.step,
            native_option: layout.native_option,
        }))
    }

    pub(crate) fn next(
        &self,
        project: &CheckedProject,
        value: &InterpValue,
    ) -> Result<CallTarget, String> {
        let InterpValue::Nominal { ty, value } = value else {
            return Err("source iterator value is not its checked nominal".into());
        };
        let owner = match project.type_store.get(*ty) {
            Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
            _ => *ty,
        };
        if owner != self.constructor {
            return Err("source iterator has a different checked owner".into());
        }
        let InterpValue::Record(record) = &**value else {
            return Err("source iterator has no record storage".into());
        };
        let next = record.borrow_checked_field(0, "next", 1)?;
        let InterpValue::Callable(next) = next else {
            return Err("source iterator next field is not a callable value".into());
        };
        Ok(next.clone())
    }
}
