use super::*;

impl<'a> EvalContext<'a> {
    pub(in crate::eval) fn eval_one_arg(
        &mut self,
        args: &[HirArg],
        span: Span,
        label: &str,
        frame: &mut Frame,
    ) -> Result<InterpValue, Box<ControlSignal>> {
        self.eval_exact_args(args, 1, span, label, frame)
            .map(|[value]| value)
    }

    pub(in crate::eval) fn eval_exact_args<const N: usize>(
        &mut self,
        args: &[HirArg],
        expected: usize,
        span: Span,
        label: &str,
        frame: &mut Frame,
    ) -> Result<[InterpValue; N], Box<ControlSignal>> {
        if args.len() != expected || N != expected {
            return Err(Box::new(ControlSignal::invalid_arguments(
                format!("{label} expects exactly {expected} argument(s)"),
                span,
            )));
        }
        let mut values = Vec::with_capacity(expected);
        for arg in args {
            let expr = match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => *expr,
            };
            match self.eval_expr(expr, frame) {
                ControlSignal::Value(value) => values.push(value),
                other => return Err(Box::new(other)),
            }
        }
        values.try_into().map_err(|_| {
            Box::new(ControlSignal::missing_checked_fact(
                format!("{label} checked argument count does not match its runtime arity"),
                span,
            ))
        })
    }

    pub(in crate::eval) fn expect_no_args(
        &self,
        args: &[HirArg],
        span: Span,
        label: &str,
    ) -> Option<ControlSignal> {
        (!args.is_empty()).then(|| {
            ControlSignal::invalid_arguments(format!("{label} expects no arguments"), span)
        })
    }
}
