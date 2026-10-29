use super::{Repr, StringValue};

pub struct TextChunks<'a> {
    first: Option<&'a str>,
    last: Option<&'a str>,
    middle: std::vec::IntoIter<&'a str>,
}

impl<'a> TextChunks<'a> {
    pub(super) fn new(text: &'a StringValue) -> Self {
        let mut current = &text.0;
        let mut last = None;
        let mut middle = Vec::new();
        let first = loop {
            match current {
                Repr::Empty => break "",
                Repr::Flat(text) => break text.as_str(),
                Repr::Concat(node) => {
                    if let Some(text) = node.materialized.get() {
                        break text.as_str();
                    }
                    // The common two-chunk view needs no cursor allocation.
                    if last.is_none() {
                        last = Some(node.suffix.as_str());
                    } else {
                        middle.push(node.suffix.as_str());
                    }
                    current = &node.prefix.0;
                }
            }
        };
        Self {
            first: Some(first),
            last,
            middle: middle.into_iter(),
        }
    }
}

impl<'a> Iterator for TextChunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.first
            .take()
            .or_else(|| self.middle.next_back())
            .or_else(|| self.last.take())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = usize::from(self.first.is_some())
            + usize::from(self.last.is_some())
            + self.middle.len();
        (length, Some(length))
    }
}

impl DoubleEndedIterator for TextChunks<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.last
            .take()
            .or_else(|| self.middle.next())
            .or_else(|| self.first.take())
    }
}

impl ExactSizeIterator for TextChunks<'_> {}
impl std::iter::FusedIterator for TextChunks<'_> {}
