use super::*;
use etas_host::{
    Budget, ExecutionBudget, MonotonicClock, TimeBudget,
    execution::{CancellationReason, ExecutionScope},
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct Clock {
    start: Instant,
    millis: AtomicU64,
}
impl MonotonicClock for Clock {
    fn now(&self) -> Instant {
        self.start + Duration::from_millis(self.millis.load(Ordering::Relaxed))
    }
}

fn span() -> Span {
    Span::empty(etas_core::SourceId(9), etas_core::TextSize::ZERO)
}

#[test]
fn quantum_yield_preserves_consumed_fuel_and_scope_stop_wins_before_more_work() {
    let scope = ExecutionScope::new_owned();
    let signal = scope.signal().unwrap();
    let budget = ExecutionBudget::start(Budget::default());
    let mut scheduler = ExecutionSafePointScheduler::new(80);
    for _ in 0..WORK_QUANTUM {
        assert!(matches!(
            scheduler.observe(&signal, &budget, span()),
            SafePointDecision::Continue
        ));
        scheduler
            .consume(ExecutionLimits::default(), &budget, span())
            .unwrap();
    }
    assert!(matches!(
        scheduler.observe(&signal, &budget, span()),
        SafePointDecision::Yield
    ));
    assert_eq!(scheduler.consumed_steps(), 80 + WORK_QUANTUM);
    assert!(matches!(
        scheduler.observe(&signal, &budget, span()),
        SafePointDecision::Continue
    ));
    scope
        .cancel_source()
        .stop(CancellationReason::Requested)
        .unwrap();
    assert!(matches!(
        scheduler.observe(&signal, &budget, span()),
        SafePointDecision::Cancelled(_)
    ));
    assert_eq!(scheduler.consumed_steps(), 80 + WORK_QUANTUM);
}

#[test]
fn safe_points_use_the_run_budget_monotonic_clock_without_wall_time_sleeps() {
    let clock = Arc::new(Clock {
        start: Instant::now(),
        millis: AtomicU64::new(0),
    });
    let budget = ExecutionBudget::start_with_clock(
        Budget {
            time: Some(TimeBudget { max_millis: 10 }),
            ..Budget::default()
        },
        clock.clone(),
    );
    let mut scheduler = ExecutionSafePointScheduler::new(0);
    for _ in 0..TIME_BUDGET_CHECK_INTERVAL {
        scheduler
            .consume(ExecutionLimits::default(), &budget, span())
            .unwrap();
    }
    clock.millis.store(11, Ordering::Relaxed);
    let failure = scheduler
        .consume(ExecutionLimits::default(), &budget, span())
        .unwrap_err();
    assert!(failure.message.contains("wall-time"));
    assert_eq!(failure.span, span());
    assert_eq!(scheduler.consumed_steps(), TIME_BUDGET_CHECK_INTERVAL);
}
