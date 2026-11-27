//! The value walker implements structural language equality. Standard
//! collections delegate their logical comparison to checked Etas method bodies.
use super::*;
use crate::value::comparison::SetSearch;

type Pair = (InterpValue, InterpValue);

#[derive(Clone, Debug)]
enum EqualityFrame {
    Fields {
        pairs: Vec<Pair>,
        next: usize,
    },
    Unordered {
        left: Vec<InterpValue>,
        right: Vec<InterpValue>,
        search: SetSearch,
    },
}

#[derive(Clone, Debug)]
pub struct ValueEquality {
    current: Option<Pair>,
    parents: Vec<EqualityFrame>,
    awaiting_call: bool,
    negate: bool,
    span: Span,
}

impl ValueEquality {
    fn fields(&mut self, pairs: Vec<Pair>) -> Option<bool> {
        if pairs.is_empty() {
            return Some(true);
        }
        self.current = Some(pairs[0].clone());
        self.parents.push(EqualityFrame::Fields { pairs, next: 1 });
        None
    }

    fn unordered(&mut self, left: Vec<InterpValue>, right: Vec<InterpValue>) -> Option<bool> {
        if left.len() != right.len() {
            return Some(false);
        }
        let len = left.len();
        let mut search = SetSearch::new(len);
        match search.next(None, |_, candidate| (candidate < len).then_some(candidate)) {
            Ok(Some((a, b))) => {
                self.current = Some((left[a].clone(), right[b].clone()));
                self.parents.push(EqualityFrame::Unordered {
                    left,
                    right,
                    search,
                });
                None
            }
            Ok(None) => Some(true),
            Err(()) => Some(false),
        }
    }

    fn sequence<'a>(
        &mut self,
        left: impl Iterator<Item = &'a InterpValue>,
        right: impl Iterator<Item = &'a InterpValue>,
    ) -> Option<bool> {
        let left = left.collect::<Vec<_>>();
        let right = right.collect::<Vec<_>>();
        if left.len() != right.len() {
            return Some(false);
        }
        self.fields(
            left.into_iter()
                .zip(right)
                .map(|(a, b)| (a.clone(), b.clone()))
                .collect(),
        )
    }

    fn propagate(&mut self, mut result: bool) -> Option<bool> {
        while let Some(parent) = self.parents.last_mut() {
            match parent {
                EqualityFrame::Fields { pairs, next } => {
                    if result && let Some(pair) = pairs.get(*next) {
                        self.current = Some(pair.clone());
                        *next += 1;
                        return None;
                    }
                    self.parents.pop();
                }
                EqualityFrame::Unordered {
                    left,
                    right,
                    search,
                } => {
                    let len = right.len();
                    match search.next(Some(result), |_, candidate| {
                        (candidate < len).then_some(candidate)
                    }) {
                        Ok(Some((a, b))) => {
                            self.current = Some((left[a].clone(), right[b].clone()));
                            return None;
                        }
                        Ok(None) => result = true,
                        Err(()) => result = false,
                    }
                    self.parents.pop();
                }
            }
        }
        Some(result)
    }
}

impl EvalContext<'_> {
    pub(super) fn eval_binary_with_equality(
        &mut self,
        op: etas_hir::HirBinaryOp,
        left: InterpValue,
        right: InterpValue,
        span: Span,
    ) -> ControlSignal {
        if matches!(
            op,
            etas_hir::HirBinaryOp::EqEq | etas_hir::HirBinaryOp::BangEq
        ) && self.checked.hir.source_standard_types
            && matches!(
                &left,
                InterpValue::Nominal { .. }
                    | InterpValue::Array(_)
                    | InterpValue::List(_)
                    | InterpValue::Slice(_)
                    | InterpValue::Stack(_)
                    | InterpValue::Queue(_)
                    | InterpValue::Deque(_)
                    | InterpValue::Tuple(_)
                    | InterpValue::Record(_)
                    | InterpValue::Variant { .. }
                    | InterpValue::OptionSome(_)
                    | InterpValue::Trust { .. }
                    | InterpValue::Map(_)
                    | InterpValue::Set(_)
                    | InterpValue::Existential(_)
            )
        {
            return self.continue_value_equality(
                ValueEquality {
                    current: Some((left, right)),
                    parents: Vec::new(),
                    awaiting_call: false,
                    negate: op == etas_hir::HirBinaryOp::BangEq,
                    span,
                },
                InterpValue::Unit,
            );
        }
        match self.eval_binary_values(op, left, right, span) {
            Ok(value) => ControlSignal::Value(value),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    pub(crate) fn continue_value_equality(
        &mut self,
        mut state: ValueEquality,
        value: InterpValue,
    ) -> ControlSignal {
        let mut result = if state.awaiting_call {
            let InterpValue::Bool(equal) = value else {
                return ControlSignal::missing_checked_fact(
                    "source value equality did not return bool",
                    state.span,
                );
            };
            state.awaiting_call = false;
            Some(equal)
        } else {
            None
        };
        loop {
            if let Some(equal) = result.take() {
                if let Some(equal) = state.propagate(equal) {
                    return ControlSignal::Value(InterpValue::Bool(equal != state.negate));
                }
            }
            let (left, right) = state
                .current
                .take()
                .expect("value equality has a pending pair");
            if let Err(fault) = self.consume_execution_step(state.span) {
                return ControlSignal::Fault(Box::new(fault));
            }
            match (&left, &right) {
                (
                    InterpValue::Nominal { ty: a, value: av },
                    InterpValue::Nominal { ty: b, value: bv },
                ) => {
                    if a != b {
                        result = Some(false);
                        continue;
                    }
                    if etas_types::source_execution::is_scoped_handle(
                        &self.checked.type_store,
                        &self.checked.types.known_std_types,
                        *a,
                    ) {
                        return ControlSignal::invalid_arguments(
                            "live execution handles cannot be compared",
                            state.span,
                        );
                    }
                    match self.source_value_equality_target(*a) {
                        Ok(Some(target)) => {
                            state.awaiting_call = true;
                            return ControlSignal::pending_call(PendingCall {
                                purity: crate::control::CallPurity::Pure,
                                site: None,
                                target,
                                args: vec![left, right],
                                span: state.span,
                                continuation: Continuation::ValueEquality(Box::new(state)),
                            });
                        }
                        Err(reason) => {
                            return ControlSignal::missing_checked_fact(reason, state.span);
                        }
                        Ok(None) => {
                            result = state.fields(vec![(av.as_ref().clone(), bv.as_ref().clone())])
                        }
                    }
                }
                (InterpValue::Array(a), InterpValue::Array(b)) => {
                    result = state.sequence(a.borrow().iter(), b.borrow().iter())
                }
                (InterpValue::Stack(a), InterpValue::Stack(b)) => {
                    result = state.sequence(a.borrow().iter(), b.borrow().iter())
                }
                (InterpValue::Queue(a), InterpValue::Queue(b))
                | (InterpValue::Deque(a), InterpValue::Deque(b)) => {
                    result = state.sequence(a.borrow().iter(), b.borrow().iter())
                }
                (InterpValue::List(a), InterpValue::List(b)) => {
                    result = state.sequence(a.iter(), b.iter())
                }
                (InterpValue::Slice(a), InterpValue::Slice(b)) => {
                    result = state.sequence(a.borrow().iter(), b.borrow().iter())
                }
                (InterpValue::Tuple(a), InterpValue::Tuple(b)) => {
                    result = state.sequence(a.iter(), b.iter())
                }
                (InterpValue::Record(a), InterpValue::Record(b)) => {
                    if a.borrow().len() != b.borrow().len() {
                        result = Some(false);
                        continue;
                    }
                    let pairs = a
                        .borrow()
                        .iter()
                        .map(|(name, value)| {
                            b.get_ref(name).map(|other| (value.clone(), other.clone()))
                        })
                        .collect::<Option<Vec<_>>>();
                    result = pairs.map_or(Some(false), |pairs| state.fields(pairs));
                }
                (
                    InterpValue::Variant {
                        name: a,
                        fields: av,
                    },
                    InterpValue::Variant {
                        name: b,
                        fields: bv,
                    },
                ) => {
                    result = if a == b {
                        state.sequence(av.iter(), bv.iter())
                    } else {
                        Some(false)
                    };
                }
                (InterpValue::OptionSome(a), InterpValue::OptionSome(b)) => {
                    result = state.fields(vec![(a.as_ref().clone(), b.as_ref().clone())])
                }
                (
                    InterpValue::Trust {
                        wrapper: a,
                        value: av,
                    },
                    InterpValue::Trust {
                        wrapper: b,
                        value: bv,
                    },
                ) => {
                    result = if a == b {
                        state.fields(vec![(av.as_ref().clone(), bv.as_ref().clone())])
                    } else {
                        Some(false)
                    };
                }
                (InterpValue::Existential(a), InterpValue::Existential(b)) => {
                    result = if a.object_type == b.object_type && a.concrete_type == b.concrete_type
                    {
                        state.fields(vec![(a.value.as_ref().clone(), b.value.as_ref().clone())])
                    } else {
                        Some(false)
                    };
                }
                (InterpValue::Map(a), InterpValue::Map(b)) => {
                    let entries = |map: &crate::value::MapValue| {
                        map.borrow()
                            .iter()
                            .map(|(key, value)| {
                                InterpValue::Tuple(vec![key.clone(), value.clone()].into())
                            })
                            .collect()
                    };
                    result = state.unordered(entries(a), entries(b));
                }
                (InterpValue::Set(a), InterpValue::Set(b)) => {
                    result = state.unordered(a.borrow().to_vec(), b.borrow().to_vec())
                }
                _ => match crate::value::comparison::checked_value_equal(
                    &left,
                    &right,
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                ) {
                    Ok(equal) => result = Some(equal),
                    Err(()) => {
                        return ControlSignal::invalid_arguments(
                            "live execution handles cannot be compared",
                            state.span,
                        );
                    }
                },
            }
        }
    }

    fn source_value_equality_target(
        &self,
        receiver: etas_types::TypeId,
    ) -> Result<Option<CallTarget>, String> {
        let owner = match self.checked.type_store.get(receiver) {
            Some(etas_types::Type::Applied { constructor, .. }) => {
                etas_types::TypeId(constructor.0)
            }
            _ => receiver,
        };
        let is_collection =
            self.checked
                .type_store
                .nominal_identity(owner)
                .is_some_and(|identity| {
                    etas_std::source_value_equality_owner(identity.module(), identity.declaration())
                });
        if !is_collection {
            return Ok(None);
        }
        let mut selected = None;
        for (method, _) in self.checked.types.value_equality_methods() {
            let Some(bindings) = etas_types::solver::spec_solver::match_implementation_subject(
                &self.checked.type_store,
                &method.owner_params,
                method.receiver_type,
                receiver,
            ) else {
                continue;
            };
            if selected.is_some() {
                return Err("source value equality has multiple checked bodies".into());
            }
            let target = match &method.target {
                etas_types::method::MethodTarget::Source { callable, .. } => {
                    CallTarget::FlowItem(*callable)
                }
                etas_types::method::MethodTarget::External { identity } => {
                    if self.checked.foreign_method(identity).is_some() {
                        self.foreign
                            .as_ref()
                            .ok_or("value equality has no live caller owner")?
                            .method(self.checked, identity)?
                    } else {
                        self.plan
                            .standard_library
                            .as_ref()
                            .ok_or("value equality has no executable library")?
                            .method(identity, &mut self.library_targets.borrow_mut())?
                    }
                }
                _ => return Err("value equality must have a checked Etas body".into()),
            };
            selected = Some(CallTarget::Specialized {
                target: target.into(),
                // These are receiver binders, not callable signature generics.
                // Keep their checked identities just as a normal method call does.
                type_bindings: Vec::new(),
                type_parameter_bindings: bindings.into_iter().collect(),
                effect_row_bindings: Vec::new(),
            });
        }
        if is_collection && selected.is_none() {
            return Err("standard collection has no checked source equality body".into());
        }
        Ok(selected)
    }
}
