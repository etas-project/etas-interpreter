use std::collections::BTreeSet;

use etas_frontend::CheckedCallableRoot;
use etas_utils::{ArtifactSet, Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::item_span;
use crate::plan::context::{PlanContext, plan_pass};

use super::{ResidualInstallationEvidence, UNINSTALLED_RESIDUAL, artifacts::*};

/// Proves that the selected callable and every callable reachable from it
/// either has no residual references or has an installed scope factory whose
/// plans cover exactly its checked references, and that every governed
/// action those callables may request has a computable monitor label.
/// Propagated references are covered by their reachable callee's factory,
/// not by inventing an extra lexical scope on the consumer.
pub(in crate::plan) struct VerifyResidualCoveragePass;

impl Pass<PlanContext<'_>> for VerifyResidualCoveragePass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.verify_residual_coverage")
            .requires(ArtifactSet::from([
                SELECTED_CALLABLE,
                PREPARED_RESIDUAL_PLANS,
            ]))
            .produces(ArtifactSet::one(VERIFIED_RESIDUAL_COVERAGE))
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.residual_installation = None;
        let (Some(root), Some(entry)) = (context.requested_root, context.entry) else {
            return PassResult::failed("residual coverage has no selected checked callable");
        };
        let Some(prepared) = context.prepared_residual.clone() else {
            return PassResult::failed("residual coverage lost its prepared plan artifact");
        };
        let project = context.project;
        let entry_span = item_span(project, entry.item);
        let root_summary = match project.checked_callable_effects(root) {
            Ok(summary) => summary,
            Err(error) => {
                context.push_missing_fact(entry_span, &error.to_string());
                return PassResult::failed("residual coverage validation failed");
            }
        };
        let reachable = match project.checked_callable_reachability(root) {
            Ok(reachable) => reachable,
            Err(error) => {
                context.push_missing_fact(entry_span, &error.to_string());
                return PassResult::failed("residual coverage validation failed");
            }
        };
        let mut items = BTreeSet::new();
        match root {
            CheckedCallableRoot::Item(item) => {
                items.insert(item);
            }
            CheckedCallableRoot::Lambda(_) | CheckedCallableRoot::StdReference(_) => {
                // Only checked callables own residual scopes.
                if !root_summary.residual_checks.is_empty() {
                    context.push_missing_fact(entry_span, UNINSTALLED_RESIDUAL);
                    return PassResult::failed("residual coverage validation failed");
                }
            }
        }
        items.extend(reachable);
        let mut required_all = root_summary.residual_checks.iter().collect::<BTreeSet<_>>();
        let mut installed_all = BTreeSet::new();
        for item in items {
            match project.hir.items.get(item) {
                Some(etas_hir::HirItem::Agent(etas_hir::HirAgentDecl {
                    body: etas_hir::HirAgentBody::Decl { .. },
                    ..
                })) => continue,
                Some(
                    etas_hir::HirItem::Flow(_)
                    | etas_hir::HirItem::Agent(_)
                    | etas_hir::HirItem::Tool(_)
                    | etas_hir::HirItem::TopLevelLet(_),
                ) => {}
                Some(_) => continue, // Constructor/type dependencies do not execute callable bodies.
                None => {
                    return PassResult::failed(
                        "reachable residual dependency has no checked HIR item",
                    );
                }
            }
            let Some(summary) = project.effects.item_effects.get(&item) else {
                context.push_missing_fact(
                    item_span(project, item),
                    "reachable callable has no checked effect summary",
                );
                return PassResult::failed("residual coverage validation failed");
            };
            required_all.extend(summary.residual_checks.iter());
            let required = project.residual_plans().item_references(item);
            if required.is_empty() {
                continue;
            }
            let span = item_span(project, item);
            let Some(scope) = prepared.scope(item) else {
                context.push_missing_fact(span, UNINSTALLED_RESIDUAL);
                continue;
            };
            let installed = scope
                .plans
                .iter()
                .flat_map(|plan| plan.checked.references().iter())
                .collect::<BTreeSet<_>>();
            if installed != required {
                context.push_missing_fact(span, UNINSTALLED_RESIDUAL);
                continue;
            }
            installed_all.extend(installed);
        }
        let required_all = required_all.into_iter().cloned().collect();
        if !project.residual_plans().supports_references(&required_all)
            || required_all.iter().any(|reference| {
                !installed_all
                    .iter()
                    .any(|installed| installed.obligation() == reference.obligation())
            })
        {
            context.push_missing_fact(
                entry_span,
                &format!(
                    "{UNINSTALLED_RESIDUAL}: reachable obligation has no installed callable scope"
                ),
            );
        }
        if context
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return PassResult::failed("residual coverage validation failed");
        }
        match ResidualInstallationEvidence::after_coverage(project.clone(), root, prepared) {
            Ok(evidence) => {
                context.residual_installation = Some(evidence);
                produced(VERIFIED_RESIDUAL_COVERAGE)
            }
            Err(reason) => PassResult::failed(reason),
        }
    }
}
