pub(super) use crate::value::graph::FrameDiscovery as Discovery;
use std::collections::HashSet;

pub(super) fn snapshot_frames(
    value: &crate::orchestration::ValueSnapshot,
    output: &mut Vec<(crate::orchestration::LocalsSnapshot, bool)>,
) {
    use crate::orchestration::{CallTargetSnapshot as Target, ValueSnapshot};
    let mut nodes = HashSet::new();
    let mut tables = HashSet::new();
    let mut frames = HashSet::new();
    for value in value.walk_unique() {
        let ValueSnapshot::Callable(root) = value else {
            continue;
        };
        let mut pending = vec![root];
        while let Some(target) = pending.pop() {
            match target {
                Target::Lambda { captured, .. } => {
                    if frames.insert((captured.id, std::rc::Rc::as_ptr(&captured.locals))) {
                        output.push((captured.clone(), false));
                    }
                }
                Target::Specialized { target, .. } | Target::Limited { target, .. } => {
                    if target.shared_identity().is_none_or(|key| nodes.insert(key)) {
                        pending.push(target);
                    }
                }
                Target::Composed(targets) => {
                    if targets
                        .shared_identity()
                        .is_none_or(|key| tables.insert(key))
                    {
                        pending.extend(targets.iter().rev());
                    }
                }
                // Scoped targets are checked using their native program's validator.
                _ => (),
            }
        }
    }
}
