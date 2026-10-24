use super::{CallTarget, Rc};

pub(super) enum Edge {
    Shared(Rc<CallTarget>),
    Children(Rc<Vec<CallTarget>>),
    Owned(CallTarget),
}

pub(super) fn release(mut next: Option<Edge>) {
    let mut pending: Vec<std::vec::IntoIter<CallTarget>> = Vec::new();
    loop {
        if let Some(edge) = next.take() {
            let mut node = match edge {
                Edge::Owned(node) => node,
                Edge::Shared(node) => match Rc::try_unwrap(node) {
                    Ok(node) => node,
                    Err(_) => continue,
                },
                Edge::Children(children) => {
                    if let Ok(children) = Rc::try_unwrap(children) {
                        let mut children = children.into_iter();
                        if let Some(first) = children.next() {
                            next = Some(Edge::Owned(first));
                            if children.len() > 0 {
                                pending.push(children);
                            }
                        }
                    }
                    continue;
                }
            };
            match &mut node {
                CallTarget::Specialized { target, .. } | CallTarget::Limited { target, .. } => {
                    next = target.0.take().map(Edge::Shared);
                }
                CallTarget::Composed(children) => {
                    next = children.0.take().map(Edge::Children);
                }
                _ => {}
            }
            continue;
        }
        let Some(siblings) = pending.last_mut() else {
            return;
        };
        if let Some(child) = siblings.next() {
            next = Some(Edge::Owned(child));
            if siblings.len() == 0 {
                pending.pop();
            }
        } else {
            pending.pop();
        }
    }
}
