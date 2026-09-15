use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
};

use super::CallTargetSnapshot;

mod equality;
mod release;

// Only durable captured data is shared, never live evaluator Frames. None is
// private to edge detachment during consuming operations and destruction.
#[derive(Clone, Debug)]
pub(crate) struct CallTargetSnapshotLink(Option<Rc<CallTargetSnapshot>>);

#[derive(Clone, Debug)]
pub(crate) struct CallTargetSnapshotChildren(Option<Rc<Vec<CallTargetSnapshot>>>);

impl CallTargetSnapshotLink {
    pub(crate) fn into_value(mut self) -> CallTargetSnapshot {
        Rc::unwrap_or_clone(self.0.take().expect("live call target snapshot link"))
    }
}

impl CallTargetSnapshotChildren {
    pub(crate) fn into_values(mut self) -> Vec<CallTargetSnapshot> {
        Rc::unwrap_or_clone(self.0.take().expect("live call target snapshot children"))
    }
}

impl From<CallTargetSnapshot> for CallTargetSnapshotLink {
    fn from(value: CallTargetSnapshot) -> Self {
        Self(Some(Rc::new(value)))
    }
}

impl From<Vec<CallTargetSnapshot>> for CallTargetSnapshotChildren {
    fn from(values: Vec<CallTargetSnapshot>) -> Self {
        Self(Some(Rc::new(values)))
    }
}

impl Deref for CallTargetSnapshotLink {
    type Target = CallTargetSnapshot;
    fn deref(&self) -> &Self::Target {
        self.0.as_deref().expect("live call target snapshot link")
    }
}

impl DerefMut for CallTargetSnapshotLink {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Rc::make_mut(self.0.as_mut().expect("live call target snapshot link"))
    }
}

impl Deref for CallTargetSnapshotChildren {
    type Target = [CallTargetSnapshot];
    fn deref(&self) -> &Self::Target {
        self.0
            .as_deref()
            .expect("live call target snapshot children")
    }
}

impl DerefMut for CallTargetSnapshotChildren {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Rc::make_mut(self.0.as_mut().expect("live call target snapshot children")).as_mut_slice()
    }
}

impl PartialEq for CallTargetSnapshotLink {
    fn eq(&self, other: &Self) -> bool {
        equality::equal(std::slice::from_ref(self), std::slice::from_ref(other))
    }
}

impl PartialEq for CallTargetSnapshotChildren {
    fn eq(&self, other: &Self) -> bool {
        equality::equal(self, other)
    }
}

impl Drop for CallTargetSnapshotLink {
    fn drop(&mut self) {
        release::release(self.0.take().map(release::Edge::Shared));
    }
}

impl Drop for CallTargetSnapshotChildren {
    fn drop(&mut self) {
        release::release(self.0.take().map(release::Edge::Children));
    }
}

#[cfg(test)]
mod tests;
