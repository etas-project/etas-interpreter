use super::{
    ActiveHandlerArmRecord, BTreeSet, ContinuationSnapshot, HandlerScopeId, LocalsSnapshot,
    SnapshotValidator,
};

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
    next: Option<Visit<'a>>,
    pending: Vec<Visit<'a>>,
}

impl<'a> ContinuationWalk<'a> {
    fn new(root: &'a ContinuationSnapshot) -> Self {
        Self {
            next: Some(Visit::Node(root)),
            pending: Vec::new(),
        }
    }
}

impl<'a> Iterator for ContinuationWalk<'a> {
    type Item = Visit<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.next.take().or_else(|| self.pending.pop())?;
        if let Visit::Node(node) = event {
            match node {
                ContinuationSnapshot::HandleBoundary {
                    scope_id,
                    inner,
                    handlers,
                    frame,
                    ..
                } => {
                    self.pending.push(Visit::LeaveHandler {
                        scope_id: *scope_id,
                        handlers,
                        frame,
                    });
                    self.next = Some(Visit::Node(inner));
                }
                ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::CallBoundary { outer: inner }
                | ContinuationSnapshot::HandlerDispatch { outer: inner }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => {
                    self.next = Some(Visit::Node(inner))
                }
                ContinuationSnapshot::Chain { inner, outer } => {
                    self.pending.push(Visit::Node(outer));
                    self.next = Some(Visit::Node(inner));
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
