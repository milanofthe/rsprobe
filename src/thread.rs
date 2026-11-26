//! One thread's state of each context it entered, and the fast path of a site.

use crate::alloc::uncounted;
use crate::context::{Clock, Shared, Tree, BUILTIN};
use crate::site::{lock, HIGHEST_LIVE_LEVEL};
use crate::stream::Item;
use crate::Site;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Weak};

pub(crate) const NONE: u32 = u32::MAX;

pub(crate) type Base = [AtomicU64; BUILTIN.len()];

#[derive(Clone, Copy)]
pub(crate) struct Cur {
    pub(crate) entry: u32,
    pub(crate) node: u32,
    pub(crate) via_handle: bool,
}

pub(crate) const IDLE: Cur = Cur {
    entry: NONE,
    node: 0,
    via_handle: false,
};

pub(crate) struct Thread {
    pub(crate) cur: Cur,
    pub(crate) entries: Vec<Entry>,
}

thread_local! {
    pub(crate) static THREAD: RefCell<Thread> =
        const { RefCell::new(Thread { cur: IDLE, entries: Vec::new() }) };
}

pub(crate) struct Entry {
    pub(crate) id: u64,
    pub(crate) weak: Weak<Shared>,
    pub(crate) live: Option<Arc<Shared>>,
    pub(crate) depth: u32,
    pub(crate) clock: Clock,
    pub(crate) emitting_ns: u64,
    activity: Vec<Activity>,
    pub(crate) thread: u32,
    pub(crate) stream: bool,
    items: Vec<Item>,
    nodes: Vec<Local>,
}

#[derive(Clone, Copy, PartialEq)]
enum Activity {
    Undecided,
    Inactive,
    Active,
}

struct Local {
    base: *const Base,
    children: Vec<(u32, u32)>,
    extras: Vec<(u32, *const AtomicU64)>,
}

#[inline(always)]
pub(crate) fn bump(c: &AtomicU64, v: u64) {
    c.store(c.load(Relaxed).wrapping_add(v), Relaxed);
}

#[inline(always)]
pub(crate) fn raise(c: &AtomicU64, v: u64) {
    if v > c.load(Relaxed) {
        c.store(v, Relaxed);
    }
}

impl Entry {
    pub(crate) fn shared(&self) -> Option<&Arc<Shared>> {
        self.live.as_ref()
    }

    fn cell<T: Default>(&self, keep: impl FnOnce(&mut Tree, Box<T>)) -> Option<*const T> {
        let shared = self.shared()?;
        let b = Box::<T>::default();
        let p: *const T = &*b;
        keep(&mut lock(&shared.tree), b);
        Some(p)
    }

    #[inline]
    pub(crate) fn base(&mut self, node: u32) -> Option<&Base> {
        match self.nodes.get(node as usize) {
            // SAFETY: the context's tree never drops a cell, and a guard on this thread holds the tree.
            Some(l) if !l.base.is_null() => Some(unsafe { &*l.base }),
            _ => self.new_base(node),
        }
    }

    #[cold]
    fn new_base(&mut self, node: u32) -> Option<&Base> {
        let n = node as usize;
        if self.nodes.len() <= n {
            self.nodes.resize_with(n + 1, || Local {
                base: std::ptr::null(),
                children: Vec::new(),
                extras: Vec::new(),
            });
        }
        if self.nodes[n].base.is_null() {
            self.nodes[n].base = self.cell(|t, b| t.bases.push((self.thread, node, b)))?;
        }
        // SAFETY: the context's tree never drops a cell, and a guard on this thread holds the tree.
        Some(unsafe { &*self.nodes[n].base })
    }

    pub(crate) fn child(&mut self, node: u32, site: u32) -> Option<u32> {
        let local = &self.nodes[node as usize];
        match local.children.iter().find(|c| c.0 == site) {
            Some(&(_, c)) => Some(c),
            None => uncounted(|| self.intern(node, site)),
        }
    }

    #[cold]
    fn intern(&mut self, node: u32, site: u32) -> Option<u32> {
        let c = lock(&self.shared()?.tree).intern(node, site);
        self.nodes[node as usize].children.push((site, c));
        self.base(c);
        Some(c)
    }

    fn extra(&mut self, node: u32, site: u32) -> Option<*const AtomicU64> {
        let local = &self.nodes[node as usize];
        match local.extras.iter().find(|e| e.0 == site) {
            Some(&(_, p)) => Some(p),
            None => uncounted(|| self.new_extra(node, site)),
        }
    }

    #[cold]
    #[inline(never)]
    fn new_extra(&mut self, node: u32, site: u32) -> Option<*const AtomicU64> {
        let p = self.cell(|t, b| t.extras.push((self.thread, node, site, b)))?;
        self.nodes[node as usize].extras.push((site, p));
        Some(p)
    }

    #[inline]
    pub(crate) fn push(&mut self, item: Item) {
        uncounted(|| self.items.push(item));
    }

    pub(crate) fn flush(&mut self) {
        if let Some(s) = self.live.as_ref().and_then(|l| l.stream.as_ref()) {
            let t = self.thread;
            uncounted(|| lock(s).items.extend(self.items.drain(..).map(|i| (t, i))));
        }
    }

    #[inline]
    fn active(&mut self, site: &Site, id: u32) -> bool {
        match self.activity.get(id as usize) {
            Some(&a) if a != Activity::Undecided => a == Activity::Active,
            _ => self.decide(site, id as usize),
        }
    }

    #[cold]
    #[inline(never)]
    fn decide(&mut self, site: &Site, id: usize) -> bool {
        let on = self
            .shared()
            .is_some_and(|s| site.level <= s.level(site.module));
        if self.activity.len() <= id {
            uncounted(|| self.activity.resize(id + 1, Activity::Undecided));
        }
        self.activity[id] = if on {
            Activity::Active
        } else {
            Activity::Inactive
        };
        on
    }
}

impl Thread {
    pub(crate) fn entry(&mut self, shared: &Arc<Shared>, via_handle: bool) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.id == shared.id) {
            return i;
        }
        let e = Entry {
            id: shared.id,
            weak: Arc::downgrade(shared),
            live: None,
            depth: 0,
            clock: shared.cfg.clock,
            emitting_ns: 0,
            activity: Vec::new(),
            thread: lock(&shared.tree).thread(via_handle),
            stream: shared.stream.is_some(),
            items: Vec::new(),
            nodes: Vec::new(),
        };
        match self.entries.iter().position(|e| e.weak.strong_count() == 0) {
            Some(i) => {
                self.entries[i] = e;
                i
            }
            None => {
                self.entries.push(e);
                self.entries.len() - 1
            }
        }
    }

    #[inline(always)]
    pub(crate) fn site(&mut self, site: &'static Site) -> Option<(&mut Cur, &mut Entry, u32)> {
        let Thread { cur, entries } = self;
        let e = entries.get_mut(cur.entry as usize)?;
        debug_assert!(e.depth > 0 && e.live.is_some(), "the current entry is held");
        let id = site.id();
        e.active(site, id).then_some((cur, e, id))
    }
}

impl Site {
    #[inline(always)]
    pub(crate) fn active<R>(
        &'static self,
        f: impl FnOnce(&mut Cur, &mut Entry, u32) -> R,
    ) -> Option<R> {
        if self.level.0 > HIGHEST_LIVE_LEVEL.load(Relaxed) {
            return None;
        }
        THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let (cur, e, id) = t.site(self)?;
            Some(f(cur, e, id))
        })
    }

    /// Whether this site is active on this thread now; what
    /// [`active!`](crate::active) expands to.
    #[inline]
    pub fn is_active(&'static self) -> bool {
        self.active(|_, _, _| ()).is_some()
    }

    /// Adds `value()` to this site's measure at the current node; what
    /// [`add!`](crate::add) expands to.
    #[inline]
    pub fn add(&'static self, value: impl FnOnce() -> u64) {
        self.put(value, bump)
    }

    /// Raises this site's measure at the current node to `value()`; what
    /// [`max!`](crate::max) expands to.
    #[inline]
    pub fn max(&'static self, value: impl FnOnce() -> u64) {
        self.put(value, raise)
    }

    #[inline(always)]
    fn put(&'static self, value: impl FnOnce() -> u64, op: fn(&AtomicU64, u64)) {
        let cell = self
            .active(|cur, e, id| Some((e.stream, cur.node, id, e.extra(cur.node, id)?)))
            .flatten();
        if let Some((stream, node, site, c)) = cell {
            let v = value();
            // SAFETY: the context's tree never drops a cell, and a guard on this thread holds the tree.
            let c = unsafe { &*c };
            op(c, v);
            if stream {
                count(site, node, c.load(Relaxed));
            }
        }
    }
}

#[cold]
fn count(site: u32, node: u32, value: u64) {
    THREAD.with(|t| {
        let t = &mut *t.borrow_mut();
        if let Some(e) = t.entries.get_mut(t.cur.entry as usize) {
            let time = (e.clock)();
            e.push(Item::Count {
                site,
                node,
                time,
                value,
            });
        }
    })
}

#[inline(always)]
pub(crate) fn with_current<R>(f: impl FnOnce(&mut Cur, &mut Entry) -> R) -> Option<R> {
    THREAD.with(|t| {
        let Thread { cur, entries } = &mut *t.borrow_mut();
        Some(f(cur, entries.get_mut(cur.entry as usize)?))
    })
}
