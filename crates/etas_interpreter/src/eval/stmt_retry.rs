use super::*;

struct RetryResumeState {
    retry: RetryAttemptRecord,
    body: HirBlockId,
    attempts: usize,
    next_attempt: usize,
    block: HirBlockId,
    next_stmt_index: usize,
    frame: Frame,
}

impl<'a> EvalContext<'a> {
    pub(super) fn execute_retry_stmt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        body: HirBlockId,
        limits: &[HirExprId],
        span: Span,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let attempts = match self.retry_attempts(limits, span) {
            Ok(attempts) => attempts as usize,
            Err(fault) => return Some(ControlSignal::Fault(Box::new(fault))),
        };
        if attempts == 0 {
            self.events.push(WorkflowEvent::RetryExhausted);
            return None;
        }
        self.execute_retry_attempt(block, stmt_index, body, attempts, 0, frame)
    }

    pub(crate) fn retry_boundary_failure_signal(
        &mut self,
        continuation: Continuation,
        span: Span,
        message: String,
    ) -> Option<ControlSignal> {
        self.retry_failure_from_continuation(continuation, span, &message)
    }

    fn retry_failure_from_continuation(
        &mut self,
        continuation: Continuation,
        span: Span,
        message: &str,
    ) -> Option<ControlSignal> {
        let mut work = vec![continuation];
        while let Some(continuation) = work.pop() {
            match continuation {
                Continuation::RetryAttempt {
                    retry,
                    body,
                    attempts,
                    next_attempt,
                    block,
                    next_stmt_index,
                    frame,
                } => {
                    return Some(self.retry_attempt_failed(
                        RetryResumeState {
                            retry,
                            body,
                            attempts,
                            next_attempt,
                            block,
                            next_stmt_index,
                            frame,
                        },
                        span,
                        message,
                    ));
                }
                Continuation::Chain { inner, outer } => {
                    work.push(*outer);
                    work.push(*inner);
                }
                _ => {}
            }
        }
        None
    }

    fn retry_attempt_failed(
        &mut self,
        mut state: RetryResumeState,
        span: Span,
        message: &str,
    ) -> ControlSignal {
        if self
            .retry_stack
            .last()
            .is_some_and(|current| current.id == state.retry.id)
        {
            self.retry_stack.pop();
        }
        self.events
            .push(WorkflowEvent::RetryAttemptFailed(state.retry.id));

        if state.next_attempt >= state.attempts {
            self.events.push(WorkflowEvent::RetryExhausted);
            return ControlSignal::invalid_arguments(
                format!(
                    "retry exhausted after {} attempt(s): {message}",
                    state.attempts
                ),
                span,
            );
        }

        match self.execute_retry_attempt(
            state.block,
            state.next_stmt_index - 1,
            state.body,
            state.attempts,
            state.next_attempt,
            &mut state.frame,
        ) {
            Some(signal) => signal,
            None => self.execute_block_from(state.block, state.next_stmt_index, &mut state.frame),
        }
    }

    fn execute_retry_attempt(
        &mut self,
        block: HirBlockId,
        stmt_index: usize,
        body: HirBlockId,
        attempts: usize,
        attempt: usize,
        frame: &mut Frame,
    ) -> Option<ControlSignal> {
        let retry = RetryAttemptRecord {
            id: RetryAttemptId(self.next_retry),
            ordinal: attempt as u32,
        };
        self.next_retry += 1;
        self.retry_stack.push(retry.clone());
        self.events
            .push(WorkflowEvent::RetryAttemptStarted(retry.id));
        let signal = self.execute_block(body, frame);
        match signal {
            ControlSignal::Cancelled(cause) => Some(ControlSignal::Cancelled(cause)),
            signal @ (ControlSignal::Apply(_)
            | ControlSignal::Block(_)
            | ControlSignal::Expr(_)
            | ControlSignal::Call(_)
            | ControlSignal::Memory(_)
            | ControlSignal::Session(_)
            | ControlSignal::Perform(_)
            | ControlSignal::Console(_)
            | ControlSignal::Command(_)
            | ControlSignal::Model(_)
            | ControlSignal::Host(_)) => Some(self.pending_retry_signal(
                signal,
                RetryResumeState {
                    retry,
                    body,
                    attempts,
                    next_attempt: attempt + 1,
                    block,
                    next_stmt_index: stmt_index + 1,
                    frame: frame.clone(),
                },
            )),
            ControlSignal::Checkpoint(pending) => Some(ControlSignal::Checkpoint(pending)),
            ControlSignal::Return(value) => {
                self.finish_retry_attempt_success(&retry);
                Some(ControlSignal::Return(value))
            }
            ControlSignal::Resume(value) => {
                self.finish_retry_attempt_success(&retry);
                Some(ControlSignal::Resume(value))
            }
            ControlSignal::Finish(value) => {
                self.finish_retry_attempt_success(&retry);
                Some(ControlSignal::Finish(value))
            }
            ControlSignal::Fault(fault) => {
                if self
                    .retry_stack
                    .last()
                    .is_some_and(|current| current.id == retry.id)
                {
                    self.retry_stack.pop();
                }
                self.events
                    .push(WorkflowEvent::RetryAttemptFailed(retry.id));
                Some(ControlSignal::Fault(fault))
            }
            ControlSignal::Break => {
                self.finish_retry_attempt_success(&retry);
                Some(ControlSignal::Break)
            }
            ControlSignal::Continue => {
                self.finish_retry_attempt_success(&retry);
                Some(ControlSignal::Continue)
            }
            ControlSignal::Value(_) => {
                self.finish_retry_attempt_success(&retry);
                None
            }
        }
    }

    fn pending_retry_signal(
        &mut self,
        signal: ControlSignal,
        state: RetryResumeState,
    ) -> ControlSignal {
        let continuation = Continuation::RetryAttempt {
            retry: state.retry,
            body: state.body,
            attempts: state.attempts,
            next_attempt: state.next_attempt,
            block: state.block,
            next_stmt_index: state.next_stmt_index,
            frame: state.frame,
        };
        compose_signal_continuation(signal, continuation)
    }
}
