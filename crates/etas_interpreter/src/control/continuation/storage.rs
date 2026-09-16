use super::Continuation;
use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
};

/// Immutable shared control structure. Mutable runtime locals remain owned by Frame;
/// checkpoint capture still freezes them separately.
#[derive(Clone, Debug)]
pub struct ContinuationLink(Rc<Node>);

#[derive(Clone, Debug)]
struct Node(Continuation);

impl ContinuationLink {
    pub fn into_value(self) -> Continuation {
        match Rc::try_unwrap(self.0) {
            Ok(mut node) => std::mem::replace(&mut node.0, Continuation::BlockValue),
            Err(node) => node.0.clone(),
        }
    }

    fn into_unique(self) -> Option<Continuation> {
        Rc::try_unwrap(self.0)
            .ok()
            .map(|mut node| std::mem::replace(&mut node.0, Continuation::BlockValue))
    }
}

impl From<Continuation> for ContinuationLink {
    fn from(value: Continuation) -> Self {
        Self(Rc::new(Node(value)))
    }
}

impl Deref for ContinuationLink {
    type Target = Continuation;
    fn deref(&self) -> &Continuation {
        &self.0.0
    }
}

impl DerefMut for ContinuationLink {
    fn deref_mut(&mut self) -> &mut Continuation {
        &mut Rc::make_mut(&mut self.0).0
    }
}

impl AsRef<Continuation> for ContinuationLink {
    fn as_ref(&self) -> &Continuation {
        self
    }
}

impl AsMut<Continuation> for ContinuationLink {
    fn as_mut(&mut self) -> &mut Continuation {
        self
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let mut next = Some(std::mem::replace(&mut self.0, Continuation::BlockValue));
        let mut pending = Vec::new();
        while let Some(node) = next.take().or_else(|| pending.pop()) {
            match node {
                Continuation::CallBoundary { outer } | Continuation::HandlerDispatch { outer } => {
                    next = outer.into_unique();
                }
                Continuation::RestoreModelPolicy { inner, .. }
                | Continuation::ScopedModelPolicy { inner, .. }
                | Continuation::HandleBoundary { inner, .. } => {
                    next = inner.into_unique();
                }
                Continuation::Chain { inner, outer } => {
                    next = inner.into_unique();
                    if let Some(outer) = outer.into_unique() {
                        if next.is_none() {
                            next = Some(outer);
                        } else {
                            pending.push(outer);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
