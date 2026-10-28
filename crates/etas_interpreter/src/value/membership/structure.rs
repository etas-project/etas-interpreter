use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

/// Cursors retain shared storage or borrow a snapshot, never copy payloads.
/// `next` hashes a child header and returns its cursor, if it has children.
pub(crate) trait StructuralPartition: Sized {
    type Identity: Clone + Eq + Hash;
    type Cursor<'a>
    where
        Self: 'a;

    fn start(&self, state: &mut impl Hasher) -> Option<Self::Cursor<'_>>;
    fn next<'a>(
        cursor: &mut Self::Cursor<'a>,
        state: &mut impl Hasher,
    ) -> Option<Option<Self::Cursor<'a>>>
    where
        Self: 'a;
    fn unordered(cursor: &Self::Cursor<'_>) -> bool;
    fn identity(_: &Self::Cursor<'_>) -> Option<Self::Identity> {
        None
    }
    // A record label prefixes its child's hash, rather than belonging to the
    // child's value. The projection remains borrowed from the immutable root.
    fn child_context(_: &Self::Cursor<'_>) -> *const () {
        std::ptr::null()
    }
}

struct Frame<C, H> {
    cursor: C,
    state: H,
    unordered_sum: u64,
    context: *const (),
}

/// Bottom-up keyed fingerprints with a heap traversal stack. Set members and
/// labeled record fields combine commutatively; sequences retain position.
/// Fingerprints are only partitions: callers must still compare exact values.
pub(crate) fn hash_structure<V: StructuralPartition, H: Hasher + Clone>(value: &V, state: &mut H) {
    let seed = state.clone();
    let Some(cursor) = value.start(state) else {
        return;
    };
    let mut frame = Frame {
        cursor,
        state: state.clone(),
        unordered_sum: 0,
        context: std::ptr::null(),
    };
    let mut stack = Vec::new();
    let mut completed: HashMap<(V::Identity, *const ()), u64> = HashMap::new();
    loop {
        let mut child_state = seed.clone();
        if let Some(child_cursor) = V::next(&mut frame.cursor, &mut child_state) {
            if let Some(cursor) = child_cursor {
                let context = V::child_context(&frame.cursor);
                if let Some(hash) =
                    V::identity(&cursor).and_then(|key| completed.get(&(key, context)).copied())
                {
                    append::<V, H>(&mut frame, hash);
                    continue;
                }
                stack.push(frame);
                frame = Frame {
                    cursor,
                    state: child_state,
                    unordered_sum: 0,
                    context,
                };
            } else {
                append::<V, H>(&mut frame, child_state.finish());
            }
        } else {
            if V::unordered(&frame.cursor) {
                frame.unordered_sum.hash(&mut frame.state);
            }
            if let Some(mut parent) = stack.pop() {
                let hash = frame.state.finish();
                if let Some(key) = V::identity(&frame.cursor) {
                    completed.insert((key, frame.context), hash);
                }
                append::<V, H>(&mut parent, hash);
                frame = parent;
            } else {
                *state = frame.state;
                return;
            }
        }
    }
}

fn append<V: StructuralPartition, H: Hasher>(frame: &mut Frame<V::Cursor<'_>, H>, hash: u64) {
    if V::unordered(&frame.cursor) {
        frame.unordered_sum = frame.unordered_sum.wrapping_add(hash);
    } else {
        hash.hash(&mut frame.state);
    }
}
