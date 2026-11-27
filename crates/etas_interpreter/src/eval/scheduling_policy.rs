use super::*;
use crate::{
    driver::scheduler::{ReadyTaskSnapshot, SourceSchedulingPolicy},
    eval::machine::{EvalMachine, MachinePoll},
};

struct SelectedPolicy {
    program: crate::value::CheckedExecutionProgram,
    item: HirItemId,
    bindings: crate::control::GenericBindings,
    projections: Vec<crate::plan::code::RevisionProjection>,
}

impl EvalContext<'_> {
    pub(super) fn execute_resource_scope(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        bindings: &crate::control::GenericBindings,
        site: Option<crate::control::CallSite>,
        span: Span,
    ) -> ControlSignal {
        let Some(crate::control::CallSite {
            kind: crate::control::CallSiteKind::Expr { expr },
            ..
        }) = site
        else {
            return ControlSignal::missing_checked_fact(
                "resource scope has no checked invocation origin",
                span,
            );
        };
        let Some((InterpValue::Callable(body), rest)) = args.split_first() else {
            return ControlSignal::missing_checked_fact(
                "resource scope requires its checked callback",
                span,
            );
        };
        let cleanup = match rest {
            [] => None,
            [InterpValue::Callable(release)] => Some(release.clone()),
            _ => {
                return ControlSignal::missing_checked_fact(
                    "invalid resource cleanup callback",
                    span,
                );
            }
        };
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        let report_type = match self.materialized_type(signature.output, &bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let plan = match super::task::checked_concurrency_origin(self.checked, expr) {
            Ok(plan) => plan,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let policy = self
            .scheduler
            .current_source_policy(self.task.task_scope)
            .cloned();
        self.enter_owned_scope(
            body.clone(),
            plan,
            None,
            policy,
            Some((report_type, cleanup)),
            span,
        )
    }

    pub(super) fn execute_runtime_scope(
        &mut self,
        item: HirItemId,
        args: Vec<InterpValue>,
        _bindings: &crate::control::GenericBindings,
        site: Option<crate::control::CallSite>,
        span: Span,
    ) -> ControlSignal {
        let Some(crate::control::CallSite {
            kind: crate::control::CallSiteKind::Expr { expr },
            ..
        }) = site
        else {
            return ControlSignal::missing_checked_fact(
                "source scope entry has no checked call origin",
                span,
            );
        };
        let [
            InterpValue::Existential(instance),
            InterpValue::Callable(body),
            InterpValue::Bool(inherit_policy),
        ] = (match <Vec<InterpValue> as TryInto<[InterpValue; 3]>>::try_into(args) {
            Ok(args) => args,
            Err(_) => {
                return ControlSignal::missing_checked_fact(
                    "source scope entry argument ABI differs",
                    span,
                );
            }
        })
        else {
            return ControlSignal::missing_checked_fact(
                "source scope entry requires its checked policy and callback",
                span,
            );
        };
        let fallback = SourceSchedulingPolicy {
            program: self.checked_execution_program(),
            instance,
            entry: item,
            span,
        };
        let policy = if inherit_policy {
            self.scheduler
                .current_source_policy(self.task.task_scope)
                .cloned()
                .unwrap_or(fallback)
        } else {
            fallback
        };
        if let Err(reason) = policy.validate(&self.storage_limits) {
            return ControlSignal::runtime_fault(reason, span);
        }
        let plan = match super::task::checked_concurrency_origin(self.checked, expr) {
            Ok(plan) => plan,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        self.enter_task_scope(body, plan, None, Some(policy), span)
    }
}

impl SourceSchedulingPolicy {
    fn selected(&self) -> Result<SelectedPolicy, String> {
        let checked = &self.program.checked;
        let Some(HirItem::Flow(etas_hir::HirFlowDecl {
            body:
                etas_hir::HirFlowBody::Runtime {
                    role: etas_std::StdExecutionRole::ScopeEnter,
                    ..
                },
            ..
        })) = checked.hir.items.get(self.entry)
        else {
            return Err("policy entry has no verified execution role".into());
        };
        let etas_types::ItemSignature::Flow(signature) =
            &checked.types.item_signatures[&self.entry]
        else {
            unreachable!()
        };
        let Some(etas_types::Type::Existential { bounds }) =
            checked.type_store.get(signature.params[0])
        else {
            return Err("scope policy has no checked interface".into());
        };
        let methods = match &bounds[0].spec {
            etas_types::CheckedSpecRef::Source(spec) => {
                &checked.types.spec_signatures[spec].methods
            }
            etas_types::CheckedSpecRef::External(spec) => {
                &checked.types.external_specs.signatures[spec].methods
            }
            _ => return Err("scope policy interface has no checked method declarations".into()),
        };
        let member = &methods[0].identity;
        let (target, mut bindings) = self.program.plan.dispatch.policy_witness(
            checked,
            crate::value::ExistentialEvidence::Runtime(&self.instance),
            member,
        )?;
        match target {
            etas_types::method::MethodTarget::Source { callable, .. } => Ok(SelectedPolicy {
                program: self.program.clone(),
                item: callable,
                bindings,
                projections: Vec::new(),
            }),
            etas_types::method::MethodTarget::External { identity } => {
                let (program, projections) = match &self.program.foreign {
                    Some(link) => (link.owner.clone(), vec![link.projection.reversed()]),
                    None => self.instance.method_owner(checked, &identity)?,
                };
                let method = crate::plan::code::checked_foreign_method(
                    checked,
                    &program.checked,
                    &identity,
                )?;
                let item = method.item;
                let scope = etas_code::descriptor::GenericOwner::Method(identity);
                for edge in projections
                    .iter()
                    .rev()
                    .map(crate::plan::code::RevisionProjection::reversed)
                {
                    bindings.types = bindings
                        .types
                        .into_iter()
                        .map(|(name, ty)| Ok((name, edge.runtime_type(ty)?)))
                        .collect::<Result<_, String>>()?;
                    bindings.parameters = bindings
                        .parameters
                        .into_iter()
                        .map(|(parameter, ty)| {
                            Ok((
                                edge.type_in_scope(&scope, parameter)?,
                                edge.runtime_type(ty)?,
                            ))
                        })
                        .collect::<Result<_, String>>()?;
                    bindings.rows = bindings
                        .rows
                        .iter()
                        .map(|(name, row)| Ok((name.clone(), edge.effect_row(row.clone())?)))
                        .collect::<Result<_, String>>()?;
                }
                Ok(SelectedPolicy {
                    program,
                    item,
                    bindings,
                    projections,
                })
            }
            _ => Err(
                "scheduling policy witness has no executable source body in its checked program"
                    .into(),
            ),
        }
    }

    pub(crate) fn validate(&self, limits: &etas_host::StorageLimits) -> Result<(), String> {
        let selected = self.selected()?;
        let root = etas_frontend::CheckedCallableRoot::Item(selected.item);
        let summary = selected
            .program
            .checked
            .checked_callable_effects(root)
            .map_err(|error| error.to_string())?;
        super::collection_ordering::validate_pure_summary(summary)?;
        if !selected
            .program
            .checked
            .checked_callable_suspension(root)
            .map_err(|error| error.to_string())?
            .proven_non_suspending()
        {
            return Err("scheduling policy select must be proven non-suspending".into());
        }
        let value = InterpValue::Existential(self.instance.clone());
        let etas_types::ItemSignature::Flow(signature) =
            &self.program.checked.types.item_signatures[&self.entry]
        else {
            unreachable!()
        };
        crate::value::contract::runtime::validate_value(
            &self.program.checked,
            &self.program.plan,
            &value,
            signature.params[0],
            limits,
        )?;
        let mut discovery = crate::value::graph::FrameDiscovery::default();
        let mut live = false;
        let mut inspect = |value: &InterpValue| {
            live |= matches!(
                value,
                InterpValue::Task(_) | InterpValue::Scheduler(_) | InterpValue::Continuation(_)
            );
            false
        };
        let mut frames = discovery.frames_filtered(
            crate::value::graph::Node::Value(&value),
            true,
            |_| false,
            &mut inspect,
        );
        let mut visited = std::collections::HashSet::new();
        while let Some(frame) = frames.pop() {
            if !visited.insert(frame.capture_identity()) {
                continue;
            }
            let locals = frame.borrow_locals()?;
            for (_, value) in locals.sorted() {
                frames.extend(discovery.frames_filtered(
                    crate::value::graph::Node::Value(value),
                    true,
                    |_| false,
                    &mut inspect,
                ));
            }
        }
        if live {
            return Err(
                "scheduling policy cannot retain live task, scheduler or continuation values"
                    .into(),
            );
        }
        Ok(())
    }

    pub(crate) fn select(
        &mut self,
        eval: &mut EvalContext<'_>,
        ready: &[ReadyTaskSnapshot],
    ) -> Result<usize, ControlSignal> {
        let selected = self
            .selected()
            .map_err(|reason| ControlSignal::missing_checked_fact(reason, self.span))?;
        let SelectedPolicy {
            program: policy_program,
            item,
            bindings,
            projections,
        } = selected;
        let checked = &policy_program.checked;
        let HirItem::Flow(flow) = &checked.hir.items[item] else {
            unreachable!()
        };
        let mut scoped = eval.scoped_context(&policy_program, 0);
        let etas_types::ItemSignature::Flow(signature) = &checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        let ready_type = scoped
            .materialized_type(signature.params[1], &bindings, self.span)
            .map_err(|fault| ControlSignal::Fault(Box::new(fault)))?;
        let values = HostValue::List(
            ready
                .iter()
                .map(|entry| {
                    HostValue::Record(vec![
                        ("id".into(), HostValue::UInt(entry.handle.task.0.into())),
                        (
                            "enqueue_order".into(),
                            HostValue::UInt(entry.enqueue_order.into()),
                        ),
                    ])
                })
                .collect(),
        );
        let snapshot = super::host_value::decode_checked_host_value(values, ready_type, checked)
            .map_err(|reason| ControlSignal::missing_checked_fact(reason, self.span))?;
        let mut wrappers = vec![self.instance.clone()];
        let mut receiver = self.instance.value.as_ref().clone();
        while let InterpValue::Existential(object) = receiver {
            receiver = object.value.as_ref().clone();
            wrappers.push(object);
        }
        let mut receiver_type = wrappers
            .last()
            .expect("policy receiver wrapper")
            .concrete_type;
        for edge in projections
            .iter()
            .rev()
            .map(crate::plan::code::RevisionProjection::reversed)
        {
            (receiver_type, receiver) = edge
                .value(receiver_type, receiver, &self.program)
                .map_err(|reason| ControlSignal::missing_checked_fact(reason, self.span))?;
        }
        let symbol = flow.params[0];
        let mut frame = scoped
            .callable_frame_with_generics(flow.scope, bindings.clone(), self.span)
            .map_err(|fault| ControlSignal::Fault(Box::new(fault)))?;
        frame.insert(symbol, receiver.clone());
        let (lease, _) = frame
            .acquire_receiver(symbol)
            .map_err(|reason| ControlSignal::runtime_fault(reason, self.span))?;
        scoped.task.receiver_updates.track(&lease);
        let outcome = crate::control::ReceiverOutcome::new(0);
        let signal = scoped.execute_flow_with_receiver(
            item,
            flow,
            vec![receiver, snapshot],
            bindings,
            Some(outcome.clone()),
        );
        drop(scoped);
        let mut machine = EvalMachine::pure_signal(signal, self.span);
        let result = loop {
            let program = machine
                .program_scope()
                .map(|scope| scope.program)
                .unwrap_or_else(|| policy_program.clone());
            let mut scoped = eval.scoped_context(&program, 0);
            match machine.run_until_yield(&mut scoped) {
                MachinePoll::Complete(value) => break *value,
                MachinePoll::CooperativeYield | MachinePoll::ProgramChanged => continue,
                MachinePoll::Fault(fault) => return Err(ControlSignal::Fault(Box::new(fault))),
                MachinePoll::Cancelled(cause) => return Err(ControlSignal::Cancelled(cause)),
                MachinePoll::Yield(_) => {
                    return Err(ControlSignal::runtime_fault(
                        "scheduling policy attempted to suspend",
                        self.span,
                    ));
                }
            }
        };
        let InterpValue::Number(crate::value::NumericValue::USize(index)) = result else {
            return Err(ControlSignal::runtime_fault(
                "scheduling policy select did not return usize",
                self.span,
            ));
        };
        let index = usize::try_from(index).map_err(|_| {
            ControlSignal::runtime_fault("scheduling policy index exceeds platform size", self.span)
        })?;
        if index >= ready.len() {
            return Err(ControlSignal::runtime_fault(
                "scheduling policy selected an out-of-range ready index",
                self.span,
            ));
        }
        let updated = outcome
            .take()
            .map_err(|reason| ControlSignal::runtime_fault(reason, self.span))?;
        lease
            .commit(updated)
            .map_err(|reason| ControlSignal::runtime_fault(reason, self.span))?;
        let mut value = frame.get(symbol).expect("committed policy receiver");
        for edge in &projections {
            (receiver_type, value) = edge
                .value(receiver_type, value, &policy_program)
                .map_err(|reason| ControlSignal::missing_checked_fact(reason, self.span))?;
        }
        for mut wrapper in wrappers.into_iter().rev() {
            wrapper.value = crate::value::SharedValue::new(value);
            value = InterpValue::Existential(wrapper);
        }
        let InterpValue::Existential(instance) = value else {
            unreachable!()
        };
        self.instance = instance;
        Ok(index)
    }
}
