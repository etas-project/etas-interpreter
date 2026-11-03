use std::{cell::OnceCell, rc::Rc};

use super::{StringValue, chunks::TextLeaves};

#[derive(Clone, Default)]
pub(super) enum Repr {
    #[default]
    Empty,
    Flat(Rc<String>),
    Concat(Rc<Concat>),
}

pub(super) struct Concat {
    pub prefix: StringValue,
    pub suffix: StringValue,
    pub length: usize,
    pub leaves: usize,
    pub materialized: OnceCell<Rc<String>>,
}

#[derive(Debug)]
pub(crate) struct TextCapacityExceeded;

impl Repr {
    pub fn leaves(&self) -> usize {
        match self {
            Self::Empty | Self::Flat(_) => 1,
            Self::Concat(node) => node.leaves,
        }
    }

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
        let mut output = String::with_capacity(capacity);
        self.append_to(&mut output);
        output
    }

    fn append_to(&self, output: &mut String) {
        if let Some(text) = self.materialized.get() {
            output.push_str(text);
            return;
        }
        for chunk in TextLeaves::new(&self.prefix.0).chain(TextLeaves::new(&self.suffix.0)) {
            output.push_str(chunk);
        }
    }
}

/// Reuse a uniquely owned first buffer; shared prefixes are copied directly to
/// the final output capacity, never through a growing sequence of prefixes.
pub(super) fn into_string(repr: Repr, length: usize) -> String {
    let mut node = match repr {
        Repr::Empty => return String::new(),
        Repr::Flat(text) => return owned_flat(text, length),
        Repr::Concat(node) => node,
    };
    if let Some(unique) = Rc::get_mut(&mut node)
        && let Some(text) = unique.materialized.get_mut().and_then(Rc::get_mut)
    {
        let output = std::mem::take(text);
        release(Repr::Concat(node));
        return output;
    }
    let mut output = if node.materialized.get().is_none() {
        Rc::get_mut(&mut node)
            .and_then(|node| take_unique_prefix(&mut node.prefix.0))
            .unwrap_or_default()
    } else {
        String::new()
    };
    output.reserve(length - output.len());
    node.append_to(&mut output);
    release(Repr::Concat(node));
    output
}

fn take_unique_prefix(mut repr: &mut Repr) -> Option<String> {
    // This tree is private to the consuming conversion. Stop at any alias or
    // cached subtree so moving a leaf cannot invalidate an observable value.
    loop {
        match repr {
            Repr::Empty => return None,
            Repr::Flat(text) => return Rc::get_mut(text).map(std::mem::take),
            Repr::Concat(node) => {
                let node = Rc::get_mut(node)?;
                if node.materialized.get().is_some() {
                    return None;
                }
                repr = &mut node.prefix.0;
            }
        }
    }
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

/// Detach both children before dropping a unique node. Only branching subtrees
/// need a worklist; left- and right-deep chains release without scratch space.
pub(super) fn release(mut repr: Repr) {
    let mut pending = Vec::new();
    loop {
        if let Repr::Concat(node) = repr
            && let Ok(mut node) = Rc::try_unwrap(node)
        {
            let prefix = std::mem::take(&mut node.prefix.0);
            let suffix = std::mem::take(&mut node.suffix.0);
            match (prefix, suffix) {
                (prefix @ Repr::Concat(_), suffix @ Repr::Concat(_)) => {
                    pending.push(suffix);
                    repr = prefix;
                    continue;
                }
                (child @ Repr::Concat(_), _) | (_, child @ Repr::Concat(_)) => {
                    repr = child;
                    continue;
                }
                _ => {}
            }
        }
        let Some(next) = pending.pop() else { break };
        repr = next;
    }
}
