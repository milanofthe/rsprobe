//! The counting allocator and this thread's counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// A global allocator that adds to this thread's counters and forwards:
/// `bytes` and `allocs` on an allocation, `freed` on a free. A reallocation
/// counts as an allocation of the new size and a free of the old one; a failed
/// call counts nothing. The counters are const-initialised thread-local cells
/// without a destructor, so the wrapper neither allocates nor locks and counts
/// during thread start and teardown too. Without the feature `alloc` it only
/// forwards. Installed by the final binary:
///
/// ```
/// #[global_allocator]
/// static A: rsprobe::Alloc = rsprobe::Alloc::new();
/// ```
pub struct Alloc<A = System>(A);

impl Alloc {
    /// Counts and forwards to [`System`].
    pub const fn new() -> Alloc {
        Alloc(System)
    }
}

impl Default for Alloc {
    fn default() -> Alloc {
        Alloc::new()
    }
}

impl<A> Alloc<A> {
    /// Counts and forwards to `inner`.
    pub const fn wrap(inner: A) -> Alloc<A> {
        Alloc(inner)
    }
}

/// Counters a guard carries: `bytes`, `allocs` and `freed` with the feature
/// `alloc`, none without, so that a scope costs what it did before.
pub(crate) const N: usize = if cfg!(feature = "alloc") { 3 } else { 0 };

thread_local! {
    /// This thread's `bytes`, `allocs` and `freed`.
    static COUNTS: [Cell<u64>; N] = const { [const { Cell::new(0) }; N] };
}

/// Adds `bytes`, `allocs` and `freed` to this thread's counters.
#[inline]
fn count(add: [u64; 3]) {
    if cfg!(feature = "alloc") {
        let _ = COUNTS.try_with(|c| {
            for (c, v) in c.iter().zip(add) {
                c.set(c.get().wrapping_add(v));
            }
        });
    }
}

/// This thread's `bytes`, `allocs` and `freed`; zeros where no [`Alloc`] is
/// installed.
#[inline]
pub(crate) fn counts() -> [u64; N] {
    if !cfg!(feature = "alloc") {
        return [0; N];
    }
    let counts = COUNTS.try_with(|c| c.each_ref().map(Cell::get));
    counts.unwrap_or([0; N])
}

/// Runs `f` and resets this thread's counters to their values before it: the
/// allocations of the instrumentation itself are not counted.
pub(crate) fn quiet<T>(f: impl FnOnce() -> T) -> T {
    let before = counts();
    let r = f();
    if cfg!(feature = "alloc") {
        let _ = COUNTS.try_with(|c| c.iter().zip(before).for_each(|(c, v)| c.set(v)));
    }
    r
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for Alloc<A> {
    #[inline]
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = self.0.alloc(l);
        if !p.is_null() {
            count([l.size() as u64, 1, 0]);
        }
        p
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = self.0.alloc_zeroed(l);
        if !p.is_null() {
            count([l.size() as u64, 1, 0]);
        }
        p
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        self.0.dealloc(p, l);
        count([0, 0, l.size() as u64]);
    }

    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let q = self.0.realloc(p, l, size);
        if !q.is_null() {
            count([size as u64, 1, l.size() as u64]);
        }
        q
    }
}
