pub mod api;
mod control;
mod diagnostics;
mod driver;
mod eval;
pub mod host;
mod intrinsic;
mod orchestration;
mod plan;
mod value;

use api::{EntryPoint, InterpValue, PlanOptions, PlanResult, RunOptions, RunResult};
use diagnostics::item_span;
use etas_frontend::CheckedProject;
use etas_hir::{HirItem, HirItemId};
use host::HostServices;

pub struct Interpreter;

impl Interpreter {
    pub fn plan(&self, project: &CheckedProject, _options: PlanOptions) -> PlanResult {
        plan::build_plan(project, _options)
    }

    pub async fn run_checked(
        &self,
        project: &CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &dyn HostServices,
        options: RunOptions,
    ) -> RunResult {
        let mut diagnostics = Vec::new();
        let mut events = Vec::new();
        let mut checkpoints = Vec::new();
        let profile = options.profile.clone();
        if let Err(message) = options.execution_limits.validate() {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, entry.item),
                message,
            ));
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        let plan_span = profile.span("interpreter.plan", "interpreter");
        let plan_result = self.plan(project, options.plan);
        plan_span.finish_ok();
        diagnostics.extend(plan_result.diagnostics);
        let Some(plan) = plan_result.plan else {
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        };
        if plan.entry != entry {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, entry.item),
                "requested interpreter entry does not match the planned checked-project entry",
            ));
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        if let Some(expected) = entry_arity(project, entry.item)
            && expected != args.len()
        {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, entry.item),
                format!(
                    "entry expects {expected} argument(s) but received {}",
                    args.len()
                ),
            ));
        }
        diagnostics.extend(host::validate_host_readiness(
            &plan,
            host.availability(),
            item_span(project, entry.item),
        ));
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        let prepare_span = profile.span("interpreter.prepare", "interpreter");
        let mut eval = eval::EvalContext::new(eval::EvalContextInput {
            checked: project,
            plan: &plan,
            host_context: options.host_context,
            model_policy: options.model_policy,
            execution_limits: options.execution_limits,
            consumed_steps: 0,
            current_session: options.current_session,
            entry_item: entry.item,
            entry_args: &args,
        });
        prepare_span.finish_ok();
        let signal = eval.execute_entry_signal(entry.item);
        let eval_span = profile.span("interpreter.eval", "interpreter");
        let value = driver::execute_entry(&mut eval, signal, host).await;
        if value.is_some() {
            eval_span.finish_ok();
        } else {
            eval_span.finish_error();
        }
        diagnostics.extend(eval.diagnostics);
        events.append(&mut eval.events);
        checkpoints.append(&mut eval.checkpoints);
        RunResult {
            value,
            diagnostics,
            events,
            checkpoints,
        }
    }

    pub async fn resume_checkpoint(
        &self,
        project: &CheckedProject,
        checkpoint: &orchestration::InterpreterCheckpoint,
        host: &dyn HostServices,
        options: RunOptions,
    ) -> RunResult {
        let mut diagnostics = Vec::new();
        let mut events = Vec::new();
        let mut checkpoints = Vec::new();
        let profile = options.profile.clone();
        if let Err(message) = checkpoint
            .compilation
            .validate_for_project(project, checkpoint.entry_item)
        {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, checkpoint.entry_item),
                format!("checkpoint compilation identity mismatch: {message}"),
            ));
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        if let Err(message) = options.execution_limits.validate() {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, checkpoint.entry_item),
                message,
            ));
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        let plan_span = profile.span("interpreter.plan", "interpreter");
        let plan_result = self.plan(project, options.plan);
        plan_span.finish_ok();
        diagnostics.extend(plan_result.diagnostics);
        let Some(plan) = plan_result.plan else {
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        };
        if let Err(message) =
            eval::machine::snapshot::SnapshotValidator::new(project, &plan.slots, &plan.dispatch)
                .validate_checkpoint(checkpoint)
        {
            diagnostics.push(diagnostics::invalid_arguments(
                item_span(project, checkpoint.entry_item),
                format!("checkpoint state validation failed: {message}"),
            ));
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        diagnostics.extend(host::validate_host_readiness(
            &plan,
            host.availability(),
            item_span(project, checkpoint.entry_item),
        ));
        if diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == etas_core::Severity::Error)
        {
            return RunResult {
                value: None,
                diagnostics,
                events,
                checkpoints,
            };
        }
        let execution_limits = checkpoint
            .execution_progress
            .original_limits
            .stricter(options.execution_limits);
        let mut host_context = options.host_context;
        host_context.budget = match checkpoint
            .host_state
            .budget
            .resume_under(&host_context.budget)
        {
            Ok(budget) => budget,
            Err(error) => {
                diagnostics.push(diagnostics::invalid_arguments(
                    item_span(project, checkpoint.entry_item),
                    format!("checkpoint budget cannot be resumed: {error}"),
                ));
                return RunResult {
                    value: None,
                    diagnostics,
                    events,
                    checkpoints,
                };
            }
        };
        let parent_trace = checkpoint.host_state.trace.trace_id;
        let resumed_trace = match (|| {
            loop {
                let trace_id = etas_host::TraceId::generate()?;
                if trace_id != parent_trace {
                    return Ok::<_, etas_host::HostError>(etas_host::TraceContext::resumed(
                        trace_id,
                        parent_trace,
                    ));
                }
            }
        })() {
            Ok(trace) => trace,
            Err(error) => {
                diagnostics.push(diagnostics::invalid_arguments(
                    item_span(project, checkpoint.entry_item),
                    error.to_string(),
                ));
                return RunResult {
                    value: None,
                    diagnostics,
                    events,
                    checkpoints,
                };
            }
        };
        host_context.trace = resumed_trace;
        let prepare_span = profile.span("interpreter.prepare", "interpreter");
        let mut eval = eval::EvalContext::new(eval::EvalContextInput {
            checked: project,
            plan: &plan,
            host_context,
            model_policy: options.model_policy,
            execution_limits,
            consumed_steps: checkpoint.execution_progress.consumed_steps,
            current_session: checkpoint
                .current_session
                .clone()
                .or(options.current_session),
            entry_item: checkpoint.entry_item,
            entry_args: &checkpoint.args,
        });
        prepare_span.finish_ok();
        let signal = eval.execute_from_checkpoint_signal(checkpoint);
        let eval_span = profile.span("interpreter.eval", "interpreter");
        let value =
            driver::execute_entry_from_snapshot(&mut eval, signal, host, &checkpoint.machine).await;
        if value.is_some() {
            eval_span.finish_ok();
        } else {
            eval_span.finish_error();
        }
        diagnostics.extend(eval.diagnostics);
        events.append(&mut eval.events);
        checkpoints.append(&mut eval.checkpoints);
        RunResult {
            value,
            diagnostics,
            events,
            checkpoints,
        }
    }
}

fn entry_arity(project: &CheckedProject, item: HirItemId) -> Option<usize> {
    match project.hir.items.get(item)? {
        HirItem::Flow(flow) => Some(flow.params.len()),
        HirItem::Agent(agent) => Some(agent.params.len()),
        _ => None,
    }
}

#[cfg(test)]
mod testing;
