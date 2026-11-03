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
        let [old, new]: [V; 2] = match args.try_into() {
            Ok(args) => args,
            Err(_) => {
                return ControlSignal::invalid_arguments(
                    "diff expects two definition snapshots",
                    span,
                );
            }
        };
        let [old_type, new_type] = call.parameter_types.as_slice() else {
            return ControlSignal::missing_checked_fact(
                "diff has no checked two-argument signature",
                span,
            );
        };
        if old_type != new_type {
            return ControlSignal::missing_checked_fact(
                "diff descriptors do not share the checked target type",
                span,
            );
        }
        let old = match Snapshot::checked(self, &old, *old_type) {
            Ok(snapshot) => snapshot,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let new = match Snapshot::checked(self, &new, *new_type) {
            Ok(snapshot) => snapshot,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        if old.definition.identity() != new.definition.identity() {
            return ControlSignal::invalid_arguments(
                "diff requires snapshots of the same definition identity",
                span,
            );
        }
        let views = old.backend.resolve(old.definition).and_then(|old_view| {
            new.backend
                .resolve(new.definition)
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
            old.backend.syntax_operations().as_ref(),
            new.backend.syntax_operations().as_ref(),
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

struct Snapshot<'a> {
    definition: &'a etas_code::definition::DefinitionRef,
    backend: &'a etas_frontend::code::definition::NativeDefinitionStore,
}

impl<'a> Snapshot<'a> {
    fn checked(
        context: &EvalContext<'_>,
        value: &'a V,
        expected: etas_types::TypeId,
    ) -> Result<Self, String> {
        let known = &context.checked.types.known_std_types.code;
        let (ty, program, definition, backend, constructor, kind) = match value {
            V::AgentDefinition(value) => (
                value.ty(),
                value.program(),
                value.definition(),
                value.backend(),
                known.agent_definition,
                etas_code::DefinitionKind::Agent,
            ),
            V::CallableDefinition(value) => (
                value.ty(),
                value.program(),
                value.definition()?,
                value.backend(),
                known.callable_definition,
                etas_code::DefinitionKind::Flow,
            ),
            _ => {
                return Err("diff requires checked AgentDef<A> or CallableDef<F> snapshots".into());
            }
        };
        if ty != expected
            || !std::sync::Arc::ptr_eq(program, context.checked)
            || definition.identity().kind() != kind
            || !matches!(context.checked.type_store.get(expected), Some(etas_types::Type::Applied { constructor: actual, args }) if constructor == Some(etas_types::TypeId(actual.0)) && args.len() == 1)
        {
            return Err("diff argument ABI, definition family or program identity does not match the checked call".into());
        }
        Ok(Self {
            definition,
            backend,
        })
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
                    CodeMetadataPayload::Definition(value.identity.clone()),
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
