use std::ops::Range;

use super::{StringValue, TextChunks};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TextRangeError {
    InvalidRange,
    CapacityExceeded,
}

/// Monotonic projections traverse the backing once. Whole results share it;
/// partial results own only their selected bytes, never a large parent view.
pub(crate) struct TextRangeProjector<'a> {
    source: &'a StringValue,
    chunks: Option<TextChunks<'a>>,
    current: &'a str,
    start: usize,
    end: usize,
    previous_end: usize,
}

impl<'a> TextRangeProjector<'a> {
    pub(crate) fn new(source: &'a StringValue) -> Self {
        Self {
            source,
            chunks: None,
            current: "",
            start: 0,
            end: 0,
            previous_end: 0,
        }
    }

    pub(crate) fn project(&mut self, range: Range<usize>) -> Result<StringValue, TextRangeError> {
        if range.start > range.end
            || range.start < self.previous_end
            || range.end > self.source.len()
        {
            return Err(TextRangeError::InvalidRange);
        }
        if range.start == 0 && range.end == self.source.len() {
            self.previous_end = range.end;
            return Ok(self.source.clone());
        }
        if range.is_empty() && (range.start == 0 || range.start == self.source.len()) {
            self.previous_end = range.end;
            return Ok(StringValue::default());
        }
        self.locate(range.start)?;
        if range.is_empty() {
            self.current
                .get(range.start - self.start..range.start - self.start)
                .ok_or(TextRangeError::InvalidRange)?;
            self.previous_end = range.end;
            return Ok(StringValue::default());
        }
        let mut output = String::new();
        output
            .try_reserve_exact(range.len())
            .map_err(|_| TextRangeError::CapacityExceeded)?;
        let mut position = range.start;
        while position < range.end {
            self.locate(position)?;
            let end = range.end.min(self.end);
            let selected = self
                .current
                .get(position - self.start..end - self.start)
                .ok_or(TextRangeError::InvalidRange)?;
            output.push_str(selected);
            position = end;
        }
        self.previous_end = range.end;
        Ok(output.into())
    }

    fn locate(&mut self, position: usize) -> Result<(), TextRangeError> {
        if position < self.start {
            return Err(TextRangeError::InvalidRange);
        }
        while position >= self.end {
            let chunks = self.chunks.get_or_insert_with(|| self.source.chunks());
            let chunk = chunks.next().ok_or(TextRangeError::InvalidRange)?;
            self.current = chunk;
            self.start = self.end;
            self.end = self
                .end
                .checked_add(chunk.len())
                .ok_or(TextRangeError::InvalidRange)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etas_builtin::text::input::TextInput;

    #[test]
    fn ranges_preserve_unicode_backing_and_own_small_partial_results() {
        let prefix = StringValue::from("  中😀");
        let alias = prefix.clone();
        let source = prefix.concat("e\u{301}  ".into()).unwrap();
        let mut ranges = TextRangeProjector::new(&source);
        let output = ranges.project(0..source.len()).unwrap();
        assert!(
            output
                .chunks()
                .map(str::as_ptr)
                .eq(source.chunks().map(str::as_ptr))
        );
        assert!(source.contiguous().is_none());
        let mut ranges = TextRangeProjector::new(&source);
        for (range, expected) in [
            (0..0, ""),
            (2..5, "中"),
            (5..12, "😀e\u{301}"),
            (12..14, "  "),
        ] {
            let text = ranges.project(range).unwrap();
            assert_eq!(text.into_string(), expected);
        }
        assert!(source.contiguous().is_none());
        assert_eq!(alias.into_string(), "  中😀");
    }

    #[test]
    fn malformed_ranges_reject_out_of_bounds_reordering_and_scalar_splits() {
        let source = StringValue::from("中😀x");
        for range in [
            1..1,
            0..1,
            1..3,
            3..4,
            5..5,
            9..9,
            0..9,
            Range { start: 4, end: 3 },
        ] {
            assert_eq!(
                TextRangeProjector::new(&source).project(range),
                Err(TextRangeError::InvalidRange)
            );
        }
        let mut ranges = TextRangeProjector::new(&source);
        assert_eq!(ranges.project(0..3).unwrap(), "中");
        assert_eq!(ranges.project(0..3), Err(TextRangeError::InvalidRange));
    }
}
