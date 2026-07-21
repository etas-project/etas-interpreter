use super::*;

impl<'a> EvalContext<'a> {
    pub(super) fn assign_target_in_block(
        &mut self,
        block: HirBlockId,
        next_stmt_index: usize,
        target: HirExprId,
        value: InterpValue,
        frame: &mut Frame,
        span: Span,
    ) -> Option<ControlSignal> {
        self.assign_target_with_resume(target, value, frame, span, Some((block, next_stmt_index)))
            .err()
            .map(|signal| *signal)
    }

    fn assign_target_with_resume(
        &mut self,
        target: HirExprId,
        value: InterpValue,
        frame: &mut Frame,
        span: Span,
        resume: Option<(HirBlockId, usize)>,
    ) -> Result<(), Box<ControlSignal>> {
        let (root_symbol, segments) =
            self.resolve_local_place(target, frame, span, value.clone(), resume)?;
        self.assign_resolved_local_place(root_symbol, &segments, value, frame, span)
    }

    pub(super) fn assign_resolved_local_place(
        &mut self,
        root_symbol: SymbolId,
        segments: &[LocalPlaceSegment],
        value: InterpValue,
        frame: &mut Frame,
        span: Span,
    ) -> Result<(), Box<ControlSignal>> {
        if segments.is_empty() {
            if frame.set(root_symbol, value) {
                return Ok(());
            }
            let message = "assignment target is not a mutable local slot".to_owned();
            return Err(Box::new(ControlSignal::invalid_arguments(message, span)));
        }
        let Some(mut root_value) = frame.get(root_symbol) else {
            let message = "assignment target root local is missing at runtime".to_owned();
            return Err(Box::new(ControlSignal::invalid_arguments(message, span)));
        };
        if let Err(fault) = self.assign_nested_value(&mut root_value, segments, value, span) {
            return Err(Box::new(ControlSignal::Fault(Box::new(fault))));
        }
        if frame.set(root_symbol, root_value) {
            Ok(())
        } else {
            let message = "assignment target is not a mutable local slot".to_owned();
            Err(Box::new(ControlSignal::invalid_arguments(message, span)))
        }
    }
}
