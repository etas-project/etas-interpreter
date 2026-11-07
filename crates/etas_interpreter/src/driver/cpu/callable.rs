use crate::control::{CallTarget, GenericBindings};

/// A callable recipe; every Rc-backed runtime target is built on its worker.
#[derive(Clone, Debug)]
pub(crate) enum FrozenCpuCallable {
    Closure {
        expr: etas_hir::HirExprId,
        captures: Vec<(etas_hir::SymbolId, super::FrozenValue)>,
        bindings: GenericBindings,
    },
    BoundMethod {
        target: Box<Self>,
        receiver: super::FrozenValue,
        slot: u32,
    },
    FlowItem(etas_hir::HirItemId),
    ExternalFlow {
        callable: etas_code::CallableId,
        item: etas_hir::HirItemId,
    },
    Specialized {
        target: Box<Self>,
        types: Vec<(String, etas_types::TypeId)>,
        parameters: Vec<(etas_types::TypeId, etas_types::TypeId)>,
        rows: Vec<(String, etas_types::EffectRowRef)>,
    },
}

impl FrozenCpuCallable {
    pub(crate) fn capture(
        checked: &etas_frontend::CheckedProject,
        target: &CallTarget,
        minimum_work: u64,
    ) -> Option<Self> {
        Self::capture_with(checked, target, minimum_work, &GenericBindings::new())
    }

    fn capture_with(
        checked: &etas_frontend::CheckedProject,
        target: &CallTarget,
        minimum_work: u64,
        bindings: &GenericBindings,
    ) -> Option<Self> {
        Some(match target {
            CallTarget::Lambda { expr, captured } => {
                let fact = checked.cpu_parallel.closures.get(expr)?;
                let bindings = captured.generic_bindings();
                let contract = materialized_contract(checked, fact, minimum_work, bindings)?;
                Self::Closure {
                    expr: *expr,
                    captures: contract
                        .captures
                        .iter()
                        .map(|capture| {
                            Some((
                                capture.symbol,
                                super::FrozenValue::freeze(&captured.get(capture.symbol)?)?,
                            ))
                        })
                        .collect::<Option<_>>()?,
                    bindings: bindings.clone(),
                }
            }
            CallTarget::BoundMethod {
                target,
                receiver,
                slot,
            } => Self::BoundMethod {
                target: Box::new(Self::capture_with(checked, target, minimum_work, bindings)?),
                receiver: super::FrozenValue::freeze(receiver)?,
                slot: *slot,
            },
            CallTarget::FlowItem(item) => {
                let fact = checked.cpu_parallel.callables.get(item)?;
                materialized_contract(checked, fact, minimum_work, bindings)?;
                match fact.origin.as_ref()? {
                    etas_frontend::CpuCallableOrigin::Program => Self::FlowItem(*item),
                    etas_frontend::CpuCallableOrigin::Package(callable) => Self::ExternalFlow {
                        callable: callable.clone(),
                        item: *item,
                    },
                }
            }
            CallTarget::Specialized {
                target,
                type_bindings,
                type_parameter_bindings,
                effect_row_bindings,
            } => {
                let mut specialized = bindings.clone();
                specialized.types.extend(type_bindings.iter().cloned());
                specialized
                    .parameters
                    .extend(type_parameter_bindings.iter().copied());
                specialized.rows.extend(effect_row_bindings.iter().cloned());
                Self::Specialized {
                    target: Box::new(Self::capture_with(
                        checked,
                        target,
                        minimum_work,
                        &specialized,
                    )?),
                    types: type_bindings.clone(),
                    parameters: type_parameter_bindings.clone(),
                    rows: effect_row_bindings.clone(),
                }
            }
            _ => return None,
        })
    }

    pub(crate) fn thaw(&self, program: &super::PreparedCpuProgram) -> CallTarget {
        match self {
            Self::Closure {
                expr,
                captures,
                bindings,
            } => {
                let layout = program
                    .plan
                    .closures
                    .get(*expr)
                    .expect("checked closure layout");
                let mut captured = crate::control::Frame::with_generic_bindings(
                    layout.slots.clone(),
                    bindings.clone(),
                );
                for (symbol, value) in captures {
                    captured.insert(*symbol, value.clone().thaw());
                }
                CallTarget::Lambda {
                    expr: *expr,
                    captured,
                }
            }
            Self::BoundMethod {
                target,
                receiver,
                slot,
            } => CallTarget::BoundMethod {
                target: target.thaw(program).into(),
                receiver: crate::value::SharedValue::new(receiver.clone().thaw()),
                slot: *slot,
            },
            Self::FlowItem(item) => CallTarget::FlowItem(*item),
            Self::ExternalFlow { callable, item } => {
                assert!(
                    matches!(&program.checked.cpu_parallel.callables[item].origin,
                    Some(etas_frontend::CpuCallableOrigin::Package(owner)) if owner == callable),
                    "frozen package callable belongs to its prepared program"
                );
                CallTarget::FlowItem(*item)
            }
            Self::Specialized {
                target,
                types,
                parameters,
                rows,
            } => CallTarget::Specialized {
                target: target.thaw(program).into(),
                type_bindings: types.clone(),
                type_parameter_bindings: parameters.clone(),
                effect_row_bindings: rows.clone(),
            },
        }
    }
}

fn materialized_contract<'a>(
    checked: &etas_frontend::CheckedProject,
    fact: &'a etas_frontend::CpuParallelEligibilityFact,
    minimum_work: u64,
    bindings: &GenericBindings,
) -> Option<&'a etas_frontend::CpuFreezeContract> {
    let contract = fact.freeze_contract.as_ref()?;
    (fact.eligible
        && (minimum_work == 0 || fact.estimated_work.is_some_and(|work| work >= minimum_work))
        && contract.is_materialized(
            &checked.type_store,
            &mut bindings.substitution(&checked.type_store),
        ))
    .then_some(contract)
}
