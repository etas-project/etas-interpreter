use super::*;
use crate::testing::allocation::measure;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    comparisons: usize,
    swaps: usize,
    copied_headers: usize,
    copied_slots: usize,
    compacted_slots: usize,
}

thread_local! { static COST: Cell<Cost> = const { Cell::new(Cost { comparisons: 0, swaps: 0, copied_headers: 0, copied_slots: 0, compacted_slots: 0 }) }; }

fn update_cost(edit: impl FnOnce(Cost) -> Cost) {
    COST.set(edit(COST.get()));
}

pub(super) fn record_clone(live: usize, slots: usize) {
    update_cost(|cost| Cost {
        copied_headers: cost.copied_headers + live * 2,
        copied_slots: cost.copied_slots + slots,
        ..cost
    });
}
pub(super) fn record_comparison() {
    update_cost(|cost| Cost {
        comparisons: cost.comparisons + 1,
        ..cost
    });
}
pub(super) fn record_swap() {
    update_cost(|cost| Cost {
        swaps: cost.swaps + 1,
        ..cost
    });
}
pub(super) fn record_compaction(slots: usize) {
    update_cost(|cost| Cost {
        compacted_slots: cost.compacted_slots + slots,
        ..cost
    });
}

fn drive(
    queue: PriorityQueueValue,
    operation: Operation,
) -> (PriorityQueueValue, Option<InterpValue>) {
    let mut update = queue.update(operation);
    let mut order = None;
    loop {
        match update.advance(order.take()).unwrap() {
            UpdateStep::Continue => {}
            UpdateStep::Compare => {
                let ComparisonCall {
                    target,
                    left,
                    right,
                } = update.comparison().unwrap();
                assert!(target.is_none());
                order = Some(
                    left.as_number()
                        .unwrap()
                        .partial_cmp_same(right.as_number().unwrap())
                        .unwrap()
                        .unwrap(),
                );
            }
            UpdateStep::Complete => {
                let result = update.finish().unwrap();
                return (result.queue, result.popped);
            }
        }
    }
}

fn queue(count: usize) -> PriorityQueueValue {
    let mut queue = PriorityQueueValue::new(None);
    for priority in 0..count {
        queue = drive(
            queue,
            Operation::push(
                InterpValue::i32(priority as i32),
                InterpValue::String("x".repeat(1024).into()),
            ),
        )
        .0;
    }
    queue
}

#[test]
fn priority_queue_unique_and_shared_update_costs_are_measured_separately() {
    for count in [1000, 2000, 4000] {
        let mut queue = queue(count);
        let data = Rc::get_mut(&mut queue.0).unwrap();
        data.slots.reserve(1);
        data.heap.reserve(1);
        COST.set(Cost::default());
        let ((queue, popped), allocations) = measure(|| drive(queue, Operation::pop()));
        let unique = COST.get();
        assert_eq!(unique.copied_headers, 0);
        assert_eq!(allocations.count, 0, "n={count}: {allocations:?}");
        assert!(
            unique.comparisons <= 2 * count.ilog2() as usize + 2,
            "{unique:?}"
        );
        assert!(matches!(popped, Some(InterpValue::String(_))));
        let alias = queue.clone();
        let pointer = match &alias.iter().next().unwrap().1 {
            InterpValue::String(s) => s.as_ptr(),
            _ => unreachable!(),
        };
        COST.set(Cost::default());
        let ((updated, _), shared_allocations) = measure(|| {
            drive(
                queue,
                Operation::push(InterpValue::i32(-1), InterpValue::Unit),
            )
        });
        let shared = COST.get();
        assert_eq!(shared.copied_headers, 2 * (count - 1));
        assert_eq!(shared.copied_slots, count);
        assert!(shared.comparisons <= count.ilog2() as usize + 2);
        assert_eq!(
            shared_allocations.count, 3,
            "n={count}: {shared_allocations:?}"
        );
        assert!(
            shared_allocations.bytes
                < count
                    * (std::mem::size_of::<Option<(InterpValue, InterpValue)>>()
                        + std::mem::size_of::<usize>())
                    + 4096
        );
        assert_eq!(alias.len(), count - 1);
        assert_eq!(updated.len(), count);
        assert_eq!(
            match &updated.iter().next().unwrap().1 {
                InterpValue::String(s) => s.as_ptr(),
                _ => unreachable!(),
            },
            pointer
        );
        eprintln!(
            "priority n={count}: unique={unique:?}/{allocations:?}; shared={shared:?}/{shared_allocations:?}"
        );
    }
}

#[test]
fn priority_queue_repeated_drain_is_logarithmic_with_amortized_linear_compaction() {
    for count in [1000_usize, 2000, 4000] {
        let mut queue = PriorityQueueValue::new(None);
        for priority in 0..count {
            queue = drive(
                queue,
                Operation::push(
                    InterpValue::i32(priority as i32),
                    InterpValue::i32(priority as i32),
                ),
            )
            .0;
        }
        COST.set(Cost::default());
        for expected in (0..count).rev() {
            let (next, popped) = drive(queue, Operation::pop());
            assert_eq!(popped, Some(InterpValue::i32(expected as i32)));
            queue = next;
            assert!(queue.slot_count() <= queue.len() * 2 + 1);
        }
        let cost = COST.get();
        assert_eq!(cost.copied_headers, 0);
        assert!(cost.comparisons <= 2 * count * (count.ilog2() as usize + 1));
        assert!(cost.compacted_slots < 2 * count, "n={count}: {cost:?}");
        assert_eq!(queue.slot_count(), 0);
        assert!(drive(queue, Operation::pop()).1.is_none());
        eprintln!("priority drain n={count}: {cost:?}");
    }
}

#[test]
fn priority_queue_unique_push_and_shared_pop_preserve_the_cost_contract() {
    for count in [1000, 2000, 4000] {
        let mut queue = queue(count);
        let data = Rc::get_mut(&mut queue.0).unwrap();
        data.slots.reserve(1);
        data.heap.reserve(1);
        COST.set(Cost::default());
        let ((queue, _), allocation) = measure(|| {
            drive(
                queue,
                Operation::push(InterpValue::i32(count as i32), InterpValue::Unit),
            )
        });
        assert_eq!(COST.get().copied_headers, 0);
        assert!(COST.get().comparisons <= count.ilog2() as usize + 1);
        assert_eq!(allocation.count, 0, "{allocation:?}");
        let alias = queue.clone();
        COST.set(Cost::default());
        let ((remaining, popped), allocation) = measure(|| drive(queue, Operation::pop()));
        let cost = COST.get();
        assert_eq!(popped, Some(InterpValue::Unit));
        assert_eq!(cost.copied_headers, 2 * (count + 1));
        assert!(cost.comparisons <= 2 * count.ilog2() as usize + 2);
        assert_eq!(allocation.count, 3, "{allocation:?}");
        assert_eq!(remaining.len(), count);
        assert_eq!(alias.len(), count + 1);
        assert_eq!(drive(alias, Operation::pop()).1, Some(InterpValue::Unit));
        eprintln!("priority shared-pop n={count}: {cost:?}/{allocation:?}");
    }
}

#[test]
fn priority_queue_restores_logical_entries_then_builds_a_heap_once() {
    for count in [1000_usize, 2000, 4000] {
        let queue = PriorityQueueValue::from_entries(
            (0..count)
                .map(|n| (InterpValue::i32(n as i32), InterpValue::i32(n as i32)))
                .collect(),
            None,
        );
        COST.set(Cost::default());
        let (queue, value) = drive(queue, Operation::pop());
        assert_eq!(value, Some(InterpValue::i32(count as i32 - 1)));
        assert!(COST.get().comparisons < 2 * count + 2 * count.ilog2() as usize);
        assert_eq!(COST.get().copied_headers, 0);
        COST.set(Cost::default());
        let (_, value) = drive(queue, Operation::pop());
        assert_eq!(value, Some(InterpValue::i32(count as i32 - 2)));
        assert!(COST.get().comparisons <= 2 * count.ilog2() as usize + 2);
    }
}

#[test]
fn deep_nested_priority_values_drop_without_recursive_rust_stack_growth() {
    let mut value = InterpValue::Unit;
    for _ in 0..4000 {
        value = InterpValue::PriorityQueue(vec![(InterpValue::i32(1), value)].into());
    }
    drop(value);
}
