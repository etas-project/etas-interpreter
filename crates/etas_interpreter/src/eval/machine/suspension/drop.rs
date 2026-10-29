use super::{Continuation, EvalFrame, ProgramContinuation, SuspendedProgram};

impl Drop for ProgramContinuation {
    fn drop(&mut self) {
        let Some(program) = self.take_unique() else {
            return;
        };
        let mut work = Vec::new();
        discard_segment(program, &mut work);
        while let Some(continuation) = work.pop() {
            match continuation {
                Continuation::ProgramResume(mut saved) => {
                    if let Some(program) = saved.take_unique() {
                        discard_segment(program, &mut work);
                    }
                }
                Continuation::Chain { inner, outer } => {
                    work.extend(inner.into_unique());
                    work.extend(outer.into_unique());
                }
                Continuation::CallBoundary { outer, .. }
                | Continuation::HandlerDispatch { outer } => work.extend(outer.into_unique()),
                Continuation::HandleBoundary { inner, .. }
                | Continuation::ReenterHandler { boundary: inner }
                | Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. } => work.extend(inner.into_unique()),
                _ => {}
            }
        }
    }
}

fn discard_segment(program: SuspendedProgram, work: &mut Vec<Continuation>) {
    work.push(program.continuation);
    for frame in program.frames {
        match frame {
            EvalFrame::PureBoundary(_) => (),
            EvalFrame::ProgramReturn(frame) => work.push(frame.outer),
            EvalFrame::Block(frame) => work.push(frame.continuation),
            EvalFrame::Call(frame) => work.push(frame.continuation),
            EvalFrame::Expr(frame) => work.push(frame.continuation),
            EvalFrame::Continuation(frame) => work.push(frame.continuation),
            EvalFrame::Handler(frame) => work.extend(frame.inner.into_unique()),
            EvalFrame::Retry(_) => {}
            EvalFrame::ModelLoop(frame) => {
                work.push(frame.pending.continuation);
                work.push(frame.outer_continuation);
            }
            EvalFrame::SourceToolReturn(frame) => {
                work.push(frame.model_loop.pending.continuation);
                work.push(frame.model_loop.outer_continuation);
            }
        }
    }
}
