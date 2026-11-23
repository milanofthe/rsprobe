//! Guards of scopes, contexts and handles, and the handle itself.

use crate::alloc::{counts, quiet, N};
use crate::context::{Shared, BUSY, BYTES, CALLS, WALL};
use crate::site::{registry, MAX};
use crate::stream::Item;
use crate::thread::{bump, with_current, Cur, Entry, Thread, IDLE, NONE, THREAD};
use crate::Site;
use std::marker::PhantomData;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

/// Guard of a scope, of [`Context::enter`](crate::Context::enter) or of
/// [`Handle::enter`]: makes a node current on this thread and restores the
/// previous one on drop, adding its span to `wall` (on a thread entered
/// through the context) or `busy` (through a handle) of that node, and for a
/// scope one call.
#[must_use]
pub struct Guard {
    /// Id of the context of `prev`, which for a scope is its own.
    ctx: u64,
    cur: Cur,
    prev: Cur,
    start: u64,
    /// This thread's allocation counters at entry.
    mem: [u64; N],
    kind: Kind,
    _thread: PhantomData<*const ()>,
}

/// What a guard records on drop.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// A scope: one call and its span.
    Scope,
    /// A context or a handle entered on a thread: its span.
    Enter,
    /// A handle entered where its context is current already: nothing, the
    /// span lies inside one this thread is timing.
    Switch,
}

impl Guard {
    const IDLE: Guard = Guard {
        ctx: 0,
        cur: IDLE,
        prev: IDLE,
        start: 0,
        mem: [0; N],
        kind: Kind::Enter,
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
                let ctx = t.entries.get(prev.entry as usize).map_or(0, |p| p.id);
                let e = &mut t.entries[i];
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
        if self.level.0 > MAX.load(Relaxed) {
            return Guard::IDLE;
        }
        THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let Some((cur, e, id)) = t.site(self) else {
                return Guard::IDLE;
            };
            let prev = *cur;
            cur.node = e.child(prev.node, id);
            Guard {
                ctx: e.id,
                cur: *cur,
                prev,
                start: (e.clock)(),
                mem: counts(),
                kind: Kind::Scope,
                ..Guard::IDLE
            }
        })
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
            // A scope or a switch that outlives the guard of its context
            // records nothing.
            let inner = self.kind != Kind::Enter;
            if inner && (cur.entry != self.cur.entry || e.id != self.ctx) {
                return None;
            }
            if self.kind == Kind::Switch {
                *cur = self.prev;
                return None;
            }
            let dt = (e.clock)().saturating_sub(self.start);
            let b = e.base(self.cur.node);
            bump(&b[CALLS], (self.kind == Kind::Scope) as u64);
            bump(&b[if self.cur.busy { BUSY } else { WALL }], dt);
            for (k, m) in mem.iter().enumerate() {
                bump(&b[BYTES + k], m.wrapping_sub(self.mem[k]));
            }
            if e.stream {
                span(e, self.cur.node, self.start, dt);
            }
            *cur = self.prev;
            if self.kind == Kind::Scope {
                return None;
            }
            e.depth -= 1;
            if e.depth == 0 {
                e.flush();
            }
            let released = e.live.take_if(|_| e.depth == 0);
            // Dropped out of order, the previous node may belong to a
            // context no longer entered on this thread.
            let p = entries.get(self.prev.entry as usize);
            if !p.is_some_and(|p| p.id == self.ctx && p.depth > 0) {
                *cur = IDLE;
            }
            released
        });
    }
}

/// Appends a guard's span to this thread's stream items.
#[cold]
#[inline(never)]
fn span(e: &mut Entry, node: u32, start: u64, dur: u64) {
    e.push(Item::Span { node, start, dur });
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
    /// Makes this node current on this thread until the guard drops; scopes
    /// entered meanwhile hang under it. On a thread where the handle's
    /// context is not current (a pool's worker) their time and the guard's
    /// own span are `busy` of their nodes. Where it is current already (the
    /// thread that issued the work runs it itself) only the node changes:
    /// the time stays in the measure this thread records and the guard adds
    /// no span, as an open guard of this thread covers it. Records nothing
    /// if the context has been dropped.
    #[inline]
    pub fn enter(self) -> Guard {
        // No context, or the feature absent: nothing to look up.
        if cfg!(not(feature = "on")) || self.ctx == 0 {
            return Guard::IDLE;
        }
        let switched = THREAD.with(|t| {
            let Thread { cur, entries } = &mut *t.borrow_mut();
            let e = entries.get_mut(cur.entry as usize)?;
            if e.id != self.ctx {
                return None;
            }
            let prev = *cur;
            quiet(|| {
                e.base(self.node);
            });
            cur.node = self.node;
            Some(Guard {
                ctx: e.id,
                cur: *cur,
                prev,
                kind: Kind::Switch,
                ..Guard::IDLE
            })
        });
        if let Some(g) = switched {
            return g;
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
