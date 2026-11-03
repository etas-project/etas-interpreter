use super::*;

impl EvalContext<'_> {
    pub(super) fn specialize_spec_method_target(
        &self,
        call: HirExprId,
        target: CallTarget,
        spec: SymbolId,
        method: &str,
        frame: &Frame,
        span: Span,
    ) -> Result<CallTarget, ExecutionFault> {
        let missing = |message: String| {
            ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
        };
        let spec = self
            .checked
            .hir
            .canonical_source_symbol(spec)
            .ok_or_else(|| missing("spec method has no canonical spec identity".into()))?;
        let method = self
            .checked
            .types
            .spec_signatures
            .get(&spec)
            .and_then(|spec| {
                spec.methods
                    .iter()
                    .find(|candidate| candidate.name == method)
            })
            .ok_or_else(|| missing("spec method has no checked declaration".into()))?;
        let signature = method
            .signature_in(&self.checked.types)
            .ok_or_else(|| missing("spec method has no checked callable signature".into()))?;
        let CallTarget::FlowItem(item) = &target else {
            return Err(missing(
                "spec method target is not a normalized flow item".into(),
            ));
        };
        let Some(etas_types::ItemSignature::Flow(actual)) =
            self.checked.types.item_signatures.get(item)
        else {
            return Err(missing(
                "spec implementation has no checked callable signature".into(),
            ));
        };
        let parameters = method
            .implementation_generic_parameters(signature, actual)
            .map_err(missing)?;
        let bindings = crate::control::CheckedCallBindings::resolve(
            self.checked,
            call,
            frame.generic_bindings(),
        )
        .map_err(|error| {
            missing(format!(
                "spec method bindings cannot be materialized: {error}"
            ))
        })?;
        let mut types = Vec::new();
        let mut rows = Vec::new();
        for (required, actual) in parameters {
            match required.kind {
                etas_types::CallableGenericParamKind::Type => {
                    let binding = bindings
                        .types
                        .iter()
                        .find(|(name, _)| name == &required.name)
                        .ok_or_else(|| {
                            missing("spec method is missing a checked type argument".into())
                        })?;
                    types.push((actual.name.clone(), binding.1));
                }
                etas_types::CallableGenericParamKind::Effect => {
                    let binding = bindings
                        .rows
                        .iter()
                        .find(|(name, _)| name == &required.name)
                        .ok_or_else(|| {
                            missing("spec method is missing a checked effect argument".into())
                        })?;
                    rows.push((actual.name.clone(), binding.1.clone()));
                }
            }
        }
        if types.len() != bindings.types.len() || rows.len() != bindings.rows.len() {
            return Err(missing(
                "spec method bindings do not match its checked declaration".into(),
            ));
        }
        if types.is_empty() && rows.is_empty() {
            return Ok(target);
        }
        Ok(CallTarget::Specialized {
            target: target.into(),
            type_bindings: types,
            effect_row_bindings: rows,
        })
    }
}
