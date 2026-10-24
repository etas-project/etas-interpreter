use std::{
    ops::{Deref, DerefMut},
    ptr::NonNull,
    rc::Rc,
};

use super::ContinuationSnapshot;

// Captured continuations contain durable snapshot data, not live evaluator frames.
// Empty links exist only while a consuming operation or Drop detaches an edge.
#[derive(Clone, Debug)]
pub(crate) struct ContinuationSnapshotLink(Option<Rc<ContinuationSnapshot>>);

impl ContinuationSnapshotLink {
    pub(crate) fn shared_identity(&self) -> Option<NonNull<ContinuationSnapshot>> {
        let node = self.0.as_ref().expect("live continuation snapshot link");
        (Rc::strong_count(node) > 1).then(|| NonNull::from(node.as_ref()))
    }

    pub(crate) fn new(value: ContinuationSnapshot) -> Self {
        Self(Some(Rc::new(value)))
    }

    pub(crate) fn into_value(mut self) -> ContinuationSnapshot {
        Rc::unwrap_or_clone(self.0.take().expect("live continuation snapshot link"))
    }
}

impl From<ContinuationSnapshot> for ContinuationSnapshotLink {
    fn from(value: ContinuationSnapshot) -> Self {
        Self::new(value)
    }
}

impl Deref for ContinuationSnapshotLink {
    type Target = ContinuationSnapshot;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().expect("live continuation snapshot link")
    }
}

impl DerefMut for ContinuationSnapshotLink {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Rc::make_mut(self.0.as_mut().expect("live continuation snapshot link"))
    }
}

impl AsRef<ContinuationSnapshot> for ContinuationSnapshotLink {
    fn as_ref(&self) -> &ContinuationSnapshot {
        self
    }
}

impl AsMut<ContinuationSnapshot> for ContinuationSnapshotLink {
    fn as_mut(&mut self) -> &mut ContinuationSnapshot {
        self
    }
}

impl Drop for ContinuationSnapshotLink {
    fn drop(&mut self) {
        let mut next = self.0.take();
        let mut pending = Vec::new();
        while let Some(node) = next.take().or_else(|| pending.pop()) {
            let Ok(mut node) = Rc::try_unwrap(node) else {
                continue;
            };
            match &mut node {
                ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::HandleBoundary { inner, .. }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => next = inner.0.take(),
                ContinuationSnapshot::CallBoundary { outer }
                | ContinuationSnapshot::HandlerDispatch { outer } => next = outer.0.take(),
                ContinuationSnapshot::Chain { inner, outer } => {
                    next = inner.0.take();
                    if let Some(outer) = outer.0.take() {
                        pending.push(outer);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
