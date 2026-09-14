use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Allocations {
    pub count: usize,
    pub bytes: usize,
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
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(ptr, layout, size) }
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
