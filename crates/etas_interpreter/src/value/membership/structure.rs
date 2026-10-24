use std::hash::{Hash, Hasher};

/// Cursors retain shared storage or borrow a snapshot, never copy payloads.
/// `next` hashes a child header and returns its cursor, if it has children.
pub(crate) trait StructuralPartition: Sized {
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
}

struct Frame<C, H> {
    cursor: C,
    state: H,
    unordered_sum: u64,
}

/// Bottom-up keyed fingerprints with a heap traversal stack. Set child hashes
/// combine commutatively; ordered aggregates retain position and field labels.
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
    };
    let mut stack = Vec::new();
    loop {
        let mut child_state = seed.clone();
        if let Some(child_cursor) = V::next(&mut frame.cursor, &mut child_state) {
            if let Some(cursor) = child_cursor {
                stack.push(frame);
                frame = Frame {
                    cursor,
                    state: child_state,
                    unordered_sum: 0,
                };
            } else {
                append::<V, H>(&mut frame, child_state.finish());
            }
        } else {
            if V::unordered(&frame.cursor) {
                frame.unordered_sum.hash(&mut frame.state);
            }
            if let Some(mut parent) = stack.pop() {
                append::<V, H>(&mut parent, frame.state.finish());
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
