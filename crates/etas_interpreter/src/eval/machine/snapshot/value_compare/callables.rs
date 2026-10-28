use super::{Comparison, Pending, Seen, equal};
use crate::orchestration::{CallTargetSnapshot as Target, LocalsSnapshot};

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        equal(Comparison::Target(self, other))
    }
}

impl PartialEq for LocalsSnapshot {
    fn eq(&self, other: &Self) -> bool {
        if std::rc::Rc::ptr_eq(&self.locals, &other.locals) {
            return self.id == other.id
                && self.type_bindings == other.type_bindings
                && self.effect_row_bindings == other.effect_row_bindings;
        }
        equal(Comparison::Frame(self, other))
    }
}

pub(super) fn frame<'a>(
    a: &'a LocalsSnapshot,
    b: &'a LocalsSnapshot,
    pending: &mut Pending<'a>,
    seen: &mut Seen,
) -> bool {
    if a.id != b.id
        || a.type_bindings != b.type_bindings
        || a.effect_row_bindings != b.effect_row_bindings
        || a.locals.len() != b.locals.len()
    {
        return false;
    }
    if std::rc::Rc::ptr_eq(&a.locals, &b.locals) {
        return true;
    }
    if seen.edge(
        2,
        Some(std::rc::Rc::as_ptr(&a.locals)),
        Some(std::rc::Rc::as_ptr(&b.locals)),
    ) {
        return true;
    }
    for ((asym, av), (bsym, bv)) in a.locals.iter().zip(b.locals.iter()) {
        if asym != bsym {
            return false;
        }
        pending.push((av, bv));
    }
    true
}

pub(super) fn target<'a>(
    a: &'a Target,
    b: &'a Target,
    pending: &mut Pending<'a>,
    next: &mut Option<Comparison<'a>>,
    seen: &mut Seen,
) -> bool {
    match (a, b) {
        (Target::FlowItem(a), Target::FlowItem(b))
        | (Target::AgentItem(a), Target::AgentItem(b))
        | (Target::ToolItem(a), Target::ToolItem(b)) => a == b,
        (Target::AgentRevision(a), Target::AgentRevision(b)) => a == b,
        (Target::SpecImplMethod(a), Target::SpecImplMethod(b))
        | (Target::EnumVariant(a), Target::EnumVariant(b)) => a == b,
        (Target::NominalConstructor(a), Target::NominalConstructor(b)) => a == b,
        (
            Target::Lambda {
                expr: a,
                captured: ac,
            },
            Target::Lambda {
                expr: b,
                captured: bc,
            },
        ) => {
            if a != b {
                return false;
            }
            *next = Some(Comparison::Frame(ac, bc));
            true
        }
        (
            Target::Scoped {
                metadata: a,
                target: at,
            },
            Target::Scoped {
                metadata: b,
                target: bt,
            },
        ) => {
            if a != b {
                return false;
            }
            if !seen.edge(0, at.shared_identity(), bt.shared_identity()) {
                *next = Some(Comparison::Target(at, bt));
            }
            true
        }
        (
            Target::Specialized {
                target: a,
                type_bindings: at,
                effect_row_bindings: ar,
            },
            Target::Specialized {
                target: b,
                type_bindings: bt,
                effect_row_bindings: br,
            },
        ) => {
            if at != bt || ar != br {
                return false;
            }
            if !seen.edge(0, a.shared_identity(), b.shared_identity()) {
                *next = Some(Comparison::Target(a, b));
            }
            true
        }
        (
            Target::Limited {
                target: a,
                limits: al,
            },
            Target::Limited {
                target: b,
                limits: bl,
            },
        ) => {
            if al != bl {
                return false;
            }
            if !seen.edge(0, a.shared_identity(), b.shared_identity()) {
                *next = Some(Comparison::Target(a, b));
            }
            true
        }
        (Target::Composed(a), Target::Composed(b)) => {
            if a.len() != b.len() {
                return false;
            }
            if !seen.edge(1, a.shared_identity(), b.shared_identity()) {
                let mut pairs = a.iter().zip(b.iter());
                if let Some((a, b)) = pairs.next() {
                    if pairs.len() > 0 {
                        pending.0.push(Comparison::Targets(pairs));
                    }
                    *next = Some(Comparison::Target(a, b));
                }
            }
            true
        }
        (
            Target::PureIntrinsic {
                intrinsic: a,
                parameter_types: ap,
                result_type: ar,
            },
            Target::PureIntrinsic {
                intrinsic: b,
                parameter_types: bp,
                result_type: br,
            },
        ) => a == b && ap == bp && ar == br,
        (
            Target::StdIntrinsic {
                origin: a,
                substitutions: ag,
                intrinsic: ai,
                dispatch: ad,
                parameter_types: ap,
                result_type: ar,
            },
            Target::StdIntrinsic {
                origin: b,
                substitutions: bg,
                intrinsic: bi,
                dispatch: bd,
                parameter_types: bp,
                result_type: br,
            },
        ) => a == b && ag == bg && ai == bi && ad == bd && ap == bp && ar == br,
        _ => false,
    }
}
