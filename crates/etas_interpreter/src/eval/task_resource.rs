use std::collections::{HashMap, HashSet};

use etas_frontend::{ResourceGuardCallablePlan, ResourceValueSelector};
use etas_hir::{HirExpr, HirExprId, HirItemId, ResolveResult, SymbolDef, SymbolId};

use super::EvalContext;
use crate::control::{CallTarget, Frame};
use crate::driver::scheduler::{EffectIdentity, FrozenSelector, RuntimeResourceKey};
use crate::value::InterpValue;

impl EvalContext<'_> {
    pub(super) fn task_resource_keys(
        &mut self,
        target: &CallTarget,
        args: &[InterpValue],
    ) -> Option<Vec<RuntimeResourceKey>> {
        let (plan, captured, item) = match target {
            CallTarget::FlowItem(item) => (
                self.checked
                    .concurrency
                    .resource_callables
                    .get(item)?
                    .clone(),
                None,
                Some(*item),
            ),
            CallTarget::Lambda { expr, captured } => (
                self.checked
                    .concurrency
                    .resource_closures
                    .get(expr)?
                    .clone(),
                Some(captured),
                None,
            ),
            _ => return None,
        };
        let mut visiting = HashSet::new();
        if let Some(item) = item {
            visiting.insert(item);
        }
        self.resolve_task_resources(&plan, args, captured, &mut visiting)
    }

    fn resolve_task_resources(
        &mut self,
        plan: &ResourceGuardCallablePlan,
        args: &[InterpValue],
        captured: Option<&Frame>,
        visiting: &mut HashSet<HirItemId>,
    ) -> Option<Vec<RuntimeResourceKey>> {
        if !plan.complete || plan.parameters.len() != args.len() {
            return None;
        }
        let locals = plan
            .parameters
            .iter()
            .copied()
            .zip(args.iter().cloned())
            .collect::<HashMap<_, _>>();
        let mut keys = Vec::new();
        for selector in &plan.resources {
            let store = match &selector.store {
                ResourceValueSelector::Expr(expr) => {
                    self.resource_expr(*expr, &locals, captured)?
                }
                ResourceValueSelector::SymbolPath { symbol, fields } => {
                    let value = self.resource_symbol(*symbol, &locals, captured)?;
                    self.resource_fields(value, *symbol, fields)?
                }
            };
            let InterpValue::MemoryStore {
                region_stable_id,
                path,
                ..
            } = store
            else {
                return None;
            };
            let key = if let Some(expr) = selector.key {
                let value = self.resource_expr(expr, &locals, captured)?;
                let host = super::interp_to_host_value(&value).ok()?;
                // Primitive selectors cannot be mutated through a shared data handle.
                if !matches!(
                    host,
                    etas_host::HostValue::String(_)
                        | etas_host::HostValue::Int(_)
                        | etas_host::HostValue::UInt(_)
                        | etas_host::HostValue::Bool(_)
                ) {
                    return None;
                }
                self.storage_limits.value_size(&host).ok()?;
                Some(etas_host::host_value_to_json_string(&host).ok()?)
            } else {
                None
            };
            keys.push(RuntimeResourceKey {
                effect: EffectIdentity::Memory,
                selector: FrozenSelector {
                    region: region_stable_id,
                    path,
                    key,
                },
            });
        }
        for call in &plan.calls {
            if self
                .checked
                .cpu_parallel
                .callables
                .get(&call.callable)
                .is_some_and(|fact| fact.eligible)
            {
                continue;
            }
            if !visiting.insert(call.callable) {
                return None;
            }
            let arguments = call
                .arguments
                .iter()
                .map(|expr| self.resource_expr(*expr, &locals, captured))
                .collect::<Option<Vec<_>>>()?;
            let child = self
                .checked
                .concurrency
                .resource_callables
                .get(&call.callable)?
                .clone();
            keys.extend(self.resolve_task_resources(&child, &arguments, None, visiting)?);
            visiting.remove(&call.callable);
        }
        Some(keys)
    }

    fn resource_expr(
        &mut self,
        expr: HirExprId,
        locals: &HashMap<SymbolId, InterpValue>,
        captured: Option<&Frame>,
    ) -> Option<InterpValue> {
        match self.checked.hir.exprs[expr].clone() {
            HirExpr::Literal(literal) => self.eval_literal(expr, &literal).ok(),
            HirExpr::Path(path) => match path.resolution {
                ResolveResult::Resolved(symbol) => self.resource_symbol(symbol, locals, captured),
                ResolveResult::PartiallyResolved(partial) => {
                    let symbol = partial.resolved_prefix?;
                    let value = self.resource_symbol(symbol, locals, captured)?;
                    self.resource_fields(value, symbol, &partial.remaining)
                }
                _ => None,
            },
            HirExpr::Field { base, field, span } => {
                let value = self.resource_expr(base, locals, captured)?;
                self.eval_const_field_value(
                    crate::plan::FieldAccessSite::Expr(expr),
                    value,
                    &field,
                    span,
                )
                .ok()
            }
            _ => None,
        }
    }

    fn resource_symbol(
        &mut self,
        symbol: SymbolId,
        locals: &HashMap<SymbolId, InterpValue>,
        captured: Option<&Frame>,
    ) -> Option<InterpValue> {
        if let Some(value) = locals.get(&symbol) {
            return Some(value.clone());
        }
        if let Some(value) = captured.and_then(|frame| frame.get(symbol)) {
            return Some(value);
        }
        let data = self.checked.symbols.get(symbol)?.clone();
        if let SymbolDef::Local {
            initializer: Some(expr),
            ..
        } = data.def
        {
            return self.resource_expr(expr, locals, captured);
        }
        self.eval_global_symbol(symbol, data.definition_span).ok()
    }

    fn resource_fields(
        &mut self,
        mut value: InterpValue,
        symbol: SymbolId,
        fields: &[String],
    ) -> Option<InterpValue> {
        let span = self.checked.symbols.get(symbol)?.definition_span;
        for (index, field) in fields.iter().enumerate() {
            value = self
                .eval_const_field_value(
                    crate::plan::FieldAccessSite::Path {
                        prefix: symbol,
                        remaining: fields,
                        index,
                    },
                    value,
                    field,
                    span,
                )
                .ok()?;
        }
        Some(value)
    }

    pub(super) fn unresolved_resource_scope(&mut self) {
        let scope = self.scheduler.serialize_invocation();
        self.events
            .push(crate::orchestration::WorkflowEvent::ScopeSerialized {
                scope: scope.0,
                reason: "unresolved_resource_selector".into(),
            });
    }
}
