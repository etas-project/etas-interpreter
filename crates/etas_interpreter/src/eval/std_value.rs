use super::*;
use crate::control::ExecutionFault;
use etas_std::RuntimeTypeRole;
use etas_types::TypeId;

impl EvalContext<'_> {
    pub(super) fn checked_std_result(
        &self,
        expected: TypeId,
        role: RuntimeTypeRole,
        payload: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        if self.runtime_type(role, span)? != expected {
            return Err(ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "std constructor result differs from its checked type identity",
            ));
        }
        self.checked_std_value(role, payload, span)
    }
    pub(super) fn runtime_type(
        &self,
        role: RuntimeTypeRole,
        span: Span,
    ) -> Result<TypeId, ExecutionFault> {
        self.checked
            .types
            .known_std_types
            .runtime_type(role)
            .ok_or_else(|| {
                ExecutionFault::new(
                    AnalysisDiagnosticCode::MissingCheckedFact,
                    span,
                    format!("missing checked std {role:?} identity"),
                )
            })
    }

    pub(super) fn checked_std_value(
        &self,
        role: RuntimeTypeRole,
        payload: InterpValue,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        let ty = self.runtime_type(role, span)?;
        let value = InterpValue::Nominal {
            ty,
            value: payload.into(),
        };
        crate::value::contract::runtime::validate_value(
            self.checked,
            self.plan,
            &value,
            ty,
            &self.storage_limits,
        )
        .map_err(|reason| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                format!("std {role:?} constructor violates its checked result ABI: {reason}"),
            )
        })?;
        Ok(value)
    }

    pub(super) fn session_id_value(
        &self,
        id: String,
        span: Span,
    ) -> Result<InterpValue, ExecutionFault> {
        self.checked_std_value(
            RuntimeTypeRole::SessionId,
            InterpValue::String(id.into()),
            span,
        )
    }
}
