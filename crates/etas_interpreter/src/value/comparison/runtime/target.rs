use crate::{
    control::{CallTarget as T, CallTargetChildren, Frame},
    value::ScopedCallableValue,
};

use super::{Children, Comparison, Context, Cursor, context::Pair};

pub(super) fn frames<'c>(a: &Frame, b: &Frame, context: &'c Context) -> Comparison<Cursor<'c>> {
    use crate::control::FrameComparison;
    context.visit();
    context.memoized(
        Pair::Frames(FrameComparison::identity(a), FrameComparison::identity(b)),
        || FrameComparison::new(a, b).map(|frames| Children::Frames(Box::new(frames))),
    )
}

pub(super) fn scoped<'c>(
    a: &ScopedCallableValue,
    b: &ScopedCallableValue,
    context: &'c Context,
) -> Comparison<Cursor<'c>> {
    if !a.equality_header_matches(b) {
        return Comparison::Ready(false);
    }
    context.memoized(
        Pair::Targets(
            a.target_ref() as *const T as usize,
            b.target_ref() as *const T as usize,
        ),
        || Some(Children::Scoped(a.clone(), b.clone())),
    )
}

pub(super) fn lists<'c>(
    a: &CallTargetChildren,
    b: &CallTargetChildren,
    context: &'c Context,
) -> Comparison<Cursor<'c>> {
    if a.len() != b.len() {
        return Comparison::Ready(false);
    }
    context.memoized(
        Pair::TargetLists(a.as_ptr() as usize, b.as_ptr() as usize),
        || Some(Children::Targets(a.clone(), b.clone())),
    )
}

pub(super) fn optional<'c>(
    a: Option<&T>,
    b: Option<&T>,
    context: &'c Context,
) -> Comparison<Cursor<'c>> {
    match (a, b) {
        (Some(a), Some(b)) => target(a, b, context),
        (None, None) => Comparison::Ready(true),
        _ => Comparison::Ready(false),
    }
}

pub(super) fn target<'c>(a: &T, b: &T, context: &'c Context) -> Comparison<Cursor<'c>> {
    context.visit();
    let links = match (a, b) {
        (
            T::Lambda {
                expr: a,
                captured: ac,
            },
            T::Lambda {
                expr: b,
                captured: bc,
            },
        ) => {
            return if a == b {
                frames(ac, bc, context)
            } else {
                Comparison::Ready(false)
            };
        }
        (T::Scoped(a), T::Scoped(b)) => return scoped(a, b, context),
        (
            T::Specialized {
                target: a,
                type_bindings: at,
                effect_row_bindings: ar,
            },
            T::Specialized {
                target: b,
                type_bindings: bt,
                effect_row_bindings: br,
            },
        ) => {
            if at != bt || ar != br {
                return Comparison::Ready(false);
            }
            (a, b)
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
                return Comparison::Ready(false);
            }
            (a, b)
        }
        (T::Composed(a), T::Composed(b)) => return lists(a, b, context),
        (T::FlowItem(a), T::FlowItem(b))
        | (T::AgentItem(a), T::AgentItem(b))
        | (T::ToolItem(a), T::ToolItem(b)) => return Comparison::Ready(a == b),
        (T::SpecImplMethod(a), T::SpecImplMethod(b)) | (T::EnumVariant(a), T::EnumVariant(b)) => {
            return Comparison::Ready(a == b);
        }
        (T::AgentRevision(a), T::AgentRevision(b)) => return Comparison::Ready(a == b),
        (T::NominalConstructor(a), T::NominalConstructor(b)) => return Comparison::Ready(a == b),
        (T::PureIntrinsic(a), T::PureIntrinsic(b)) => return Comparison::Ready(a == b),
        (T::StdIntrinsic(a), T::StdIntrinsic(b)) => return Comparison::Ready(a == b),
        _ => return Comparison::Ready(false),
    };
    let (a, b) = links;
    context.memoized(
        Pair::Targets(&**a as *const T as usize, &**b as *const T as usize),
        || Some(Children::TargetLinks(a.clone(), b.clone())),
    )
}
