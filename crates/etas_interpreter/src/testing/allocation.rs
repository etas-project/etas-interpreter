use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Allocations {
    pub count: usize,
    pub bytes: usize,
    pub released_bytes: usize,
}

thread_local! {
    static ACTIVE: Cell<Option<Allocations>> = const { Cell::new(None) };
}

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn record(bytes: usize) {
    let _ = ACTIVE.try_with(|active| {
        if let Some(mut stats) = active.get() {
            stats.count = stats.count.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(bytes);
            active.set(Some(stats));
        }
    });
}

fn record_release(bytes: usize) {
    let _ = ACTIVE.try_with(|active| {
        if let Some(mut stats) = active.get() {
            stats.released_bytes = stats.released_bytes.saturating_add(bytes);
            active.set(Some(stats));
        }
    });
}

// Forward every allocation unchanged; only this test thread's measured region is counted.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record_release(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            record_release(layout.size());
        }
        result
    }
}

pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, Allocations) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.with(|active| active.set(None));
        }
    }
    ACTIVE.with(|active| {
        assert!(active.get().is_none(), "nested allocation measurement");
        active.set(Some(Allocations::default()));
    });
    let reset = Reset;
    let result = operation();
    let stats = ACTIVE.with(|active| active.get().unwrap());
    drop(reset);
    (result, stats)
}

#[test]
fn allocation_counter_distinguishes_retained_output_from_released_temporaries() {
    let (retained, retained_cost) = measure(|| std::hint::black_box(vec![7u8; 1024]));
    assert_eq!(retained_cost.bytes, 1024);
    assert_eq!(retained_cost.released_bytes, 0);
    let (_, released_cost) = measure(|| drop(retained));
    assert_eq!(released_cost.bytes, 0);
    assert_eq!(released_cost.released_bytes, 1024);
}

#[test]
fn allocation_counter_includes_reallocated_old_storage_in_releases() {
    let (_, cost) = measure(|| {
        let mut buffer = Vec::<u8>::with_capacity(8);
        buffer.resize(1024, 7);
        drop(std::hint::black_box(buffer));
    });
    assert!(cost.count >= 2);
    assert!(cost.bytes >= 1032);
    assert_eq!(cost.released_bytes, cost.bytes);
}
