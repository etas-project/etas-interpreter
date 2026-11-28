use etas_core::{AnalysisDiagnosticCode, Span};
use etas_hir::{HirArg, HirExpr, HirExprId, HirLiteral, ResolveResult};
use etas_host::{Budget, CostBudget, TimeBudget, TokenBudget};
use etas_std::StdLimitKind;

use super::{EvalContext, compose_signal_continuation};
use crate::control::{Continuation, ControlSignal, ExecutionFault, Frame};
use crate::value::InterpValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLimit {
    pub kind: StdLimitKind,
    pub value: RuntimeLimitValue,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeLimitValue {
    Count(u64),
    DurationMillis(u64),
    MoneyMicros { amount: u128, currency: String },
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct IterationBudget {
    pub limit: usize,
    pub consumed: usize,
}

impl<'a> EvalContext<'a> {
    pub(super) fn execute_local_limits(
        &mut self,
        expr: HirExprId,
        values: Vec<InterpValue>,
        frame: &mut Frame,
    ) -> ControlSignal {
        let HirExpr::Limited {
            ref bounds,
            body,
            target,
            span,
        } = self.checked.hir.exprs[expr]
        else {
            unreachable!("checked local limit scope")
        };
        let mut limits = Vec::new();
        for (bound, value) in bounds.iter().zip(values) {
            let expected = self
                .checked
                .types
                .expr_types
                .get(bound)
                .copied()
                .and_then(|ty| {
                    let layout =
                        etas_types::applied_representation_in_store(&self.checked.type_store, ty)
                            .ok()
                            .flatten()
                            .unwrap_or(ty);
                    match self.checked.type_store.get(layout) {
                        Some(etas_types::Type::Array(element)) => Some(*element),
                        _ => None,
                    }
                });
            let Some(expected) = expected else {
                return ControlSignal::missing_checked_fact(
                    "local @limits has no checked array element type",
                    span,
                );
            };
            let InterpValue::Array(values) = value else {
                return ControlSignal::invalid_arguments(
                    "local @limits requires Array<Limit>",
                    span,
                );
            };
            for value in values.snapshot() {
                match self.runtime_limit_value(&value, expected, span) {
                    Ok(limit) => limits.push(limit),
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                }
            }
        }
        let mut policy = self.task.model_policy.clone();
        let mut iterations = None;
        let mut attempts = None;
        for limit in &limits {
            if let (StdLimitKind::Iterations, RuntimeLimitValue::Count(value)) =
                (&limit.kind, &limit.value)
            {
                let Ok(value) = usize::try_from(*value) else {
                    return ControlSignal::invalid_arguments("iteration limit exceeds usize", span);
                };
                iterations = Some(iterations.map_or(value, |current: usize| current.min(value)));
            } else if let (StdLimitKind::Attempts, RuntimeLimitValue::Count(value)) =
                (&limit.kind, &limit.value)
            {
                let Ok(value) = usize::try_from(*value) else {
                    return ControlSignal::invalid_arguments("attempt limit exceeds usize", span);
                };
                attempts = Some(attempts.map_or(value, |current: usize| current.min(value)));
            } else if let Err(fault) = self.apply_runtime_limit_to_model_policy(limit, &mut policy)
            {
                return ControlSignal::Fault(Box::new(fault));
            }
        }
        if attempts.is_some() && !matches!(target, etas_hir::HirLimitTarget::Retry(_)) {
            return ControlSignal::invalid_arguments("Attempts requires a retry limit scope", span);
        }
        let previous = std::mem::replace(&mut self.task.model_policy, policy);
        let signal = match target {
            etas_hir::HirLimitTarget::For(statement) => {
                let etas_hir::HirStmt::For {
                    pat,
                    iter,
                    body,
                    span,
                } = self.checked.hir.stmts[statement]
                else {
                    unreachable!("checked for limit target")
                };
                self.execute_for_loop(pat, iter, body, span, iterations, frame)
            }
            etas_hir::HirLimitTarget::While(statement) => {
                let etas_hir::HirStmt::While { cond, body, span } =
                    self.checked.hir.stmts[statement]
                else {
                    unreachable!("checked while limit target")
                };
                self.continue_while_loop(
                    cond,
                    body,
                    span,
                    iterations.map(|limit| IterationBudget { limit, consumed: 0 }),
                    frame,
                )
            }
            etas_hir::HirLimitTarget::Retry(statement) if iterations.is_none() => {
                let etas_hir::HirStmt::Retry {
                    body: retry_body,
                    span,
                } = self.checked.hir.stmts[statement]
                else {
                    unreachable!("checked retry limit target")
                };
                self.execute_retry_stmt(body, 0, retry_body, span, attempts, frame)
                    .unwrap_or(ControlSignal::Value(InterpValue::Unit))
            }
            _ if iterations.is_some() => ControlSignal::invalid_arguments(
                "Iterations requires a supported loop limit scope",
                span,
            ),
            _ => self.execute_block(body, frame),
        };
        match signal {
            signal @ (ControlSignal::Value(_)
            | ControlSignal::Return(_)
            | ControlSignal::Resume(_)
            | ControlSignal::Finish(_)
            | ControlSignal::Break
            | ControlSignal::Continue
            | ControlSignal::Fault(_)
            | ControlSignal::Cancelled(_)) => {
                self.task.model_policy = previous;
                signal
            }
            signal => compose_signal_continuation(
                signal,
                Continuation::RestoreModelPolicy {
                    previous: Box::new(previous),
                    inner: Continuation::BlockValue.into(),
                },
            ),
        }
    }

    pub(super) fn consume_loop_iteration(
        &mut self,
        budget: &mut Option<IterationBudget>,
        span: Span,
    ) -> Option<ControlSignal> {
        if let Some(budget) = budget {
            if budget.consumed >= budget.limit {
                return Some(self.raise_limit_exhausted(
                    "iterations",
                    budget.limit,
                    budget.consumed,
                    span,
                ));
            }
            budget.consumed += 1;
        }
        None
    }

    pub(super) fn raise_limit_exhausted(
        &mut self,
        resource: &str,
        limit: usize,
        consumed: usize,
        span: Span,
    ) -> ControlSignal {
        let Some(error_type) = self.checked.types.known_std_types.budget_exceeded else {
            return ControlSignal::missing_checked_fact(
                "local budget has no checked BudgetExceeded type",
                span,
            );
        };
        let value = match super::host_value::decode_checked_host_value(
            etas_host::HostValue::Record(vec![
                (
                    "resource".into(),
                    etas_host::HostValue::String(resource.into()),
                ),
                ("limit".into(), etas_host::HostValue::UInt(limit as u128)),
                (
                    "consumed".into(),
                    etas_host::HostValue::UInt(consumed as u128),
                ),
            ]),
            error_type,
            self.checked,
        ) {
            Ok(value) => value,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        ControlSignal::pending_perform(crate::control::PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: etas_hir::ResolvedActionRef {
                effect: etas_hir::HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], span),
                    args: Vec::new(),
                    span,
                },
                action: "raise".into(),
                action_symbol: ResolveResult::Unresolved,
                span,
            },
            error_type: Some(error_type),
            args: vec![value],
            span,
            continuation: Continuation::BlockValue,
            residual: None,
        })
    }

    fn runtime_limit_value(
        &self,
        value: &InterpValue,
        expected: etas_types::TypeId,
        span: Span,
    ) -> Result<RuntimeLimit, ExecutionFault> {
        let (name, fields) = if self.checked.hir.source_standard_types {
            self.plan
                .prepared_program()
                .enum_payload(value, expected)
                .map_err(|reason| {
                    ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, span, reason)
                })?
        } else {
            let InterpValue::Nominal { ty, value } = value else {
                return Err(invalid_limit(&span, "limit has no checked owner identity"));
            };
            if *ty != expected {
                return Err(invalid_limit(
                    &span,
                    "limit has a different checked owner identity",
                ));
            }
            let InterpValue::Variant { name, fields } = value.as_ref() else {
                return Err(invalid_limit(
                    &span,
                    "limit has no checked constructor payload",
                ));
            };
            (name.as_str(), fields.as_ref())
        };
        let [value] = fields else {
            return Err(invalid_limit(&span, "limit requires one payload value"));
        };
        let count = || {
            value
                .as_number()
                .and_then(|value| value.as_u64())
                .ok_or_else(|| invalid_limit(&span, "limit count must be a non-negative integer"))
        };
        let (kind, value) = match name {
            "Iterations" => (StdLimitKind::Iterations, RuntimeLimitValue::Count(count()?)),
            "Tokens" => (StdLimitKind::Tokens, RuntimeLimitValue::Count(count()?)),
            "ContextTokens" => (
                StdLimitKind::ContextTokens,
                RuntimeLimitValue::Count(count()?),
            ),
            "Attempts" => (StdLimitKind::Attempts, RuntimeLimitValue::Count(count()?)),
            "Concurrency" => (
                StdLimitKind::Concurrency,
                RuntimeLimitValue::Count(count()?),
            ),
            "WallTime" => {
                let millis = limit_record_field(value, "millis")
                    .and_then(|value| value.as_number())
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| {
                        invalid_limit(
                            &span,
                            "WallTime requires a Duration within the runtime clock range",
                        )
                    })?;
                (
                    StdLimitKind::WallTime,
                    RuntimeLimitValue::DurationMillis(millis),
                )
            }
            "Cost" => {
                let amount = limit_record_field(value, "amount")
                    .and_then(|value| value.as_number())
                    .and_then(|value| value.as_f64())
                    .ok_or_else(|| invalid_limit(&span, "Cost requires Money"))?
                    * 1_000_000.0;
                if !amount.is_finite() || amount < 0.0 || amount >= u128::MAX as f64 {
                    return Err(invalid_limit(
                        &span,
                        "Cost amount is outside the supported money range",
                    ));
                }
                let Some(InterpValue::String(currency)) = limit_record_field(value, "currency")
                else {
                    return Err(invalid_limit(&span, "Cost requires a currency"));
                };
                (
                    StdLimitKind::Cost,
                    RuntimeLimitValue::MoneyMicros {
                        amount: amount.round() as u128,
                        currency: currency.to_string(),
                    },
                )
            }
            _ => {
                return Err(invalid_limit(
                    &span,
                    "limit has no checked runtime contract",
                ));
            }
        };
        Ok(RuntimeLimit { kind, value, span })
    }
    pub(in crate::eval) fn declaration_model_policy(
        &mut self,
        item: etas_hir::HirItemId,
    ) -> Result<Option<crate::api::ModelExecutionPolicy>, ExecutionFault> {
        let annotations = self
            .checked
            .hir
            .item_annotations
            .get(&item)
            .into_iter()
            .flatten()
            .filter(|annotation| {
                matches!(annotation.path.segments.as_slice(), [segment] if segment.name == "limits")
            })
            .cloned()
            .collect::<Vec<_>>();
        if annotations.is_empty() {
            return Ok(None);
        }
        let mut policy = self.task.model_policy.clone();
        for annotation in annotations {
            self.apply_model_limits_annotation(&annotation, &mut policy)?;
        }
        Ok(Some(policy))
    }

    pub(super) fn resolve_limit_expr(
        &self,
        expr: HirExprId,
    ) -> Result<RuntimeLimit, ExecutionFault> {
        let (kind, value_expr, span) = self.limit_call(expr)?;
        let value = self.resolve_limit_value(kind, value_expr)?;
        Ok(RuntimeLimit { kind, value, span })
    }

    fn limit_call(
        &self,
        expr: HirExprId,
    ) -> Result<(StdLimitKind, HirExprId, Span), ExecutionFault> {
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        let HirExpr::Call {
            callee, args, span, ..
        } = &self.checked.hir.exprs[expr]
        else {
            return Err(invalid_limit(
                &span,
                "limit expression must call a std requirement constructor",
            ));
        };
        if args.len() != 1 {
            return Err(invalid_limit(
                span,
                "limit constructors must be called with exactly one argument",
            ));
        }
        let kind = self.resolve_limit_kind(*callee, *span)?;
        let value_expr = match args.first().expect("len checked") {
            HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
        };
        Ok((kind, value_expr, *span))
    }

    fn resolve_limit_kind(
        &self,
        callee: HirExprId,
        span: Span,
    ) -> Result<StdLimitKind, ExecutionFault> {
        let HirExpr::Path(path) = &self.checked.hir.exprs[callee] else {
            return Err(invalid_limit(
                &span,
                "limit callee must be a resolved std requirement constructor",
            ));
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return Err(invalid_limit(
                &span,
                "limit callee must be resolved before interpretation",
            ));
        };
        etas_types::facts::enums::standard_limit_constructor(
            &self.checked.hir,
            &self.checked.std_registry,
            symbol,
        )
        .ok_or_else(|| invalid_limit(&span, "limit callee has no checked runtime budget contract"))
    }

    fn resolve_limit_value(
        &self,
        kind: StdLimitKind,
        expr: HirExprId,
    ) -> Result<RuntimeLimitValue, ExecutionFault> {
        let span = self.checked.hir.exprs[expr].span(&self.checked.hir.blocks);
        let value = match &self.checked.hir.exprs[expr] {
            HirExpr::Literal(HirLiteral::Int { text, .. }) => {
                text.replace('_', "").parse::<u64>().map_err(|_| {
                    invalid_limit(&span, "limit literal must be a non-negative integer")
                })?
            }
            _ => {
                return Err(invalid_limit(
                    &span,
                    "limit argument must be a static integer literal",
                ));
            }
        };
        if kind == StdLimitKind::Concurrency
            && (value == 0 || value > crate::driver::scheduler::MAX_CONCURRENCY as u64)
        {
            return Err(invalid_limit(
                &span,
                "Concurrency must be positive and within the runtime implementation ceiling",
            ));
        }
        Ok(match kind {
            StdLimitKind::Iterations
            | StdLimitKind::Tokens
            | StdLimitKind::ContextTokens
            | StdLimitKind::Attempts
            | StdLimitKind::Concurrency => RuntimeLimitValue::Count(value),
            StdLimitKind::WallTime => RuntimeLimitValue::DurationMillis(value),
            StdLimitKind::Cost => RuntimeLimitValue::MoneyMicros {
                amount: u128::from(value),
                currency: "USD".to_owned(),
            },
        })
    }

    pub(super) fn apply_runtime_limit_to_model_policy(
        &self,
        limit: &RuntimeLimit,
        policy: &mut crate::api::ModelExecutionPolicy,
    ) -> Result<(), ExecutionFault> {
        match (&limit.kind, &limit.value) {
            (StdLimitKind::Tokens, RuntimeLimitValue::Count(value)) => {
                set_token_budget(&mut policy.budget, *value);
            }
            (StdLimitKind::ContextTokens, RuntimeLimitValue::Count(value)) => {
                policy.options.max_context_tokens = Some(
                    policy
                        .options
                        .max_context_tokens
                        .map_or(*value, |current| current.min(*value)),
                );
            }
            (StdLimitKind::WallTime, RuntimeLimitValue::DurationMillis(value)) => {
                let budget = policy.budget.get_or_insert_with(Budget::default);
                budget.time = Some(TimeBudget {
                    max_millis: budget
                        .time
                        .as_ref()
                        .map_or(*value, |current| current.max_millis.min(*value)),
                });
            }
            (StdLimitKind::Cost, RuntimeLimitValue::MoneyMicros { amount, currency }) => {
                let budget = policy.budget.get_or_insert_with(Budget::default);
                if budget
                    .cost
                    .as_ref()
                    .is_some_and(|current| current.currency != *currency)
                {
                    return Err(invalid_limit(
                        &limit.span,
                        "cost budget currency conflicts with the enclosing scope",
                    ));
                }
                budget.cost = Some(CostBudget {
                    max_micros: budget
                        .cost
                        .as_ref()
                        .map_or(*amount, |current| current.max_micros.min(*amount)),
                    currency: currency.clone(),
                });
            }
            (
                StdLimitKind::Attempts | StdLimitKind::Iterations | StdLimitKind::Concurrency,
                RuntimeLimitValue::Count(_),
            ) => {
                return Err(ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    limit.span,
                    "this runtime limit cannot be applied to a model stage budget",
                ));
            }
            _ => unreachable!("runtime limit resolver returns value shape matching limit kind"),
        }
        Ok(())
    }
}

fn invalid_limit(span: &Span, message: &'static str) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::InvalidArguments, *span, message)
}

fn limit_record_field(value: &InterpValue, field: &str) -> Option<InterpValue> {
    let value = match value {
        InterpValue::Nominal { value, .. } => value.as_ref(),
        value => value,
    };
    match value {
        InterpValue::Record(record) => record.get(field),
        _ => None,
    }
}

pub(super) fn set_token_budget(budget: &mut Option<Budget>, max_tokens: u64) {
    let budget = budget.get_or_insert_with(Budget::default);
    budget.tokens = Some(TokenBudget {
        max_tokens: budget
            .tokens
            .as_ref()
            .map_or(max_tokens, |current| current.max_tokens.min(max_tokens)),
    });
}
