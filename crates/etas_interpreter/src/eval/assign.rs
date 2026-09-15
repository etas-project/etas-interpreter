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
        let assignment = self.resolve_local_place(target, frame, span, value, resume)?;
        self.assign_resolved_local_place(
            assignment.root_symbol,
            &assignment.segments,
            assignment.value,
            frame,
            span,
        )
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
        let Some(result) = frame.with_local_mut(root_symbol, |root| {
            super::assign_nested::commit(root, segments, value, span)
        }) else {
            let message = "assignment target root local is missing at runtime".to_owned();
            return Err(Box::new(ControlSignal::invalid_arguments(message, span)));
        };
        result.map_err(|fault| Box::new(ControlSignal::Fault(Box::new(fault))))
    }
}

#[cfg(test)]
#[path = "assign_tests.rs"]
mod tests;
