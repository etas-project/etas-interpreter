use std::cell::RefCell;

use etas_builtin::{
    text::{
        iteration::{
            TextCursorState, TextSequenceElement, TextSequencePlan, TextSequenceStep,
            TextSequenceWork,
        },
        output::TextOutputWork,
    },
    work::{WorkControl, WorkPoll},
};
use etas_types::TypeId;

use super::{InterpValue, ListValue, OwnedTextCursor, StringValue, range::IntegerRange};

#[derive(Clone, Debug)]
pub struct TextSequenceValue {
    source: StringValue,
    plan: TextSequencePlan,
    item_type: TypeId,
}

impl TextSequenceValue {
    pub(crate) fn new(
        source: StringValue,
        plan: TextSequencePlan,
        item_type: TypeId,
    ) -> Result<Self, String> {
        plan.validate().map_err(str::to_owned)?;
        Ok(Self {
            source,
            plan,
            item_type,
        })
    }

    pub(crate) fn source(&self) -> &StringValue {
        &self.source
    }

    pub(crate) fn plan(&self) -> &TextSequencePlan {
        &self.plan
    }

    pub(crate) fn item_type(&self) -> TypeId {
        self.item_type
    }

    fn element(&self, element: TextSequenceElement) -> Result<InterpValue, String> {
        Ok(match element {
            TextSequenceElement::Scalar(value) => InterpValue::String(value.to_string().into()),
            TextSequenceElement::IndexedScalar { byte, scalar } => InterpValue::Tuple(
                vec![
                    InterpValue::usize(byte),
                    InterpValue::String(scalar.to_string().into()),
                ]
                .into(),
            ),
            TextSequenceElement::Segment(_) | TextSequenceElement::IndexedSegment { .. } => {
                return Err("text segment must pass through bounded projection".into());
            }
            TextSequenceElement::Match(range) => InterpValue::Tuple(
                vec![
                    InterpValue::usize(range.start),
                    InterpValue::usize(range.end),
                ]
                .into(),
            ),
        })
    }
}

#[derive(Clone, Debug)]
struct TextProjection {
    range: std::ops::Range<usize>,
    indexed: Option<usize>,
    output: TextOutputWork,
}

#[derive(Clone, Debug)]
struct TextIterationCursor {
    position: usize,
    scan: OwnedTextCursor,
    projection: OwnedTextCursor,
    work: TextSequenceWork,
    pending_projection: Option<TextProjection>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IterationPoll {
    Pending,
    Ready(Option<InterpValue>),
}

/// Retains the evaluated collection version. Source updates use COW, so advancing the
/// cursor clones only the visited element, never the complete collection.
#[derive(Clone, Debug)]
pub(crate) struct IterationSource {
    value: InterpValue,
    list_cursor: RefCell<Option<(usize, ListValue)>>,
    queue_cursor: RefCell<(usize, usize)>,
    ordered_cursor: RefCell<Option<(usize, super::ordered::Cursor)>>,
    text_cursor: RefCell<Option<TextIterationCursor>>,
}

impl IterationSource {
    pub(crate) fn new(value: InterpValue) -> Result<Self, String> {
        match &value {
            InterpValue::Array(_)
            | InterpValue::List(_)
            | InterpValue::Slice(_)
            | InterpValue::Set(_)
            | InterpValue::Deque(_)
            | InterpValue::Queue(_)
            | InterpValue::Stack(_)
            | InterpValue::Map(_)
            | InterpValue::PriorityQueue(_)
            | InterpValue::OrderedMap(_)
            | InterpValue::OrderedSet(_) => {}
            InterpValue::Range(range) => {
                IntegerRange::from_value(range)
                    .map_err(|e| format!("invalid checked integer range: {e:?}"))?;
            }
            InterpValue::TextSequence(value) => value.plan().validate().map_err(str::to_owned)?,
            _ => return Err("for iteration requires a local collection or range value".into()),
        }
        Ok(Self {
            value,
            list_cursor: RefCell::new(None),
            queue_cursor: RefCell::new((0, 0)),
            ordered_cursor: RefCell::new(None),
            text_cursor: RefCell::new(None),
        })
    }

    pub(crate) fn restore_text_cursor(
        value: InterpValue,
        position: usize,
        cursor: TextCursorState,
    ) -> Result<Self, String> {
        let source = Self::new(value)?;
        if !matches!(source.value, InterpValue::TextSequence(_)) {
            return Err("text cursor requires a text sequence source".into());
        }
        let InterpValue::TextSequence(value) = &source.value else {
            unreachable!("checked above")
        };
        *source.text_cursor.borrow_mut() = Some(TextIterationCursor {
            position,
            scan: OwnedTextCursor::at(value.source(), cursor.byte_offset)?,
            projection: OwnedTextCursor::new(value.source()),
            work: TextSequenceWork::new(value.plan().clone(), cursor)
                .map_err(|error| format!("invalid checked text cursor: {error:?}"))?,
            pending_projection: None,
        });
        Ok(source)
    }

    pub(crate) fn value(&self) -> &InterpValue {
        &self.value
    }

    pub(crate) fn text_cursor(&self, position: usize) -> Result<Option<TextCursorState>, String> {
        let cursor = self.text_cursor.borrow();
        match (&self.value, cursor.as_ref()) {
            (InterpValue::TextSequence(_), Some(cursor)) if cursor.position == position => {
                Ok(Some(cursor.work.cursor().clone()))
            }
            (InterpValue::TextSequence(_), Some(cursor)) => Err(format!(
                "text iteration cursor is at {}, expected {position}",
                cursor.position
            )),
            (InterpValue::TextSequence(_), None) if position == 0 => {
                Ok(Some(TextCursorState::default()))
            }
            (InterpValue::TextSequence(_), None) => {
                Err("text iteration cursor is missing after progress".into())
            }
            (_, _) => Ok(None),
        }
    }

    pub(crate) fn get(&self, index: usize) -> Result<Option<InterpValue>, String> {
        if matches!(self.value, InterpValue::TextSequence(_)) {
            loop {
                let mut control = WorkControl::new(usize::MAX, usize::MAX, usize::MAX);
                match self.poll(index, &mut control)? {
                    IterationPoll::Pending => continue,
                    IterationPoll::Ready(value) => return Ok(value),
                }
            }
        }
        Ok(match &self.value {
            InterpValue::Array(values) | InterpValue::Stack(values) => {
                values.borrow().get(index).cloned()
            }
            InterpValue::Deque(values) | InterpValue::Queue(values) => {
                values.borrow().get(index).cloned()
            }
            InterpValue::List(values) => {
                let mut cursor = self.list_cursor.borrow_mut();
                if cursor
                    .as_ref()
                    .is_some_and(|(position, _)| *position > index)
                {
                    *cursor = None;
                }
                let (position, remaining) = cursor.get_or_insert_with(|| (0, values.clone()));
                while *position < index {
                    if !remaining.advance() {
                        return Ok(None);
                    }
                    *position += 1;
                }
                remaining.get(0).cloned()
            }
            InterpValue::Slice(values) => values.borrow().get(index).cloned(),
            InterpValue::Set(values) => values.borrow().get(index).cloned(),
            InterpValue::OrderedMap(values) => self.ordered_get(index, || values.cursor(), true),
            InterpValue::OrderedSet(values) => self.ordered_get(index, || values.cursor(), false),
            InterpValue::PriorityQueue(values) => {
                let mut cursor = self.queue_cursor.borrow_mut();
                if cursor.0 > index {
                    *cursor = (0, 0);
                }
                loop {
                    if cursor.1 == values.slot_count() {
                        return Ok(None);
                    }
                    if let Some((priority, value)) = values.slot(cursor.1) {
                        if cursor.0 == index {
                            break Some(InterpValue::Tuple(
                                vec![priority.clone(), value.clone()].into(),
                            ));
                        }
                        cursor.0 += 1;
                    }
                    cursor.1 += 1;
                }
            }
            InterpValue::Map(values) => values
                .borrow()
                .get(index)
                .map(|(key, value)| InterpValue::Tuple(vec![key.clone(), value.clone()].into())),
            InterpValue::Range(range) => IntegerRange::from_value(range)
                .and_then(|range| range.get(index))
                .map_err(|e| format!("invalid checked integer range: {e:?}"))?
                .map(InterpValue::Number),
            InterpValue::TextSequence(_) => unreachable!("handled before collection access"),
            _ => return Err("invalid iteration source".into()),
        })
    }

    pub(crate) fn is_text_sequence(&self) -> bool {
        matches!(self.value, InterpValue::TextSequence(_))
    }

    pub(crate) fn native_output_reserved(&self) -> usize {
        self.text_cursor
            .borrow()
            .as_ref()
            .and_then(|cursor| cursor.pending_projection.as_ref())
            .map_or(0, |projection| projection.output.reserved_bytes())
    }

    pub(crate) fn native_scratch_reserved(&self) -> usize {
        self.text_cursor
            .borrow()
            .as_ref()
            .map_or(0, |cursor| cursor.work.reserved_scratch_bytes())
    }

    pub(crate) fn poll(
        &self,
        index: usize,
        control: &mut WorkControl,
    ) -> Result<IterationPoll, String> {
        let InterpValue::TextSequence(value) = &self.value else {
            return self.get(index).map(IterationPoll::Ready);
        };
        let mut slot = self.text_cursor.borrow_mut();
        if slot.as_ref().is_some_and(|cursor| cursor.position > index) {
            *slot = None;
        }
        if slot.is_none() {
            *slot = Some(TextIterationCursor {
                position: 0,
                scan: OwnedTextCursor::new(value.source()),
                projection: OwnedTextCursor::new(value.source()),
                work: TextSequenceWork::new(value.plan().clone(), TextCursorState::default())
                    .map_err(|error| format!("invalid checked text iterator: {error:?}"))?,
                pending_projection: None,
            });
        }
        let cursor = slot.as_mut().expect("initialized above");
        while cursor.position <= index {
            if let Some(projection) = &mut cursor.pending_projection {
                match project_segment(value.source(), &mut cursor.projection, projection, control)?
                {
                    WorkPoll::Pending => return Ok(IterationPoll::Pending),
                    WorkPoll::Ready(text) => {
                        let indexed = projection.indexed;
                        cursor.pending_projection = None;
                        let item = match indexed {
                            Some(byte) => InterpValue::Tuple(
                                vec![InterpValue::usize(byte), InterpValue::String(text)].into(),
                            ),
                            None => InterpValue::String(text),
                        };
                        let selected = cursor.position == index;
                        cursor.position += 1;
                        if selected {
                            return Ok(IterationPoll::Ready(Some(item)));
                        }
                        continue;
                    }
                }
            }
            let Some(piece) = cursor.scan.next_piece() else {
                let (_, poll) = cursor
                    .work
                    .step("", true, control)
                    .map_err(|error| format!("invalid checked text iteration: {error:?}"))?;
                match poll {
                    WorkPoll::Pending => return Ok(IterationPoll::Pending),
                    WorkPoll::Ready(step) => {
                        if let Some(result) = handle_sequence_step(value, cursor, step, index)? {
                            return Ok(result);
                        }
                        continue;
                    }
                }
            };
            let input = piece.as_str();
            let end_of_input = cursor.scan.position() + input.len() == value.source().len();
            let (consumed, poll) = cursor
                .work
                .step(input, end_of_input, control)
                .map_err(|error| format!("invalid checked text iteration: {error:?}"))?;
            cursor.scan.advance(consumed);
            match poll {
                WorkPoll::Pending => return Ok(IterationPoll::Pending),
                WorkPoll::Ready(step) => {
                    if let Some(result) = handle_sequence_step(value, cursor, step, index)? {
                        return Ok(result);
                    }
                }
            }
        }
        Err("text iteration advanced past its requested element".into())
    }

    fn ordered_get(
        &self,
        index: usize,
        create: impl FnOnce() -> super::ordered::Cursor,
        pairs: bool,
    ) -> Option<InterpValue> {
        let mut cursor = self.ordered_cursor.borrow_mut();
        if cursor
            .as_ref()
            .is_some_and(|(position, _)| *position > index)
        {
            *cursor = None;
        }
        let (position, source) = cursor.get_or_insert_with(|| (0, create()));
        while *position < index {
            source.next()?;
            *position += 1;
        }
        let (key, value) = source.next()?;
        *position += 1;
        Some(if pairs {
            InterpValue::Tuple(vec![key, value].into())
        } else {
            key
        })
    }
}

fn handle_sequence_step(
    value: &TextSequenceValue,
    cursor: &mut TextIterationCursor,
    step: TextSequenceStep,
    index: usize,
) -> Result<Option<IterationPoll>, String> {
    let element = match step {
        TextSequenceStep::End => return Ok(Some(IterationPoll::Ready(None))),
        TextSequenceStep::Item(element) => element,
    };
    match element {
        TextSequenceElement::Segment(range) => {
            cursor.pending_projection = Some(TextProjection {
                range,
                indexed: None,
                output: TextOutputWork::default(),
            });
            Ok(None)
        }
        TextSequenceElement::IndexedSegment { byte, range } => {
            cursor.pending_projection = Some(TextProjection {
                range,
                indexed: Some(byte),
                output: TextOutputWork::default(),
            });
            Ok(None)
        }
        element => {
            let item = value.element(element)?;
            let selected = cursor.position == index;
            cursor.position += 1;
            Ok(selected.then_some(IterationPoll::Ready(Some(item))))
        }
    }
}

fn project_segment(
    source: &StringValue,
    cursor: &mut OwnedTextCursor,
    projection: &mut TextProjection,
    control: &mut WorkControl,
) -> Result<WorkPoll<StringValue>, String> {
    if projection.range.start == 0 && projection.range.end == source.len() {
        return Ok(WorkPoll::Ready(source.clone()));
    }
    if projection.range.is_empty() {
        return Ok(WorkPoll::Ready(StringValue::default()));
    }
    projection
        .output
        .reserve_exact(projection.range.len(), control)
        .map_err(|error| format!("text sequence output limit failed: {error:?}"))?;
    while cursor.position() < projection.range.start {
        let piece = cursor
            .next_piece()
            .ok_or("text segment start exceeds source length")?;
        let missing = projection.range.start - cursor.position();
        let input = &piece.as_str()[..missing.min(piece.as_str().len())];
        let mut consumed = 0usize;
        while let Some(scalar) = input[consumed..].chars().next() {
            let width = scalar.len_utf8();
            if control.consume(width).is_err() {
                cursor.advance(consumed);
                return Ok(WorkPoll::Pending);
            }
            consumed += width;
        }
        cursor.advance(consumed);
    }
    while cursor.position() < projection.range.end {
        let piece = cursor
            .next_piece()
            .ok_or("text segment end exceeds source length")?;
        let remaining = projection.range.end - cursor.position();
        let input = &piece.as_str()[..remaining.min(piece.as_str().len())];
        let consumed = projection
            .output
            .append_step(input, control)
            .map_err(|error| format!("text sequence output failed: {error:?}"))?;
        cursor.advance(consumed);
        if consumed != input.len() {
            return Ok(WorkPoll::Pending);
        }
    }
    Ok(WorkPoll::Ready(projection.output.take().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::allocation::measure;
    use crate::value::{ArrayValue, NumericValue, RangeBounds, RangeValue};

    #[test]
    fn iteration_setup_and_early_break_do_not_copy_unvisited_payloads() {
        for count in [1000, 2000, 4000] {
            let values = ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("x".repeat(128).into()))
                    .collect(),
            );
            let input = InterpValue::Array(values.clone());
            let (source, setup) = measure(|| IterationSource::new(input).unwrap());
            assert_eq!(setup.count, 0);
            let (first, access) = measure(|| source.get(0).unwrap());
            assert!(matches!(first, Some(InterpValue::String(_))));
            assert_eq!(access.count, 0);
            assert_eq!(access.bytes, 0);
            let (_, drop_cost) = measure(|| drop(source));
            assert_eq!(drop_cost.count, 0);
        }
    }

    #[test]
    fn iterator_retains_collection_version_across_cow_updates() {
        let mut values = ArrayValue::new(vec![InterpValue::i32(1), InterpValue::i32(2)]);
        let source = IterationSource::new(InterpValue::Array(values.clone())).unwrap();
        values.make_unique();
        values.borrow_mut()[1] = InterpValue::i32(9);
        assert_eq!(source.get(1).unwrap(), Some(InterpValue::i32(2)));
        assert_eq!(values.borrow()[1], InterpValue::i32(9));
    }

    #[test]
    fn huge_range_iteration_is_lazy_and_stops_at_inclusive_endpoint() {
        let source = IterationSource::new(InterpValue::Range(RangeValue {
            start: Box::new(InterpValue::Number(NumericValue::U128(0))),
            end: Box::new(InterpValue::Number(NumericValue::U128(u128::MAX))),
            bounds: RangeBounds::ClosedClosed,
        }))
        .unwrap();
        let (first, allocations) = measure(|| source.get(0).unwrap());
        assert_eq!(first, Some(InterpValue::Number(NumericValue::U128(0))));
        assert_eq!(allocations.count, 0);
        assert!(IterationSource::new(InterpValue::Bool(true)).is_err());
    }

    #[test]
    fn map_iteration_allocates_only_the_visited_pair_not_the_backing() {
        for count in [1000, 2000, 4000] {
            let values = crate::value::MapValue::new(
                (0..count)
                    .map(|n| {
                        (
                            InterpValue::String(format!("{n}{}", "k".repeat(1024)).into()),
                            InterpValue::Bytes(vec![7; 1024].into()),
                        )
                    })
                    .collect(),
            );
            let input = InterpValue::Map(values.clone());
            let (source, setup) = measure(|| IterationSource::new(input).unwrap());
            assert_eq!(setup.count, 0, "n={count}: {setup:?}");
            let (first, access) = measure(|| source.get(0).unwrap());
            assert!(matches!(first, Some(InterpValue::Tuple(_))));
            assert_eq!(access.count, 2, "n={count}: {access:?}");
            assert!(
                access.bytes <= 2 * std::mem::size_of::<InterpValue>() + 128,
                "n={count}: {access:?}"
            );
            let (_, drop_cost) = measure(|| drop(source));
            assert_eq!(drop_cost.count, 0);
        }
    }

    #[test]
    fn text_iteration_yields_during_a_large_segment_without_a_delimiter() {
        let value = TextSequenceValue::new(
            StringValue::from("x".repeat(256 * 1024)),
            TextSequencePlan::new(etas_builtin::text::iteration::TextSequenceKind::Lines),
            TypeId(0),
        )
        .unwrap();
        let source = IterationSource::new(InterpValue::TextSequence(value)).unwrap();
        let mut polls = 0usize;
        loop {
            let mut control = WorkControl::new(32, usize::MAX, usize::MAX);
            match source.poll(0, &mut control).unwrap() {
                IterationPoll::Pending => polls += 1,
                IterationPoll::Ready(Some(InterpValue::String(value))) => {
                    assert_eq!(value.len(), 256 * 1024);
                    break;
                }
                other => panic!("unexpected text iteration result: {other:?}"),
            }
        }
        assert!(polls > 1);
    }
}
