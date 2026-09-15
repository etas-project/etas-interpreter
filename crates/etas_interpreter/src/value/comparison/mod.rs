mod engine;
mod runtime;
pub(crate) use engine::{Comparison, CursorStep, EqualityCursor, compare};
pub(super) use runtime::{messages_equal, sets_equal, value_equal};

/// Matching continuations retain failed candidates instead of recursing into
/// equality. Every right member can satisfy at most one left member.
pub(crate) struct SetSearch {
    left: usize,
    candidate: usize,
    last_right: usize,
    len: usize,
    matched: Option<Vec<bool>>,
}

impl SetSearch {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            left: 0,
            candidate: 0,
            last_right: 0,
            len,
            matched: Some(vec![false; len]),
        }
    }

    /// SetValue storage already enforces uniqueness; no match bitmap is needed.
    pub(crate) fn unique(len: usize) -> Self {
        Self {
            left: 0,
            candidate: 0,
            last_right: 0,
            len,
            matched: None,
        }
    }

    pub(crate) fn next(
        &mut self,
        previous: Option<bool>,
        mut candidate: impl FnMut(usize, usize) -> Option<usize>,
    ) -> Result<Option<(usize, usize)>, ()> {
        if previous == Some(true) {
            if let Some(matched) = &mut self.matched {
                matched[self.last_right] = true;
            }
            self.left += 1;
            self.candidate = 0;
        }
        if self.left == self.len {
            return Ok(None);
        }
        loop {
            let right = candidate(self.left, self.candidate).ok_or(())?;
            self.candidate += 1;
            if self.matched.as_ref().is_none_or(|matched| !matched[right]) {
                self.last_right = right;
                return Ok(Some((self.left, right)));
            }
        }
    }
}
