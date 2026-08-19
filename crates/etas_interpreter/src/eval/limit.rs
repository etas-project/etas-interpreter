use etas_core::{AnalysisDiagnosticCode, Span};
use etas_hir::{HirArg, HirExpr, HirExprId, HirLiteral, ResolveResult, SymbolDef};
use etas_host::{Budget, CostBudget, TimeBudget, TokenBudget};
use etas_std::{RequirementSemantics, StdDecl, StdLimitKind};

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
    pub(super) fn resolve_limit_expr(
        &self,
        expr: HirExprId,
    ) -> Result<RuntimeLimit, ExecutionFault> {
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
                "limit constructors must be called with exactly one static literal argument",
            ));
        }
        let kind = self.resolve_limit_kind(*callee, *span)?;
        let value_expr = match args.first().expect("len checked") {
            HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
        };
        let value = self.resolve_limit_value(kind, value_expr)?;
        Ok(RuntimeLimit {
            kind,
            value,
            span: *span,
        })
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
        let Some(symbol) = self.checked.symbols.get(symbol) else {
            return Err(invalid_limit(
                &span,
                "limit callee is missing from checked symbol facts",
            ));
        };
        let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
            return Err(invalid_limit(
                &span,
                "limit callee must resolve to a std requirement constructor",
            ));
        };
        let Some(std_symbol) = self.checked.std_registry.lookup_qualified(path) else {
            return Err(invalid_limit(
                &span,
                "limit callee is not present in the std registry",
            ));
        };
        let StdDecl::Requirement(requirement) = &std_symbol.decl else {
            return Err(invalid_limit(
                &span,
                "limit callee is not a std requirement constructor",
            ));
        };
        let RequirementSemantics::Limit(kind) = requirement.semantics else {
            return Err(invalid_limit(
                &span,
                "std requirement constructor is not a runtime limit",
            ));
        };
        Ok(kind)
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
        Ok(match kind {
            StdLimitKind::Iterations
            | StdLimitKind::Tokens
            | StdLimitKind::ContextTokens
            | StdLimitKind::Attempts => RuntimeLimitValue::Count(value),
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
                policy.options.max_output_tokens = Some(*value);
                set_token_budget(&mut policy.budget, *value);
            }
            (StdLimitKind::ContextTokens, RuntimeLimitValue::Count(value)) => {
                set_token_budget(&mut policy.budget, *value);
            }
            (StdLimitKind::WallTime, RuntimeLimitValue::DurationMillis(value)) => {
                let budget = policy.budget.get_or_insert_with(Budget::default);
                budget.time = Some(TimeBudget { max_millis: *value });
            }
            (StdLimitKind::Cost, RuntimeLimitValue::MoneyMicros { amount, currency }) => {
                let budget = policy.budget.get_or_insert_with(Budget::default);
                budget.cost = Some(CostBudget {
                    max_micros: *amount,
                    currency: currency.clone(),
                });
            }
            (StdLimitKind::Attempts | StdLimitKind::Iterations, RuntimeLimitValue::Count(_)) => {
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
    budget.tokens = Some(TokenBudget { max_tokens });
}
