use super::{
    ActiveHandlerArmRecord, BTreeSet, ContinuationSnapshot, HandlerScopeId, LocalsSnapshot,
    SnapshotValidator,
};
use crate::orchestration::ContinuationSnapshotLink;
use std::{collections::HashSet, ptr::NonNull};

#[derive(Clone, Copy)]
enum Visit<'a> {
    Node(&'a ContinuationSnapshot),
    LeaveHandler {
        scope_id: HandlerScopeId,
        handlers: &'a [ActiveHandlerArmRecord],
        frame: &'a LocalsSnapshot,
    },
}

struct ContinuationWalk<'a> {
    next: Option<Task<'a>>,
    pending: Vec<Task<'a>>,
    scope_free: HashSet<NonNull<ContinuationSnapshot>>,
    scopes_seen: usize,
}

enum Task<'a> {
    Visit(Visit<'a>),
    Link(&'a ContinuationSnapshotLink),
    CompleteShared {
        identity: NonNull<ContinuationSnapshot>,
        scopes_before: usize,
    },
}

impl<'a> ContinuationWalk<'a> {
    fn new(root: &'a ContinuationSnapshot) -> Self {
        Self {
            next: Some(Task::Visit(Visit::Node(root))),
            pending: Vec::new(),
            scope_free: HashSet::new(),
            scopes_seen: 0,
        }
    }
}

impl<'a> Iterator for ContinuationWalk<'a> {
    type Item = Visit<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = loop {
            match self.next.take().or_else(|| self.pending.pop())? {
                Task::Visit(event) => break event,
                Task::Link(link) => {
                    if let Some(identity) = link.shared_identity() {
                        if self.scope_free.contains(&identity) {
                            continue;
                        }
                        self.pending.push(Task::CompleteShared {
                            identity,
                            scopes_before: self.scopes_seen,
                        });
                    }
                    self.next = Some(Task::Visit(Visit::Node(link)));
                }
                Task::CompleteShared {
                    identity,
                    scopes_before,
                } => {
                    // Repeated scope-bearing subtrees must still visit their
                    // boundaries so duplicate IDs cannot bypass validation.
                    if self.scopes_seen == scopes_before {
                        self.scope_free.insert(identity);
                    }
                }
            }
        };
        if let Visit::Node(node) = event {
            match node {
                ContinuationSnapshot::HandleBoundary {
                    scope_id,
                    inner,
                    handlers,
                    frame,
                    ..
                } => {
                    self.scopes_seen += 1;
                    self.pending.push(Task::Visit(Visit::LeaveHandler {
                        scope_id: *scope_id,
                        handlers,
                        frame,
                    }));
                    self.next = Some(Task::Link(inner));
                }
                ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::CallBoundary { outer: inner }
                | ContinuationSnapshot::HandlerDispatch { outer: inner }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => {
                    self.next = Some(Task::Link(inner))
                }
                ContinuationSnapshot::Chain { inner, outer } => {
                    self.pending.push(Task::Link(outer));
                    self.next = Some(Task::Link(inner));
                }
                _ => {}
            }
        }
        Some(event)
    }
}

impl SnapshotValidator<'_> {
    pub(super) fn continuation(
        &self,
        continuation: &ContinuationSnapshot,
        context: &str,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        for event in ContinuationWalk::new(continuation) {
            match event {
                Visit::Node(node) => self.continuation_node(node, context, boundary_scopes)?,
                Visit::LeaveHandler {
                    handlers, frame, ..
                } => {
                    // Preserve the original validation order: enter scope, validate
                    // its inner continuation, then check arm and frame metadata.
                    for (index, handler) in handlers.iter().enumerate() {
                        self.handler_arm(handler, &format!("{context} handler arm {index}"))?;
                    }
                    self.frame(frame, context)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn continuation_scope_unwind_order(
        continuation: &ContinuationSnapshot,
        scopes: &mut Vec<HandlerScopeId>,
    ) {
        for event in ContinuationWalk::new(continuation) {
            if let Visit::LeaveHandler { scope_id, .. } = event {
                scopes.push(scope_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_scope_free_continuations_are_visited_once() {
        for depth in [10, 1000, 4000, 30_000] {
            let mut root = ContinuationSnapshot::Return;
            for _ in 0..depth {
                let child: ContinuationSnapshotLink = root.into();
                root = ContinuationSnapshot::Chain {
                    inner: child.clone(),
                    outer: child,
                };
            }
            assert_eq!(ContinuationWalk::new(&root).count(), depth + 1);
        }
    }
}
