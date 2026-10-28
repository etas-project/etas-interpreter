use super::{
    definition_operation_failure,
    project::{Projector, optional},
    report,
};
use crate::{
    control::ControlSignal,
    eval::EvalContext,
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::{CodeMetadataPayload, InterpValue as V},
};
use etas_core::Span;

impl EvalContext<'_> {
    pub(super) fn execute_code_diff(
        &self,
        call: &CheckedStdIntrinsicCall,
        args: Vec<V>,
        span: Span,
    ) -> ControlSignal {
        let [V::AgentDefinition(old), V::AgentDefinition(new)]: [V; 2] = (match args.try_into() {
            Ok(args) => args,
            Err(_) => {
                return ControlSignal::invalid_arguments(
                    "diff expects two AgentDef snapshots",
                    span,
                );
            }
        }) else {
            return ControlSignal::invalid_arguments("diff requires checked AgentDef values", span);
        };
        if call.parameter_types != [old.ty(), new.ty()]
            || old.ty() != new.ty()
            || !std::sync::Arc::ptr_eq(old.program(), self.checked)
            || !std::sync::Arc::ptr_eq(new.program(), self.checked)
        {
            return ControlSignal::missing_checked_fact(
                "diff argument ABI or program identity does not match the checked call",
                span,
            );
        }
        let views = old
            .backend()
            .resolve(old.definition())
            .and_then(|old_view| {
                new.backend()
                    .resolve(new.definition())
                    .map(|new_view| (old_view, new_view))
            });
        let (old_view, new_view) = match views {
            Ok(views) => views,
            Err(error) => return definition_operation_failure(error.into(), span),
        };
        let control = match self.code_syntax_control(span) {
            Ok(control) => control,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let changes = match old_view.diff(
            &new_view,
            old.backend().syntax_operations().as_ref(),
            new.backend().syntax_operations().as_ref(),
            self.syntax_limits.max_formatted_bytes,
            &control,
        ) {
            Ok(changes) => changes,
            Err(error) => {
                if let etas_frontend::code::definition::NativeDefinitionError::Operation(
                    etas_code::definition::DefinitionOperationError::Syntax(error),
                ) = error
                {
                    return self.controlled_syntax_failure(error, span);
                }
                return definition_operation_failure(error, span);
            }
        };
        match project(self, call.result_type, &changes) {
            Ok(value) => ControlSignal::Value(value),
            Err(reason) => ControlSignal::missing_checked_fact(reason, span),
        }
    }
}

fn project(
    context: &EvalContext<'_>,
    ty: etas_types::TypeId,
    value: &etas_code::definition::ChangeReport,
) -> Result<V, String> {
    let store = &context.checked.type_store;
    let known = &context.checked.types.known_std_types.code;
    let p = Projector { store, known };
    p.record(
        ty,
        vec![
            (
                "schema",
                V::String(etas_code::definition::CHANGE_REPORT_SCHEMA.into()),
            ),
            (
                "identity",
                p.metadata(
                    p.field(ty, "identity")?,
                    CodeMetadataPayload::Agent(value.identity.clone()),
                )?,
            ),
            (
                "old_revision",
                report::revision_info(
                    store,
                    known,
                    p.field(ty, "old_revision")?,
                    &value.old_revision,
                )?,
            ),
            (
                "new_revision",
                report::revision_info(
                    store,
                    known,
                    p.field(ty, "new_revision")?,
                    &value.new_revision,
                )?,
            ),
            ("old_body_available", V::Bool(value.old_body_available)),
            ("new_body_available", V::Bool(value.new_body_available)),
            ("body_changed", optional(value.body_changed.map(V::Bool))),
            ("signature_changed", V::Bool(value.signature_changed)),
            ("dependencies_changed", V::Bool(value.dependencies_changed)),
            (
                "declared_bound_changed",
                V::Bool(value.declared_bound_changed),
            ),
            (
                "escaping_changed",
                optional(value.escaping_changed.map(V::Bool)),
            ),
            (
                "requested_changed",
                optional(value.requested_changed.map(V::Bool)),
            ),
            (
                "default_handled_changed",
                optional(value.default_handled_changed.map(V::Bool)),
            ),
            (
                "determinism_changed",
                optional(value.determinism_changed.map(V::Bool)),
            ),
            ("trace_changed", optional(value.trace_changed.map(V::Bool))),
        ],
    )
}
