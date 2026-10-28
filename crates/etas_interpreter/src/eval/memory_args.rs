use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn resume_memory_args(
        &mut self,
        resume: MemoryArgsResume,
        frame: &mut Frame,
    ) -> ControlSignal {
        let MemoryArgsResume {
            region_stable_id,
            path,
            key_type,
            value_type,
            result_type,
            method,
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
                        Continuation::MemoryArgs {
                            region_stable_id: region_stable_id.clone(),
                            path: path.clone(),
                            key_type,
                            value_type,
                            result_type,
                            method: method.clone(),
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
                ControlSignal::Cancelled(cause) => return ControlSignal::Cancelled(cause),
                ControlSignal::Continue => return ControlSignal::Continue,
            }
        }

        let query_type = if matches!(method.as_str(), "select" | "query" | "related_to") {
            let Some(arg) = args.first() else {
                return ControlSignal::invalid_arguments("memory query requires an argument", span);
            };
            let expr = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            };
            match self.materialized_expression_type(expr, frame.generic_bindings(), span) {
                Ok(ty) => Some(ty),
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            }
        } else {
            None
        };
        self.finish_memory_store_method(MemoryStoreArgs {
            region_stable_id,
            path,
            key_type,
            value_type,
            result_type,
            query_type,
            method,
            evaluated_args,
            span,
        })
    }
}

pub(super) struct MemoryArgsResume {
    pub region_stable_id: String,
    pub path: Vec<String>,
    pub key_type: etas_types::TypeId,
    pub value_type: etas_types::TypeId,
    pub result_type: etas_types::TypeId,
    pub method: String,
    pub args: Vec<HirArg>,
    pub start_arg_index: usize,
    pub evaluated_args: Vec<InterpValue>,
    pub span: Span,
}

impl MemoryArgsResume {
    pub(super) fn from_eval(eval: MemoryStoreMethodEval<'_>) -> Self {
        Self {
            region_stable_id: eval.region_stable_id,
            path: eval.path,
            key_type: eval.key_type,
            value_type: eval.value_type,
            result_type: eval.result_type,
            method: eval.method.to_owned(),
            args: eval.args.to_vec(),
            start_arg_index: 0,
            evaluated_args: Vec::new(),
            span: eval.span,
        }
    }
}
