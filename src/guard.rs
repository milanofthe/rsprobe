//! Guards of scopes, contexts and handles, and the handle itself.

use crate::alloc::{counts, quiet, N};
use crate::context::{Shared, BUSY, BYTES, CALLS, WALL};
use crate::site::registry;
use crate::stream::Item;
use crate::thread::{bump, with_current, Cur, Thread, IDLE, NONE, THREAD};
use crate::Site;
use std::marker::PhantomData;
use std::sync::Arc;

/// Guard of a scope, of [`Context::enter`](crate::Context::enter) or of
/// [`Handle::enter`]: makes a node current on this thread and restores the
/// previous one on drop, adding its span to `wall` (on a thread entered
/// through the context) or `busy` (through a handle) of that node, and for a
/// scope one call.
#[must_use]
pub struct Guard {
    /// Id of the context the guard belongs to.
    ctx: u64,
    cur: Cur,
    prev: Cur,
    start: u64,
    /// This thread's allocation counters at entry.
    mem: [u64; N],
    scope: bool,
    _thread: PhantomData<*const ()>,
}

impl Guard {
    const IDLE: Guard = Guard {
        ctx: 0,
        cur: IDLE,
        prev: IDLE,
        start: 0,
        mem: [0; N],
        scope: false,
        _thread: PhantomData,
    };

    /// Makes `node` of `shared` current on this thread. The allocations of a
    /// thread's first entry are not counted.
    pub(crate) fn enter(shared: Arc<Shared>, node: u32, busy: bool) -> Guard {
        let mut g = quiet(|| {
            THREAD.with(|t| {
                let t = &mut *t.borrow_mut();
                let i = t.entry(&shared);
                let prev = t.cur;
                let e = &mut t.entries[i];
                let ctx = e.id;
                e.depth += 1;
                e.live.get_or_insert(shared);
                e.base(node);
                let entry = i as u32;
                t.cur = Cur { entry, node, busy };
                let start = (e.clock)();
                Guard {
                    ctx,
                    cur: t.cur,
                    prev,
                    start,
                    ..Guard::IDLE
                }
            })
        });
        g.mem = counts();
        g
    }
}

impl Site {
    /// Enters a scope at this site; what [`scope!`](crate::scope) expands to.
    #[inline]
    pub fn scope(&'static self) -> Guard {
        let guard = self.active(|cur, e, id| {
            let prev = *cur;
            cur.node = e.child(prev.node, id);
            Guard {
                ctx: e.id,
                cur: *cur,
                prev,
                start: (e.clock)(),
                mem: counts(),
                scope: true,
                ..Guard::IDLE
            }
        });
        guard.unwrap_or(Guard::IDLE)
    }
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        if self.cur.entry == NONE {
            return;
        }
        let mem = counts();
        // The context released on the last drop is dropped after the borrow.
        THREAD.with(|t| {
            let Thread { cur, entries } = &mut *t.borrow_mut();
            let e = &mut entries[self.cur.entry as usize];
            // A scope that outlives the guard of its context records nothing.
            if self.scope && (cur.entry != self.cur.entry || e.id != self.ctx) {
                return None;
            }
            let dt = (e.clock)().saturating_sub(self.start);
            let b = e.base(self.cur.node);
            bump(&b[CALLS], self.scope as u64);
            bump(&b[if self.cur.busy { BUSY } else { WALL }], dt);
            for (k, m) in mem.iter().enumerate() {
                bump(&b[BYTES + k], m.wrapping_sub(self.mem[k]));
            }
            if e.stream {
                let (node, start) = (self.cur.node, self.start);
                e.push(Item::Span {
                    node,
                    start,
                    dur: dt,
                });
            }
            *cur = self.prev;
            if self.scope {
                return None;
            }
            e.depth -= 1;
            if e.depth == 0 {
                e.flush();
            }
            e.live.take_if(|_| e.depth == 0)
        });
    }
}

/// A node of a context, `Copy`, to be entered on another thread.
#[derive(Clone, Copy, Debug)]
pub struct Handle {
    ctx: u64,
    node: u32,
}

/// The current node of this thread; a handle to nothing when no context is
/// current.
#[inline]
pub fn current() -> Handle {
    let none = Handle { ctx: 0, node: 0 };
    if cfg!(not(feature = "on")) {
        return none;
    }
    with_current(|cur, e| Handle {
        ctx: e.id,
        node: cur.node,
    })
    .unwrap_or(none)
}

impl Handle {
    /// Makes this node current on this thread until the guard drops. Scopes
    /// entered meanwhile hang under it and add their time to `busy`. Records
    /// nothing if the context has been dropped.
    #[inline]
    pub fn enter(self) -> Guard {
        // No context, or the feature absent: nothing to look up.
        if cfg!(not(feature = "on")) || self.ctx == 0 {
            return Guard::IDLE;
        }
        // A context this thread has entered is looked up without a lock.
        let local = THREAD.with(|t| {
            let t = t.borrow();
            let e = t.entries.iter().find(|e| e.id == self.ctx);
            e.map(|e| e.weak.upgrade())
        });
        match local.unwrap_or_else(|| registry().find(self.ctx)) {
            Some(s) => Guard::enter(s, self.node, true),
            None => Guard::IDLE,
        }
    }
}
