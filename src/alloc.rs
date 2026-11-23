//! The counting allocator and this thread's counters.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

/// A global allocator that adds to this thread's counters and forwards:
/// `bytes` and `allocs` on an allocation, `freed` on a free. A reallocation
/// counts as an allocation of the new size and a free of the old one; a failed
/// call counts nothing.
///
/// `peak` of a scope: the largest value of this thread's live bytes (`bytes`
/// minus `freed`) after an allocation inside the scope, minus their value at
/// entry; zero where nothing allocated beyond the entry level. A scope's peak
/// includes those of the scopes inside it, which are relative to their own
/// entry. A node keeps the maximum over its calls and threads: the peaks of
/// threads running at once are not added. Memory freed on another thread
/// lowers the live bytes of that one. The counters are const-initialised thread-local cells
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

/// Counters a guard carries: `bytes`, `allocs`, `freed` and the high-water
/// mark with the feature `alloc`, none without, so that a scope costs what it
/// did before.
pub(crate) const N: usize = if cfg!(feature = "alloc") { 4 } else { 0 };

thread_local! {
    /// This thread's `bytes`, `allocs`, `freed`, and the largest live bytes
    /// since the innermost open guard (as `i64`).
    static COUNTS: [Cell<u64>; N] = const { [const { Cell::new(0) }; N] };
}

/// Adds `bytes`, `allocs` and `freed` to this thread's counters and raises
/// the high-water mark after an allocation.
#[inline]
fn count(add: [u64; 3]) {
    if cfg!(feature = "alloc") {
        let _ = COUNTS.try_with(|c| {
            for (c, v) in c.iter().zip(add) {
                c.set(c.get().wrapping_add(v));
            }
            if let ([b, _, f, high], true) = (c.as_slice(), add[0] > 0) {
                let live = b.get().wrapping_sub(f.get());
                if live as i64 > high.get() as i64 {
                    high.set(live);
                }
            }
        });
    }
}

/// This thread's counters at a guard's entry, with the high-water mark
/// before it; the mark restarts at the live bytes.
#[inline]
pub(crate) fn open() -> [u64; N] {
    let at = counts();
    let _ = COUNTS.try_with(|c| {
        if let [b, _, f, high] = c.as_slice() {
            high.set(b.get().wrapping_sub(f.get()));
        }
    });
    at
}

/// The differences of `bytes`, `allocs` and `freed` since [`open`] returned
/// `at`, and the peak; the high-water mark before the guard is raised to the
/// one inside it.
#[inline]
pub(crate) fn close(at: &[u64; N]) -> [u64; N] {
    let mut d = counts();
    if let ([b, a, f, high], [b0, a0, f0, high0]) = (&mut d[..], &at[..]) {
        let mark = (*high as i64).max(*high0 as i64) as u64;
        let _ = COUNTS.try_with(|c| c.as_slice().last().map(|h| h.set(mark)));
        *high = high.wrapping_sub(b0.wrapping_sub(*f0));
        (*b, *a, *f) = (
            b.wrapping_sub(*b0),
            a.wrapping_sub(*a0),
            f.wrapping_sub(*f0),
        );
    }
    d
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
