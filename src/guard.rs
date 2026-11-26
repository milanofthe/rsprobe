//! Guards of scopes, contexts and handles, and the handle itself.

use crate::alloc::{close, open, uncounted, COUNTERS};
use crate::context::{Shared, BUSY, BYTES, CALLS, PEAK, POOL, TASKS, WALL, WORKER_CALLS};
use crate::site::{registry, HIGHEST_LIVE_LEVEL};
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
    prev_ctx: u64,
    cur: Cur,
    prev: Cur,
    start: u64,
    emitting_ns_at_start: u64,
    alloc_at_start: [u64; COUNTERS],
    kind: Kind,
    _thread: PhantomData<*const ()>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Scope,
    Enter,
    Switch,
}

impl Guard {
    const IDLE: Guard = Guard {
        prev_ctx: 0,
        cur: IDLE,
        prev: IDLE,
        start: 0,
        emitting_ns_at_start: 0,
        alloc_at_start: [0; COUNTERS],
        kind: Kind::Enter,
        _thread: PhantomData,
    };

    pub(crate) fn enter(shared: Arc<Shared>, node: u32, via_handle: bool) -> Guard {
        let (mut g, released) = uncounted(|| {
            THREAD.with(|t| {
                let t = &mut *t.borrow_mut();
                let i = t.entry(&shared, via_handle);
                let held = |e: &Entry| e.depth == 0 && e.live.is_some();
                let other = t.entries.iter_mut().find(|e| held(e) && e.id != shared.id);
                let released = other.and_then(|e| e.live.take());
                t.entries[i].live.get_or_insert(shared);
                (t.open(i, node, via_handle), released)
            })
        });
        drop(released);
        g.alloc_at_start = open();
        g
    }
}

impl Thread {
    fn open(&mut self, i: usize, node: u32, via_handle: bool) -> Guard {
        let prev = self.cur;
        let prev_ctx = self.entries.get(prev.entry as usize).map_or(0, |p| p.id);
        let e = &mut self.entries[i];
        e.depth += 1;
        e.base(node);
        self.cur = Cur {
            entry: i as u32,
            node,
            via_handle,
        };
        let start = (e.clock)();
        Guard {
            prev_ctx,
            cur: self.cur,
            prev,
            start,
            emitting_ns_at_start: e.emitting_ns,
            ..Guard::IDLE
        }
    }
}

impl Site {
    /// Enters a scope at this site; what [`scope!`](crate::scope) expands to.
    #[inline]
    pub fn scope(&'static self) -> Guard {
        if self.level.0 > HIGHEST_LIVE_LEVEL.load(Relaxed) {
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
                prev_ctx: e.id,
                cur: *cur,
                prev,
                start: (e.clock)(),
                emitting_ns_at_start: e.emitting_ns,
                alloc_at_start: open(),
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
            Kind::Switch => [0; COUNTERS],
            _ => close(&self.alloc_at_start),
        };
        let released = THREAD.with(|t| {
            let Thread { cur, entries } = &mut *t.borrow_mut();
            let e = &mut entries[self.cur.entry as usize];
            let outlived_its_context =
                self.kind != Kind::Enter && (cur.entry != self.cur.entry || e.id != self.prev_ctx);
            if outlived_its_context {
                return None;
            }
            if self.kind == Kind::Switch {
                *cur = self.prev;
                return None;
            }
            let span_ns = (e.clock)().saturating_sub(self.start);
            let emitting_ns = e.emitting_ns - self.emitting_ns_at_start;
            let span_without_emitting_ns = span_ns.saturating_sub(emitting_ns);
            let b = e.base(self.cur.node);
            let scope = self.kind == Kind::Scope;
            let (count, time) = match (scope, self.cur.via_handle) {
                (_, false) => (CALLS, WALL),
                (true, true) => (WORKER_CALLS, BUSY),
                (false, true) => (TASKS, POOL),
            };
            bump(&b[count], (scope || self.cur.via_handle) as u64);
            bump(&b[time], span_without_emitting_ns);
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
            let keeps_handle_context = self.cur.via_handle && !e.shared().closed.load(Relaxed);
            let released = e.live.take_if(|_| e.depth == 0 && !keeps_handle_context);
            let p = entries.get(self.prev.entry as usize);
            let prev_still_entered = p.is_some_and(|p| p.id == self.prev_ctx && p.depth > 0);
            if !prev_still_entered {
                *cur = IDLE;
            }
            released
        });
        drop(released);
    }
}

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
    if cfg!(not(feature = "on")) || HIGHEST_LIVE_LEVEL.load(Relaxed) == 0 {
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
        if cfg!(not(feature = "on")) || self.ctx == 0 || HIGHEST_LIVE_LEVEL.load(Relaxed) == 0 {
            return Guard::IDLE;
        }
        self.enter_live()
    }

    fn enter_live(self) -> Guard {
        let held = THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let Thread { cur, entries } = t;
            if let Some(e) = entries
                .get_mut(cur.entry as usize)
                .filter(|e| e.id == self.ctx)
            {
                let prev = *cur;
                uncounted(|| e.base(self.node));
                cur.node = self.node;
                let kind = Kind::Switch;
                return Ok(Guard {
                    prev_ctx: e.id,
                    cur: *cur,
                    prev,
                    kind,
                    ..Guard::IDLE
                });
            }
            let i = entries.iter().position(|e| e.id == self.ctx).ok_or(None)?;
            let e = &mut entries[i];
            if e.live.as_ref().is_some_and(|s| !s.closed.load(Relaxed)) {
                let mut g = uncounted(|| t.open(i, self.node, true));
                g.alloc_at_start = open();
                return Ok(g);
            }
            Err(e
                .live
                .take_if(|_| e.depth == 0)
                .or_else(|| e.weak.upgrade()))
        });
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
