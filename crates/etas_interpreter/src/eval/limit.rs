use etas_core::{AnalysisDiagnosticCode, Span};
use etas_hir::{HirArg, HirExpr, HirExprId, HirLiteral, ResolveResult};
use etas_host::{Budget, CostBudget, TimeBudget, TokenBudget};
use etas_std::StdLimitKind;

use super::EvalContext;
use crate::control::ExecutionFault;

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

impl<'a> EvalContext<'a> {
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
                policy.options.max_output_tokens = Some(
                    policy
                        .options
                        .max_output_tokens
                        .map_or(*value, |current| current.min(*value)),
                );
                set_token_budget(&mut policy.budget, *value);
            }
            (StdLimitKind::ContextTokens, RuntimeLimitValue::Count(value)) => {
                set_token_budget(&mut policy.budget, *value);
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

pub(super) fn set_token_budget(budget: &mut Option<Budget>, max_tokens: u64) {
    let budget = budget.get_or_insert_with(Budget::default);
    budget.tokens = Some(TokenBudget {
        max_tokens: budget
            .tokens
            .as_ref()
            .map_or(max_tokens, |current| current.max_tokens.min(max_tokens)),
    });
}
