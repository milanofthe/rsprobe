//! Guards of scopes, contexts and handles, and the handle itself.

use crate::alloc::{close, open, quiet, N};
use crate::context::{Shared, BUSY, BYTES, CALLS, PEAK, POOL, TASKS, WALL, WORKER_CALLS};
use crate::site::{registry, MAX};
use crate::stream::Item;
use crate::thread::{bump, raise, with_current, Cur, Entry, Thread, IDLE, NONE, THREAD};
use crate::Site;
use std::marker::PhantomData;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

/// Guard of a scope, of [`Context::enter`](crate::Context::enter) or of
/// [`Handle::enter`]: makes a node current on this thread and restores the
/// previous one on drop. On a thread entered through the context a scope adds
/// one call and its span to `calls` and `wall` of its node, the guard of the
/// context its span to `wall` of the root. On a thread entered through a
/// handle a scope adds to `worker_calls` (part of `calls` in the record) and
/// `busy`, the handle's guard one task and its span to `tasks` and `pool` of
/// the handle's node. The time this thread spent emitting events meanwhile is
/// not part of the span added; it is the measure `emit` of the event's node.
#[must_use]
pub struct Guard {
    /// Id of the context of `prev`, which for a scope is its own.
    ctx: u64,
    cur: Cur,
    prev: Cur,
    start: u64,
    /// [`Entry::out`] at entry.
    out: u64,
    /// This thread's allocation counters at entry, from [`open`].
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
        out: 0,
        mem: [0; N],
        kind: Kind::Enter,
        _thread: PhantomData,
    };

    /// Makes `node` of `shared` current on this thread. The other context
    /// this thread holds without a guard, if any, is released after the
    /// borrow. The allocations of a thread's first entry are not counted.
    pub(crate) fn enter(shared: Arc<Shared>, node: u32, busy: bool) -> Guard {
        let mut g = quiet(|| {
            THREAD
                .with(|t| {
                    let t = &mut *t.borrow_mut();
                    let i = t.entry(&shared, busy);
                    let held = |e: &Entry| e.depth == 0 && e.live.is_some();
                    let other = t.entries.iter_mut().find(|e| held(e) && e.id != shared.id);
                    let released = other.and_then(|e| e.live.take());
                    t.entries[i].live.get_or_insert(shared);
                    (t.open(i, node, busy), released)
                })
                .0
        });
        g.mem = open();
        g
    }
}

impl Thread {
    /// Makes `node` of entry `i`, whose context is held, current.
    fn open(&mut self, i: usize, node: u32, busy: bool) -> Guard {
        let prev = self.cur;
        let ctx = self.entries.get(prev.entry as usize).map_or(0, |p| p.id);
        let e = &mut self.entries[i];
        e.depth += 1;
        e.base(node);
        self.cur = Cur {
            entry: i as u32,
            node,
            busy,
        };
        let start = (e.clock)();
        Guard {
            ctx,
            cur: self.cur,
            prev,
            start,
            out: e.out,
            ..Guard::IDLE
        }
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
                out: e.out,
                mem: open(),
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
        let mem = match self.kind {
            Kind::Switch => [0; N],
            _ => close(&self.mem),
        };
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
            // The span as the clock gives it, and without the time this
            // thread spent emitting events meanwhile.
            let span_ns = (e.clock)().saturating_sub(self.start);
            let dt = span_ns.saturating_sub(e.out - self.out);
            let b = e.base(self.cur.node);
            let scope = self.kind == Kind::Scope;
            let (count, time) = match (scope, self.cur.busy) {
                (_, false) => (CALLS, WALL),
                (true, true) => (WORKER_CALLS, BUSY),
                (false, true) => (TASKS, POOL),
            };
            bump(&b[count], (scope || self.cur.busy) as u64);
            bump(&b[time], dt);
            for (k, &m) in mem.iter().enumerate() {
                let op = if BYTES + k == PEAK { raise } else { bump };
                op(&b[BYTES + k], m);
            }
            if e.stream {
                span(e, self.cur.node, self.start, span_ns);
            }
            *cur = self.prev;
            if self.kind == Kind::Scope {
                return None;
            }
            e.depth -= 1;
            if e.depth == 0 {
                e.flush();
            }
            // A thread keeps a context entered through a handle.
            let keep = self.cur.busy && !e.shared().closed.load(Relaxed);
            let released = e.live.take_if(|_| e.depth == 0 && !keep);
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
    if cfg!(not(feature = "on")) || MAX.load(Relaxed) == 0 {
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
    /// context is not current (a pool's worker) their calls and time are
    /// `worker_calls` and `busy` of their nodes, and the guard adds one task
    /// and its span to `tasks` and `pool` of this node. Where it is current
    /// already (the thread that issued the work runs it itself) only the node
    /// changes: the time stays in the measure this thread records and the
    /// guard adds no span and no task, as an open guard of this thread covers
    /// it. Records nothing if the context has been dropped.
    ///
    /// A thread holds the context it entered through a handle after the
    /// guard drops, so entering it again writes no state shared with other
    /// threads; it lets go when it enters another context or ends, or enters
    /// this one after the [`Context`](crate::Context) dropped. A context's
    /// tree is freed when the last thread that holds it lets go.
    #[inline]
    pub fn enter(self) -> Guard {
        // No context, or the feature absent: nothing to look up.
        if cfg!(not(feature = "on")) || self.ctx == 0 || MAX.load(Relaxed) == 0 {
            return Guard::IDLE;
        }
        self.held()
    }

    /// [`enter`](Self::enter) where a context is live.
    fn held(self) -> Guard {
        let held = THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let Thread { cur, entries } = t;
            if let Some(e) = entries
                .get_mut(cur.entry as usize)
                .filter(|e| e.id == self.ctx)
            {
                let prev = *cur;
                quiet(|| e.base(self.node));
                cur.node = self.node;
                let kind = Kind::Switch;
                return Ok(Guard {
                    ctx: e.id,
                    cur: *cur,
                    prev,
                    kind,
                    ..Guard::IDLE
                });
            }
            let i = entries.iter().position(|e| e.id == self.ctx).ok_or(None)?;
            let e = &mut entries[i];
            if e.live.as_ref().is_some_and(|s| !s.closed.load(Relaxed)) {
                let mut g = quiet(|| t.open(i, self.node, true));
                g.mem = open();
                return Ok(g);
            }
            // Held after its context dropped: released after the borrow.
            Err(e
                .live
                .take_if(|_| e.depth == 0)
                .or_else(|| e.weak.upgrade()))
        });
        // Not held: a context this thread has entered is looked up without
        // a lock.
        let shared = match held {
            Ok(g) => return g,
            Err(s) => s.or_else(|| registry().find(self.ctx)),
        };
        match shared.filter(|s| !s.closed.load(Relaxed)) {
            Some(s) => Guard::enter(s, self.node, true),
            None => Guard::IDLE,
        }
    }
}
