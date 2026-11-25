use std::collections::BTreeSet;

use etas_code::{
    CallableId,
    descriptor::{
        CallableOutcomeInfo, ParameterCallInfo, ParameterHandlerCompletionInfo,
        ParameterHandlerScopeInfo, TraceConstraintInfo, TraceInfo, TraceInfoRef, TracePatternInfo,
        TraceRuleInfo,
    },
};
use etas_types::TypeId;

use super::{Description, observations};
use crate::{
    eval::code::project::optional,
    value::{CodeMetadataPayload as M, InterpValue as V},
};

impl Description<'_, '_, '_> {
    pub(in crate::eval::code) fn trace_summary(
        &mut self,
        ty: TypeId,
        root: TraceInfoRef,
    ) -> Result<V, String> {
        let p = self.p;
        let expression = self.trace(p.field(ty, "expression")?, root)?;
        let gaps = self.trace_gaps(root)?;
        let element = p.array_element(p.field(ty, "unresolved")?)?;
        let unresolved = gaps
            .iter()
            .map(|(callable, position, reason)| {
                p.record(
                    element,
                    vec![
                        (
                            "callable",
                            p.optional_metadata(
                                p.field(element, "callable")?,
                                callable.clone().map(M::Callable),
                            )?,
                        ),
                        (
                            "position",
                            optional(position.map(|position| V::usize(position as usize))),
                        ),
                        ("location", optional(None)),
                        ("reason", V::String((*reason).into())),
                    ],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        p.record(
            ty,
            vec![
                ("expression", expression),
                ("complete", V::Bool(gaps.is_empty())),
                ("unresolved", V::Array(unresolved.into())),
            ],
        )
    }

    fn trace(&mut self, ty: TypeId, root: TraceInfoRef) -> Result<V, String> {
        let p = self.p;
        let node = self
            .graph
            .trace(root)
            .ok_or("trace description is missing")?
            .clone();
        let (variant, values) = match node {
            TraceInfo::Empty => ("Empty", Vec::new()),
            TraceInfo::Recursive { id, body } => {
                let body_ty = p.variant_field(ty, "Recursive", "body")?;
                let body = body
                    .map(|body| self.trace(p.option(body_ty)?, body))
                    .transpose()?;
                (
                    "Recursive",
                    vec![
                        V::String(id.to_string().into()),
                        p.optional_value(body_ty, body)?,
                    ],
                )
            }
            TraceInfo::Event(effect) => (
                "Action",
                vec![self.effect(p.variant_payload(ty, "Action")?, &effect)?],
            ),
            TraceInfo::Outcome(value) => (
                "Outcome",
                vec![self.outcome(p.variant_payload(ty, "Outcome")?, &value)?],
            ),
            TraceInfo::ParameterCall(call) => (
                "ParameterCall",
                vec![self.parameter_call(p.variant_payload(ty, "ParameterCall")?, &call)?],
            ),
            TraceInfo::Seq(parts) => (
                "Sequence",
                vec![self.trace_array(p.variant_payload(ty, "Sequence")?, &parts)?],
            ),
            TraceInfo::Choice(parts) => (
                "Choice",
                vec![self.trace_array(p.variant_payload(ty, "Choice")?, &parts)?],
            ),
            TraceInfo::Repeat(inner) => (
                "Repeat",
                vec![self.trace(p.variant_payload(ty, "Repeat")?, inner)?],
            ),
            TraceInfo::UnknownOrder(row) => (
                "UnknownOrder",
                vec![self.row(p.variant_payload(ty, "UnknownOrder")?, row, false)?],
            ),
            TraceInfo::Widened {
                actions,
                parameter_calls,
            } => {
                let actions =
                    self.row(p.variant_field(ty, "Widened", "actions")?, actions, false)?;
                let element =
                    p.array_element(p.variant_field(ty, "Widened", "parameter_calls")?)?;
                let calls = parameter_calls
                    .iter()
                    .map(|call| self.parameter_call(element, call))
                    .collect::<Result<Vec<_>, _>>()?;
                return p.named_variant(
                    ty,
                    "Widened",
                    vec![
                        ("actions", actions),
                        ("parameter_calls", V::Array(calls.into())),
                    ],
                );
            }
        };
        p.variant(ty, variant, values)
    }

    fn trace_array(&mut self, ty: TypeId, roots: &[TraceInfoRef]) -> Result<V, String> {
        let element = self.p.array_element(ty)?;
        roots
            .iter()
            .map(|root| self.trace(element, *root))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| V::Array(values.into()))
    }

    fn parameter_call(&mut self, ty: TypeId, call: &ParameterCallInfo) -> Result<V, String> {
        let p = self.p;
        let signature = self.type_info(p.field(ty, "signature")?, call.signature)?;
        let arguments =
            observations::arguments(p, p.field(ty, "arguments")?, call.arguments.as_deref())?;
        let handlers_ty = p.option(p.field(ty, "handlers")?)?;
        let handlers = call
            .handlers
            .as_ref()
            .map(|handlers| {
                let element = p.array_element(handlers_ty)?;
                handlers
                    .iter()
                    .map(|handler| self.handler_scope(element, handler))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| V::Array(values.into()))
            })
            .transpose()?;
        p.record(
            ty,
            vec![
                (
                    "callable",
                    p.metadata(p.field(ty, "callable")?, M::Callable(call.callable.clone()))?,
                ),
                ("signature", signature),
                ("position", V::usize(call.position as usize)),
                (
                    "projection",
                    super::value_projection::path(p, p.field(ty, "projection")?, &call.projection)?,
                ),
                ("arguments", arguments),
                ("handlers", optional(handlers)),
            ],
        )
    }

    fn handler_scope(
        &mut self,
        ty: TypeId,
        scope: &ParameterHandlerScopeInfo,
    ) -> Result<V, String> {
        let p = self.p;
        let ParameterHandlerScopeInfo::Handle { arms } = scope;
        let element = p.array_element(p.variant_payload(ty, "Handle")?)?;
        let mut values = Vec::with_capacity(arms.len());
        for arm in arms {
            let action = self.effect(p.field(element, "action")?, &arm.action)?;
            let trace = self.trace(p.field(element, "trace")?, arm.trace)?;
            let completion_ty = p.field(element, "completion")?;
            let (variant, result, value) = match &arm.completion {
                ParameterHandlerCompletionInfo::Resume { result_type, value } => {
                    ("Resume", *result_type, value)
                }
                ParameterHandlerCompletionInfo::Finish { result_type, value } => {
                    ("Finish", *result_type, value)
                }
            };
            let result = self.type_info(
                p.variant_field(completion_ty, variant, "result_type")?,
                result,
            )?;
            let observation = observations::observation(
                p,
                p.variant_field(completion_ty, variant, "value")?,
                value,
            )?;
            let completion = p.named_variant(
                completion_ty,
                variant,
                vec![("result_type", result), ("value", observation)],
            )?;
            values.push(p.record(
                element,
                vec![
                    ("action", action),
                    ("trace", trace),
                    ("completion", completion),
                ],
            )?);
        }
        p.variant(ty, "Handle", vec![V::Array(values.into())])
    }

    fn outcome(&mut self, ty: TypeId, outcome: &CallableOutcomeInfo) -> Result<V, String> {
        let p = self.p;
        match outcome {
            CallableOutcomeInfo::Returned {
                ty: root,
                observation,
            } => {
                let value = self.type_info(p.variant_field(ty, "Returned", "ty")?, *root)?;
                let observation = observations::observation(
                    p,
                    p.variant_field(ty, "Returned", "observation")?,
                    observation,
                )?;
                p.named_variant(
                    ty,
                    "Returned",
                    vec![("ty", value), ("observation", observation)],
                )
            }
            CallableOutcomeInfo::Raised { error } => {
                let error = self.type_info(p.variant_field(ty, "Raised", "error")?, *error)?;
                p.named_variant(ty, "Raised", vec![("error", error)])
            }
            CallableOutcomeInfo::NonReturn => p.variant(ty, "NonReturn", Vec::new()),
            CallableOutcomeInfo::ParameterNonReturn(call)
            | CallableOutcomeInfo::ParameterNonReturnTemplate(call) => {
                let variant = if matches!(outcome, CallableOutcomeInfo::ParameterNonReturn(_)) {
                    "ParameterNonReturn"
                } else {
                    "ParameterNonReturnTemplate"
                };
                let call = self.parameter_call(p.variant_payload(ty, variant)?, call)?;
                p.variant(ty, variant, vec![call])
            }
        }
    }

    pub(in crate::eval::code) fn constraint(
        &mut self,
        ty: TypeId,
        constraint: &TraceConstraintInfo,
    ) -> Result<V, String> {
        let p = self.p;
        let alternative_ty = p.array_element(p.field(ty, "alternatives")?)?;
        let rule_ty = p.array_element(alternative_ty)?;
        let alternatives = constraint
            .alternatives
            .iter()
            .map(|rules| {
                rules
                    .iter()
                    .map(|rule| self.rule(rule_ty, rule))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| V::Array(values.into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        p.record(ty, vec![("alternatives", V::Array(alternatives.into()))])
    }

    fn rule(&mut self, ty: TypeId, rule: &TraceRuleInfo) -> Result<V, String> {
        let p = self.p;
        match rule {
            TraceRuleInfo::Allow(pattern) | TraceRuleInfo::Deny(pattern) => {
                let variant = if matches!(rule, TraceRuleInfo::Allow(_)) {
                    "Allow"
                } else {
                    "Deny"
                };
                let pattern = self.pattern(p.variant_payload(ty, variant)?, pattern)?;
                p.variant(ty, variant, vec![pattern])
            }
            TraceRuleInfo::RequireBefore { guard, target } => {
                let guard = self.pattern(p.variant_field(ty, "RequireBefore", "guard")?, guard)?;
                let target =
                    self.pattern(p.variant_field(ty, "RequireBefore", "target")?, target)?;
                p.named_variant(
                    ty,
                    "RequireBefore",
                    vec![("guard", guard), ("target", target)],
                )
            }
            TraceRuleInfo::RequireAfter { target, obligation } => {
                let target =
                    self.pattern(p.variant_field(ty, "RequireAfter", "target")?, target)?;
                let obligation = self.pattern(
                    p.variant_field(ty, "RequireAfter", "obligation")?,
                    obligation,
                )?;
                p.named_variant(
                    ty,
                    "RequireAfter",
                    vec![("target", target), ("obligation", obligation)],
                )
            }
            TraceRuleInfo::Limit { count } => {
                p.named_variant(ty, "Limit", vec![("count", V::usize(*count))])
            }
        }
    }

    fn pattern(&mut self, ty: TypeId, pattern: &TracePatternInfo) -> Result<V, String> {
        let p = self.p;
        let row = self.row(p.field(ty, "row")?, pattern.row, false)?;
        let element = p.array_element(p.field(ty, "bounds")?)?;
        let mut bounds = Vec::with_capacity(pattern.bounds.len());
        for bound in &pattern.bounds {
            let target = self.target(p.field(element, "target")?, &bound.target)?;
            let spec = self.spec(p.field(element, "spec")?, &bound.spec)?;
            bounds.push(p.record(
                element,
                vec![
                    ("target", target),
                    ("argument", V::usize(bound.argument as usize)),
                    ("spec", spec),
                ],
            )?);
        }
        p.record(ty, vec![("row", row), ("bounds", V::Array(bounds.into()))])
    }

    fn trace_gaps(
        &self,
        root: TraceInfoRef,
    ) -> Result<BTreeSet<(Option<CallableId>, Option<u32>, &'static str)>, String> {
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        let mut gaps = BTreeSet::new();
        while let Some(root) = pending.pop() {
            if !seen.insert(root) {
                continue;
            }
            let mut calls = Vec::new();
            match self
                .graph
                .trace(root)
                .ok_or("trace description is missing")?
            {
                TraceInfo::Empty | TraceInfo::Event(_) => {}
                TraceInfo::Seq(parts) | TraceInfo::Choice(parts) => {
                    pending.extend(parts.iter().copied())
                }
                TraceInfo::Recursive { body, .. } => pending.extend(*body),
                TraceInfo::Repeat(inner) => pending.push(*inner),
                TraceInfo::UnknownOrder(_) => {
                    gaps.insert((None, None, "action order is unavailable"));
                }
                TraceInfo::Widened {
                    parameter_calls, ..
                } => {
                    gaps.insert((None, None, "trace was widened"));
                    calls.extend(parameter_calls);
                }
                TraceInfo::ParameterCall(call) => calls.push(call),
                TraceInfo::Outcome(
                    CallableOutcomeInfo::ParameterNonReturn(call)
                    | CallableOutcomeInfo::ParameterNonReturnTemplate(call),
                ) => calls.push(call),
                TraceInfo::Outcome(_) => {}
            }
            for call in calls {
                gaps.insert((
                    Some(call.callable.clone()),
                    Some(call.position),
                    "callback trace depends on the supplied argument",
                ));
                for scope in call.handlers.iter().flatten() {
                    let ParameterHandlerScopeInfo::Handle { arms } = scope;
                    pending.extend(arms.iter().map(|arm| arm.trace));
                }
            }
        }
        Ok(gaps)
    }
}
