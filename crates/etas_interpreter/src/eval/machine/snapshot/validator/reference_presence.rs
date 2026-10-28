use crate::orchestration::{
    CallTargetSnapshot as Target, CallTargetSnapshotChildren as Children,
    CallTargetSnapshotLink as Link,
};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct ReferencePresence {
    // Own immutable edges so COW edits cannot reuse a cached pointer identity.
    nodes: HashMap<*const Target, (Link, bool)>,
    tables: HashMap<*const Vec<Target>, (Children, bool)>,
}

enum Task<'a> {
    Visit(&'a Target),
    Node(*const Target, &'a Link, usize),
    Table(*const Vec<Target>, &'a Children, usize),
}

impl ReferencePresence {
    fn cached(&self, target: &Target) -> Option<bool> {
        match target {
            Target::AgentRevision(_) | Target::Scoped { .. } => Some(true),
            Target::Limited { target, .. } | Target::Specialized { target, .. } => target
                .shared_identity()
                .and_then(|key| self.nodes.get(&key))
                .map(|(_, has)| *has),
            Target::Composed(targets) => targets
                .shared_identity()
                .and_then(|key| self.tables.get(&key))
                .map(|(_, has)| *has),
            _ => Some(false),
        }
    }

    pub(super) fn contains(&mut self, root: &Target) -> bool {
        if let Some(has) = self.cached(root) {
            return has;
        }
        let mut found = 0;
        let mut pending = vec![Task::Visit(root)];
        while let Some(task) = pending.pop() {
            match task {
                Task::Visit(target) => {
                    if let Some(has) = self.cached(target) {
                        found += usize::from(has);
                        continue;
                    }
                    match target {
                        Target::Limited { target, .. } | Target::Specialized { target, .. } => {
                            if let Some(key) = target.shared_identity() {
                                pending.push(Task::Node(key, target, found));
                            }
                            pending.push(Task::Visit(target));
                        }
                        Target::Composed(targets) => {
                            if let Some(key) = targets.shared_identity() {
                                pending.push(Task::Table(key, targets, found));
                            }
                            pending.extend(targets.iter().rev().map(Task::Visit));
                        }
                        _ => {}
                    }
                }
                Task::Node(key, owner, before) => {
                    self.nodes.insert(key, (owner.clone(), found != before));
                }
                Task::Table(key, owner, before) => {
                    self.tables.insert(key, (owner.clone(), found != before));
                }
            }
        }
        found != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_presence_keeps_old_edges_alive_across_cow_mutation() {
        let mut target = Target::Composed(vec![Target::FlowItem(etas_hir::HirItemId(0))].into());
        let original = target.clone();
        let mut cache = ReferencePresence::default();
        assert!(!cache.contains(&target));
        let Target::Composed(children) = &mut target else {
            unreachable!()
        };
        children[0] = Target::Scoped {
            metadata: Box::new(crate::orchestration::ScopedCallableMetadata {
                native_program: crate::orchestration::CheckpointProgramReference::Source,
                native_type: etas_types::TypeId(0),
                projections: vec![],
            }),
            target: Target::FlowItem(etas_hir::HirItemId(0)).into(),
        };
        assert!(cache.contains(&target));
        assert!(!cache.contains(&original));
    }

    #[test]
    fn shared_reference_presence_is_stack_safe_and_cached_by_graph_identity() {
        let mut target = Target::FlowItem(etas_hir::HirItemId(0));
        for _ in 0..30_000 {
            target = Target::Composed(vec![target.clone(), target].into());
        }
        let retained = target.clone();
        let mut cache = ReferencePresence::default();
        assert!(!cache.contains(&target));
        let (_, cost) = crate::testing::allocation::measure(|| {
            for _ in 0..1000 {
                assert!(!cache.contains(&retained));
            }
        });
        assert_eq!(cost.count, 0, "revisited a shared graph: {cost:?}");
    }
}
