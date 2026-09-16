use std::{ops::Deref, rc::Rc};

use etas_host::session::SessionPublishedContext;

/// An immutable selected publication, independent of later backend publications.
/// Runtime aliases and in-memory snapshots share the same read-only content.
#[derive(Clone, Debug)]
pub struct PublishedContextValue(Rc<SessionPublishedContext>);

impl From<SessionPublishedContext> for PublishedContextValue {
    fn from(context: SessionPublishedContext) -> Self {
        Self(Rc::new(context))
    }
}

impl Deref for PublishedContextValue {
    type Target = SessionPublishedContext;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for PublishedContextValue {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Eq for PublishedContextValue {}
