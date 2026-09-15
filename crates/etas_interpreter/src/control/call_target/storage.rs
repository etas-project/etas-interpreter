use super::CallTarget;
use std::{
    ops::{Deref, DerefMut},
    rc::Rc,
};

mod equality;
mod release;

// Share target structure, not a durable capture. Frame aliasing remains Frame's
// responsibility; checkpoint capture still converts it into immutable locals.
#[derive(Clone, Debug)]
pub struct CallTargetLink(Option<Rc<CallTarget>>);

#[derive(Clone, Debug)]
pub struct CallTargetChildren(Option<Rc<Vec<CallTarget>>>);

impl CallTargetLink {
    pub(crate) fn shared_identity(&self) -> Option<*const CallTarget> {
        let node = self.0.as_ref().expect("live call target link");
        (Rc::strong_count(node) > 1).then_some(Rc::as_ptr(node))
    }
    pub fn into_value(mut self) -> CallTarget {
        Rc::unwrap_or_clone(self.0.take().expect("live call target link"))
    }
}

impl CallTargetChildren {
    pub(crate) fn shared_identity(&self) -> Option<*const Vec<CallTarget>> {
        let nodes = self.0.as_ref().expect("live call target children");
        (Rc::strong_count(nodes) > 1).then_some(Rc::as_ptr(nodes))
    }
    pub fn into_values(mut self) -> Vec<CallTarget> {
        Rc::unwrap_or_clone(self.0.take().expect("live call target children"))
    }
}

impl From<CallTarget> for CallTargetLink {
    fn from(value: CallTarget) -> Self {
        Self(Some(Rc::new(value)))
    }
}

impl From<Vec<CallTarget>> for CallTargetChildren {
    fn from(values: Vec<CallTarget>) -> Self {
        Self(Some(Rc::new(values)))
    }
}

impl Deref for CallTargetLink {
    type Target = CallTarget;
    fn deref(&self) -> &CallTarget {
        self.0.as_deref().expect("live call target link")
    }
}

impl DerefMut for CallTargetLink {
    fn deref_mut(&mut self) -> &mut CallTarget {
        Rc::make_mut(self.0.as_mut().expect("live call target link"))
    }
}

impl Deref for CallTargetChildren {
    type Target = [CallTarget];
    fn deref(&self) -> &[CallTarget] {
        self.0.as_deref().expect("live call target children")
    }
}

impl DerefMut for CallTargetChildren {
    fn deref_mut(&mut self) -> &mut [CallTarget] {
        Rc::make_mut(self.0.as_mut().expect("live call target children")).as_mut_slice()
    }
}

impl PartialEq for CallTargetLink {
    fn eq(&self, other: &Self) -> bool {
        equality::equal(std::slice::from_ref(self), std::slice::from_ref(other))
    }
}
impl Eq for CallTargetLink {}

impl PartialEq for CallTargetChildren {
    fn eq(&self, other: &Self) -> bool {
        equality::equal(self, other)
    }
}
impl Eq for CallTargetChildren {}

impl Drop for CallTargetLink {
    fn drop(&mut self) {
        release::release(self.0.take().map(release::Edge::Shared));
    }
}

impl Drop for CallTargetChildren {
    fn drop(&mut self) {
        release::release(self.0.take().map(release::Edge::Children));
    }
}
