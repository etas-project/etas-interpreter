pub(crate) enum Comparison<C> {
    Ready(bool),
    Pending(C),
}

pub(crate) enum CursorStep<C> {
    Complete(bool),
    Child(Comparison<C>),
}

pub(crate) trait EqualityCursor: Sized {
    fn advance(&mut self, previous: Option<bool>) -> CursorStep<Self>;
}

/// Flat comparisons keep their cursor on the stack. Only descending into a
/// compound child saves a continuation, so auxiliary memory follows depth.
pub(crate) fn compare<C: EqualityCursor>(initial: Comparison<C>) -> bool {
    let mut current = match initial {
        Comparison::Ready(result) => return result,
        Comparison::Pending(cursor) => cursor,
    };
    let mut parents = Vec::new();
    let mut previous = None;
    loop {
        match current.advance(previous.take()) {
            CursorStep::Child(Comparison::Ready(result)) => previous = Some(result),
            CursorStep::Child(Comparison::Pending(child)) => {
                parents.push(current);
                current = child;
            }
            CursorStep::Complete(result) => match parents.pop() {
                Some(parent) => {
                    current = parent;
                    previous = Some(result);
                }
                None => return result,
            },
        }
    }
}
