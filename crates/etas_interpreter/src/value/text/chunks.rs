use std::{collections::VecDeque, ops::Range, rc::Rc};

use super::{Repr, StringValue};

pub struct TextChunks<'a> {
    flat: Option<&'a str>,
    first: Option<&'a Repr>,
    last: Option<&'a Repr>,
    middle: VecDeque<&'a Repr>,
    remaining: usize,
}

impl<'a> TextChunks<'a> {
    pub(super) fn new(text: &'a StringValue) -> Self {
        let flat = match &text.0 {
            Repr::Empty => Some(""),
            Repr::Flat(text) => Some(text.as_str()),
            Repr::Concat(node) => node.materialized.get().map(|text| text.as_str()),
        };
        Self {
            flat,
            first: flat.is_none().then_some(&text.0),
            last: None,
            middle: VecDeque::new(),
            remaining: if flat.is_some() { 1 } else { text.0.leaves() },
        }
    }

    fn push_front(&mut self, node: &'a Repr) {
        if let Some(previous) = self.first.replace(node) {
            if self.last.is_none() && self.middle.is_empty() {
                self.last = Some(previous);
            } else {
                self.middle.push_front(previous);
            }
        }
    }

    fn push_back(&mut self, node: &'a Repr) {
        if let Some(previous) = self.last.replace(node) {
            if self.first.is_none() && self.middle.is_empty() {
                self.first = Some(previous);
            } else {
                self.middle.push_back(previous);
            }
        }
    }
}

impl<'a> Iterator for TextChunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(flat) = self.flat.take() {
            self.remaining = 0;
            return Some(flat);
        }
        loop {
            let node = self
                .first
                .take()
                .or_else(|| self.middle.pop_front())
                .or_else(|| self.last.take())?;
            let text = match node {
                Repr::Empty => "",
                Repr::Flat(text) => text.as_str(),
                Repr::Concat(node) => {
                    // Below the root, use immutable leaves even if an alias fills
                    // a cache mid-iteration; remaining must stay exact.
                    self.push_front(&node.suffix.0);
                    self.push_front(&node.prefix.0);
                    continue;
                }
            };
            self.remaining -= 1;
            return Some(text);
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for TextChunks<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if let Some(flat) = self.flat.take() {
            self.remaining = 0;
            return Some(flat);
        }
        loop {
            let node = self
                .last
                .take()
                .or_else(|| self.middle.pop_back())
                .or_else(|| self.first.take())?;
            let text = match node {
                Repr::Empty => "",
                Repr::Flat(text) => text.as_str(),
                Repr::Concat(node) => {
                    self.push_back(&node.prefix.0);
                    self.push_back(&node.suffix.0);
                    continue;
                }
            };
            self.remaining -= 1;
            return Some(text);
        }
    }
}

impl ExactSizeIterator for TextChunks<'_> {}
impl std::iter::FusedIterator for TextChunks<'_> {}

#[derive(Clone, Debug)]
pub(crate) struct TextPiece {
    text: Rc<String>,
    range: Range<usize>,
}

impl TextPiece {
    pub(crate) fn as_str(&self) -> &str {
        &self.text[self.range.clone()]
    }
}

#[derive(Clone, Default)]
pub(crate) struct OwnedTextCursor {
    next: Option<Repr>,
    pending: Vec<Repr>,
    current: Option<Rc<String>>,
    current_offset: usize,
    position: usize,
    length: usize,
}

impl std::fmt::Debug for OwnedTextCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OwnedTextCursor")
            .field("position", &self.position)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl OwnedTextCursor {
    pub(crate) fn new(text: &StringValue) -> Self {
        Self {
            next: Some(text.0.clone()),
            pending: Vec::new(),
            current: None,
            current_offset: 0,
            position: 0,
            length: text.len(),
        }
    }

    pub(crate) fn at(text: &StringValue, position: usize) -> Result<Self, String> {
        if position > text.len() {
            return Err("text cursor position exceeds source length".into());
        }
        let mut cursor = Self::new(text);
        while cursor.position < position {
            let piece = cursor
                .next_piece()
                .ok_or("text cursor position is not reachable")?;
            let remaining = position - cursor.position;
            let consumed = remaining.min(piece.as_str().len());
            if !piece.as_str().is_char_boundary(consumed) {
                return Err("text cursor position is not a scalar boundary".into());
            }
            cursor.advance(consumed);
        }
        Ok(cursor)
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.position == self.length
    }

    pub(crate) fn next_piece(&mut self) -> Option<TextPiece> {
        loop {
            if let Some(text) = &self.current
                && self.current_offset < text.len()
            {
                return Some(TextPiece {
                    text: text.clone(),
                    range: self.current_offset..text.len(),
                });
            }
            self.current = None;
            self.current_offset = 0;
            let mut current = self.next.take().or_else(|| self.pending.pop())?;
            loop {
                match current {
                    Repr::Empty => break,
                    Repr::Flat(text) => {
                        self.current = Some(text);
                        break;
                    }
                    Repr::Concat(node) => {
                        if let Some(text) = node.materialized.get() {
                            self.current = Some(text.clone());
                            break;
                        }
                        self.pending.push(node.suffix.0.clone());
                        current = node.prefix.0.clone();
                    }
                }
            }
        }
    }

    pub(crate) fn advance(&mut self, bytes: usize) {
        self.current_offset += bytes;
        self.position += bytes;
    }
}

/// Forward-only consumers need pending tree edges, not a complete leaf index.
pub(super) struct TextLeaves<'a> {
    next: Option<&'a Repr>,
    pending: Vec<&'a Repr>,
}

impl<'a> TextLeaves<'a> {
    pub(super) fn new(repr: &'a Repr) -> Self {
        Self {
            next: Some(repr),
            pending: Vec::new(),
        }
    }
}

impl<'a> Iterator for TextLeaves<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let mut current = self.next.take().or_else(|| self.pending.pop())?;
        loop {
            match current {
                Repr::Empty => return Some(""),
                Repr::Flat(text) => return Some(text.as_str()),
                Repr::Concat(node) => {
                    if let Some(text) = node.materialized.get() {
                        return Some(text.as_str());
                    }
                    // Keep one pending sibling inline for a two-chunk input.
                    if let Some(sibling) = self.next.replace(&node.suffix.0) {
                        self.pending.push(sibling);
                    }
                    current = &node.prefix.0;
                }
            }
        }
    }
}
