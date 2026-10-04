//! Scoped Rust allocation accounting for real in-isolate bridge regression tests.
//! V8's own allocator is deliberately outside this counter; the regression is Rust
//! metadata copying, measured only after plugin admission and warmup.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static BYTES: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn account(bytes: usize) {
    let _ = ACTIVE.try_with(|active| {
        if active.get() {
            ALLOCATIONS.with(|n| n.set(n.get() + 1));
            BYTES.with(|n| n.set(n.get() + bytes));
        }
    });
}
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        account(layout.size());
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        account(layout.size());
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        account(size);
        System.realloc(ptr, layout, size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

pub(super) fn measure(run: impl FnOnce()) -> (usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) { ACTIVE.with(|active| active.set(false)); }
    }
    ALLOCATIONS.with(|n| n.set(0));
    BYTES.with(|n| n.set(0));
    ACTIVE.with(|active| assert!(!active.replace(true), "nested allocation accounting"));
    let reset = Reset;
    run();
    drop(reset);
    (ALLOCATIONS.with(Cell::get), BYTES.with(Cell::get))
}
