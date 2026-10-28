use std::{cell::OnceCell, rc::Rc};

use super::StringValue;

#[derive(Clone, Default)]
pub(super) enum Repr {
    #[default]
    Empty,
    Flat(Rc<String>),
    Concat(Rc<Concat>),
}

pub(super) struct Concat {
    pub prefix: StringValue,
    pub suffix: Rc<String>,
    pub length: usize,
    pub materialized: OnceCell<Rc<String>>,
}

#[derive(Debug)]
pub(crate) struct TextCapacityExceeded;

impl Repr {
    pub fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Flat(text) => text.len(),
            Self::Concat(node) => node.length,
        }
    }

    pub fn text(&self) -> &String {
        static EMPTY: String = String::new();
        match self {
            Self::Empty => &EMPTY,
            Self::Flat(text) => text,
            Self::Concat(node) => node.flat(),
        }
    }
}

impl Concat {
    fn flat(&self) -> &Rc<String> {
        self.materialized
            .get_or_init(|| Rc::new(self.render(self.length)))
    }

    fn render(&self, capacity: usize) -> String {
        let mut suffixes = vec![self.suffix.as_str()];
        let mut prefix = &self.prefix.0;
        let first = loop {
            match prefix {
                Repr::Empty => break "",
                Repr::Flat(text) => break text.as_str(),
                Repr::Concat(node) => {
                    if let Some(text) = node.materialized.get() {
                        break text.as_str();
                    }
                    suffixes.push(node.suffix.as_str());
                    prefix = &node.prefix.0;
                }
            }
        };
        let mut output = String::with_capacity(capacity);
        output.push_str(first);
        for suffix in suffixes.into_iter().rev() {
            output.push_str(suffix);
        }
        output
    }
}

pub(super) fn into_flat(repr: Repr) -> Rc<String> {
    match repr {
        Repr::Empty => Rc::new(String::new()),
        Repr::Flat(text) => text,
        Repr::Concat(node) => node.flat().clone(),
    }
}

/// Reuse a uniquely owned first buffer; shared prefixes are copied directly to
/// the final output capacity, never through a growing sequence of prefixes.
pub(super) fn into_string(mut repr: Repr, length: usize) -> String {
    let mut suffixes = Vec::new();
    let mut output = loop {
        match repr {
            Repr::Empty => break String::new(),
            Repr::Flat(text) => break owned_flat(text, length),
            Repr::Concat(node) => match Rc::try_unwrap(node) {
                Ok(node) => {
                    let Concat {
                        prefix,
                        suffix,
                        materialized,
                        ..
                    } = node;
                    if let Some(text) = materialized.into_inner() {
                        break owned_flat(text, length);
                    }
                    suffixes.push(suffix);
                    repr = prefix.into_repr();
                }
                Err(node) => {
                    break match node.materialized.get() {
                        Some(text) => owned_flat(text.clone(), length),
                        None => node.render(length),
                    };
                }
            },
        }
    };
    output.reserve(length - output.len());
    for suffix in suffixes.into_iter().rev() {
        output.push_str(&suffix);
    }
    output
}

fn owned_flat(text: Rc<String>, capacity: usize) -> String {
    match Rc::try_unwrap(text) {
        Ok(text) => text,
        Err(text) => {
            let mut output = String::with_capacity(capacity);
            output.push_str(&text);
            output
        }
    }
}

/// Each concatenation has one prefix edge. Detach it before dropping the node
/// so ordinary value/snapshot release never recurses down a long append chain.
pub(super) fn release(mut repr: Repr) {
    while let Repr::Concat(node) = repr {
        match Rc::try_unwrap(node) {
            Ok(mut node) => repr = std::mem::take(&mut node.prefix.0),
            Err(_) => break,
        }
    }
}
