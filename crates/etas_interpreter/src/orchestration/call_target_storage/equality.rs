use super::{CallTargetSnapshot, Rc};
use std::collections::HashSet;

// Acyclic snapshots may contain shared subgraphs. Memoize only aliased edges;
// pointer equality alone is not proof of value equality (for example, NaN).
fn already_seen<T>(a: &Rc<T>, b: &Rc<T>, seen: &mut HashSet<(*const T, *const T)>) -> bool {
    (Rc::strong_count(a) > 1 || Rc::strong_count(b) > 1)
        && !seen.insert((Rc::as_ptr(a), Rc::as_ptr(b)))
}

pub(super) fn equal(left: &[CallTargetSnapshot], right: &[CallTargetSnapshot]) -> bool {
    type Siblings<'a> = std::iter::Zip<
        std::slice::Iter<'a, CallTargetSnapshot>,
        std::slice::Iter<'a, CallTargetSnapshot>,
    >;
    if left.len() != right.len() {
        return false;
    }
    let mut initial = left.iter().zip(right);
    let mut next = initial.next();
    let mut pending: Vec<Siblings<'_>> = Vec::new();
    if initial.len() > 0 {
        pending.push(initial);
    }
    let mut seen_nodes = HashSet::new();
    let mut seen_children = HashSet::new();
    loop {
        if let Some((left, right)) = next.take() {
            use CallTargetSnapshot as T;
            match (left, right) {
                (
                    T::Specialized {
                        target: a,
                        type_bindings: ab,
                    },
                    T::Specialized {
                        target: b,
                        type_bindings: bb,
                    },
                ) => {
                    if ab != bb {
                        return false;
                    }
                    if !already_seen(
                        a.0.as_ref().expect("live call target snapshot link"),
                        b.0.as_ref().expect("live call target snapshot link"),
                        &mut seen_nodes,
                    ) {
                        next = Some((a, b));
                    }
                }
                (
                    T::Limited {
                        target: a,
                        limits: al,
                    },
                    T::Limited {
                        target: b,
                        limits: bl,
                    },
                ) => {
                    if al != bl {
                        return false;
                    }
                    if !already_seen(
                        a.0.as_ref().expect("live call target snapshot link"),
                        b.0.as_ref().expect("live call target snapshot link"),
                        &mut seen_nodes,
                    ) {
                        next = Some((a, b));
                    }
                }
                (T::Composed(a), T::Composed(b)) => {
                    if a.len() != b.len() {
                        return false;
                    }
                    if !already_seen(
                        a.0.as_ref().expect("live call target snapshot children"),
                        b.0.as_ref().expect("live call target snapshot children"),
                        &mut seen_children,
                    ) {
                        let mut children = a.iter().zip(b.iter());
                        if let Some((a, b)) = children.next() {
                            if children.len() > 0 {
                                pending.push(children);
                            }
                            next = Some((a, b));
                        }
                    }
                }
                (T::Specialized { .. } | T::Limited { .. } | T::Composed(_), _)
                | (_, T::Specialized { .. } | T::Limited { .. } | T::Composed(_)) => return false,
                (a, b) => {
                    if a != b {
                        return false;
                    }
                }
            }
            continue;
        }
        match pending.pop() {
            Some(mut siblings) => {
                if let Some((a, b)) = siblings.next() {
                    if siblings.len() > 0 {
                        pending.push(siblings);
                    }
                    next = Some((a, b));
                }
            }
            None => return true,
        }
    }
}
