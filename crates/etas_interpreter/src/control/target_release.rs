use super::CallTarget;

// Used by partially constructed target owners. Consume wrapper edges before
// dropping their storage, without cloning targets or recursing through Box/Vec.
pub(crate) fn release_call_targets(targets: impl IntoIterator<Item = CallTarget>) {
    let mut pending = Vec::new();
    for root in targets {
        let mut next = Some(root);
        loop {
            if let Some(node) = next.take() {
                match node {
                    CallTarget::Specialized { target, .. } | CallTarget::Limited { target, .. } => {
                        next = Some(*target);
                    }
                    CallTarget::Composed(targets) => {
                        let mut children = targets.into_iter();
                        next = children.next();
                        if children.len() > 0 {
                            pending.push(children);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let Some(siblings) = pending.last_mut() else {
                break;
            };
            next = siblings.next();
            if siblings.len() == 0 {
                pending.pop();
            }
        }
    }
}
