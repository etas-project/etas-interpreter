use super::host_value::interp_to_host_value;
use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn eval_memory_selection_method(
        &mut self,
        eval: MemorySelectionMethodEval<'_>,
        frame: &mut Frame,
    ) -> ControlSignal {
        if eval.method != "limit" {
            return ControlSignal::invalid_arguments(
                format!("unsupported memory selection method `{}`", eval.method),
                eval.span,
            );
        }
        self.resume_memory_selection_limit(MemorySelectionLimitResume::from_eval(eval), frame)
    }

    pub(super) fn resume_memory_selection_limit(
        &mut self,
        resume: MemorySelectionLimitResume,
        frame: &mut Frame,
    ) -> ControlSignal {
        let MemorySelectionLimitResume {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate,
            limit: existing_limit,
            args,
            start_arg_index,
            mut evaluated_args,
            span,
        } = resume;
        for (index, arg) in args.iter().enumerate().skip(start_arg_index) {
            let expr = match arg {
                HirArg::Positional(value) | HirArg::Named { value, .. } => *value,
            };
            match self.eval_expr(expr, frame) {
                ControlSignal::Value(value) => evaluated_args.push(value),
                signal @ (ControlSignal::Apply(_)
                | ControlSignal::Checkpoint(_)
                | ControlSignal::Block(_)
                | ControlSignal::Expr(_)
                | ControlSignal::Call(_)
                | ControlSignal::Perform(_)
                | ControlSignal::Memory(_)
                | ControlSignal::Session(_)
                | ControlSignal::Console(_)
                | ControlSignal::Command(_)
                | ControlSignal::Model(_)
                | ControlSignal::Host(_)) => {
                    return compose_signal_continuation(
                        signal,
                        Continuation::MemorySelectionLimitArgs {
                            region_stable_id: region_stable_id.clone(),
                            path: path.clone(),
                            key_type,
                            value_type,
                            kind: kind.clone(),
                            predicate: predicate.clone(),
                            limit: existing_limit,
                            args: args.clone(),
                            next_arg_index: index + 1,
                            evaluated_args,
                            span,
                            frame: frame.clone(),
                        },
                    );
                }
                ControlSignal::Return(value) => return ControlSignal::Return(value),
                ControlSignal::Resume(value) => return ControlSignal::Resume(value),
                ControlSignal::Finish(value) => return ControlSignal::Finish(value),
                ControlSignal::Break => return ControlSignal::Break,
                ControlSignal::Fault(fault) => return ControlSignal::Fault(fault),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }

        let limit = match evaluated_args.as_slice() {
            [InterpValue::Variant { name, fields }]
                if matches!(
                    name.as_str(),
                    "Iterations" | "Tokens" | "ContextTokens" | "Attempts"
                ) =>
            {
                let [InterpValue::Number(value)] = fields.as_slice() else {
                    return abort_memory_selection(
                        span,
                        "memory selection limit requires a count limit constructor with one integer field",
                    );
                };
                match value.as_u32() {
                    Some(value) => value,
                    None => {
                        return abort_memory_selection(
                            span,
                            "memory selection limit exceeds the maximum supported count",
                        );
                    }
                }
            }
            [_] => {
                return abort_memory_selection(
                    span,
                    "memory selection limit requires a count limit such as Tokens(n)",
                );
            }
            _ => {
                return abort_memory_selection(
                    span,
                    "MemorySelection.limit expects exactly one argument",
                );
            }
        };

        ControlSignal::Value(InterpValue::MemorySelection {
            region_stable_id,
            path,
            key_type,
            value_type,
            kind,
            predicate: predicate.map(Box::new),
            limit: Some(limit),
        })
    }

    pub(super) fn resume_memory_clear_delete_keys(
        &mut self,
        region_stable_id: String,
        path: Vec<String>,
        remaining_keys: Vec<InterpValue>,
        next_index: usize,
        span: Span,
    ) -> ControlSignal {
        if next_index >= remaining_keys.len() {
            return ControlSignal::Value(InterpValue::Unit);
        }
        let key = match interp_to_host_value(&remaining_keys[next_index]) {
            Ok(key) => key,
            Err(error) => {
                return abort_memory_selection(
                    span,
                    format!("memory clear encountered a non-host-encodable key: {error}"),
                );
            }
        };
        let request_id = HostRequestId(self.next_host_request);
        self.next_host_request += 1;
        ControlSignal::pending_memory(PendingMemory {
            request: MemoryRequest {
                id: request_id,
                store: StoreRef {
                    region: MemoryRegionRef {
                        stable_id: region_stable_id.clone(),
                        schema_fingerprint: None,
                    },
                    path: path.clone(),
                },
                operation: MemoryOperation::Delete {
                    key,
                    expected: None,
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            },
            decode: MemoryDecode::Unit,
            span,
            continuation: Continuation::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys,
                next_index: next_index + 1,
                span,
            },
        })
    }
}

fn abort_memory_selection(span: Span, message: impl Into<String>) -> ControlSignal {
    ControlSignal::invalid_arguments(message.into(), span)
}

pub(super) struct MemorySelectionMethodEval<'a> {
    pub region_stable_id: String,
    pub path: Vec<String>,
    pub key_type: etas_types::TypeId,
    pub value_type: etas_types::TypeId,
    pub kind: crate::value::MemorySelectionKind,
    pub predicate: Option<InterpValue>,
    pub limit: Option<u32>,
    pub method: &'a str,
    pub args: &'a [HirArg],
    pub span: Span,
}

pub(super) struct MemorySelectionLimitResume {
    pub region_stable_id: String,
    pub path: Vec<String>,
    pub key_type: etas_types::TypeId,
    pub value_type: etas_types::TypeId,
    pub kind: crate::value::MemorySelectionKind,
    pub predicate: Option<InterpValue>,
    pub limit: Option<u32>,
    pub args: Vec<HirArg>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}

impl MemorySelectionLimitResume {
    fn from_eval(eval: MemorySelectionMethodEval<'_>) -> Self {
        Self {
            region_stable_id: eval.region_stable_id,
            path: eval.path,
            key_type: eval.key_type,
            value_type: eval.value_type,
            kind: eval.kind,
            predicate: eval.predicate,
            limit: eval.limit,
            args: eval.args.to_vec(),
            start_arg_index: 0,
            evaluated_args: Vec::new(),
            span: eval.span,
        }
    }
}
