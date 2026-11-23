//! One thread's state of each context it entered, and the fast path of a site.

use crate::alloc::quiet;
use crate::context::{Clock, Shared, Tree};
use crate::site::{lock, MAX};
use crate::stream::Item;
use crate::Site;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Weak};

/// Marks no entry, no parent, no site.
pub(crate) const NONE: u32 = u32::MAX;

/// One thread's built-in measures of one node, in the order of
/// [`BUILTIN`](crate::context::BUILTIN).
pub(crate) type Base = [AtomicU64; 6];

/// The current node of a thread.
#[derive(Clone, Copy)]
pub(crate) struct Cur {
    /// Index of the entry in [`Thread::entries`]; [`NONE`] for no context.
    pub(crate) entry: u32,
    pub(crate) node: u32,
    /// Entered through a handle: time adds to `busy`, else to `wall`.
    pub(crate) busy: bool,
}

pub(crate) const IDLE: Cur = Cur {
    entry: NONE,
    node: 0,
    busy: false,
};

/// Every context this thread entered, and the current node. `cur.entry` is
/// [`NONE`] or an entry with `depth > 0`.
pub(crate) struct Thread {
    pub(crate) cur: Cur,
    pub(crate) entries: Vec<Entry>,
}

thread_local! {
    pub(crate) static THREAD: RefCell<Thread> =
        const { RefCell::new(Thread { cur: IDLE, entries: Vec::new() }) };
}

/// This thread's state of one context.
pub(crate) struct Entry {
    pub(crate) id: u64,
    pub(crate) weak: Weak<Shared>,
    /// Held while a guard of this context is alive on this thread.
    pub(crate) live: Option<Arc<Shared>>,
    /// Guards of [`Context::enter`](crate::Context::enter) and
    /// [`Handle::enter`] alive on this thread.
    pub(crate) depth: u32,
    pub(crate) clock: Clock,
    /// By site id: 0 undecided, 1 inactive, 2 active in this context.
    active: Vec<u8>,
    /// Index of this thread among those that entered the context.
    pub(crate) thread: u32,
    /// Whether the context keeps a stream, and this thread's items not yet
    /// flushed to it.
    pub(crate) stream: bool,
    items: Vec<Item>,
    /// This thread's cells and child cache, by node id.
    nodes: Vec<Local>,
}

/// This thread's cells of one node and the children it has seen.
struct Local {
    /// Null until this thread uses the node.
    base: *const Base,
    /// (scope site, child node).
    children: Vec<(u32, u32)>,
    /// (`add!` site, cell).
    extras: Vec<(u32, *const AtomicU64)>,
}

#[inline(always)]
pub(crate) fn bump(c: &AtomicU64, v: u64) {
    c.store(c.load(Relaxed).wrapping_add(v), Relaxed);
}

impl Entry {
    /// The shared state, alive while `depth > 0`.
    pub(crate) fn shared(&self) -> &Arc<Shared> {
        self.live.as_ref().expect("entry is current")
    }

    /// A new cell kept by the tree of the shared state, which never drops
    /// one. A reference to it is valid while a guard of the context is alive
    /// on this thread.
    fn cell<T: Default>(&self, keep: impl FnOnce(&mut Tree, Box<T>)) -> *const T {
        let b = Box::<T>::default();
        let p: *const T = &*b;
        keep(&mut lock(&self.shared().tree), b);
        p
    }

    /// This thread's base cells of `node`, allocated on first use.
    #[inline]
    pub(crate) fn base(&mut self, node: u32) -> &Base {
        match self.nodes.get(node as usize) {
            // SAFETY: as in `cell`.
            Some(l) if !l.base.is_null() => unsafe { &*l.base },
            _ => self.new_base(node),
        }
    }

    #[cold]
    fn new_base(&mut self, node: u32) -> &Base {
        let n = node as usize;
        if self.nodes.len() <= n {
            self.nodes.resize_with(n + 1, || Local {
                base: std::ptr::null(),
                children: Vec::new(),
                extras: Vec::new(),
            });
        }
        if self.nodes[n].base.is_null() {
            self.nodes[n].base = self.cell(|t, b| t.bases.push((node, b)));
        }
        // SAFETY: as in `cell`.
        unsafe { &*self.nodes[n].base }
    }

    /// The child of `node` for a scope site, interned on first use.
    pub(crate) fn child(&mut self, node: u32, site: u32) -> u32 {
        let local = &self.nodes[node as usize];
        match local.children.iter().find(|c| c.0 == site) {
            Some(&(_, c)) => c,
            None => quiet(|| self.intern(node, site)),
        }
    }

    #[cold]
    fn intern(&mut self, node: u32, site: u32) -> u32 {
        let c = lock(&self.shared().tree).intern(node, site);
        self.nodes[node as usize].children.push((site, c));
        self.base(c);
        c
    }

    /// This thread's cell of an `add!` site at `node`, allocated on first use.
    fn extra(&mut self, node: u32, site: u32) -> *const AtomicU64 {
        let local = &self.nodes[node as usize];
        match local.extras.iter().find(|e| e.0 == site) {
            Some(&(_, p)) => p,
            None => quiet(|| self.new_extra(node, site)),
        }
    }

    #[cold]
    #[inline(never)]
    fn new_extra(&mut self, node: u32, site: u32) -> *const AtomicU64 {
        let p = self.cell(|t, b| t.extras.push((node, site, b)));
        self.nodes[node as usize].extras.push((site, p));
        p
    }

    /// Appends an item to this thread's stream items.
    #[inline]
    pub(crate) fn push(&mut self, item: Item) {
        quiet(|| self.items.push(item));
    }

    /// Moves this thread's stream items to the context's stream.
    pub(crate) fn flush(&mut self) {
        if let Some(s) = self.live.as_ref().and_then(|l| l.stream.as_ref()) {
            let t = self.thread;
            quiet(|| lock(s).items.extend(self.items.drain(..).map(|i| (t, i))));
        }
    }

    /// Whether `site` is active in this context, decided on first use.
    #[inline]
    fn active(&mut self, site: &Site, id: u32) -> bool {
        match self.active.get(id as usize) {
            Some(&d) if d != 0 => d == 2,
            _ => self.decide(site, id as usize),
        }
    }

    #[cold]
    #[inline(never)]
    fn decide(&mut self, site: &Site, id: usize) -> bool {
        let on = site.level <= self.shared().level(site.module);
        if self.active.len() <= id {
            quiet(|| self.active.resize(id + 1, 0));
        }
        self.active[id] = 1 + on as u8;
        on
    }
}

impl Thread {
    /// Index of this thread's entry of `shared`, created on first use in the
    /// slot of a dropped context or appended.
    pub(crate) fn entry(&mut self, shared: &Arc<Shared>) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.id == shared.id) {
            return i;
        }
        let e = Entry {
            id: shared.id,
            weak: Arc::downgrade(shared),
            live: None,
            depth: 0,
            clock: shared.cfg.clock,
            active: Vec::new(),
            thread: lock(&shared.tree).thread(),
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

    /// The current node, the current entry and the site id if `site` is
    /// active in the current context.
    #[inline(always)]
    pub(crate) fn site(&mut self, site: &'static Site) -> Option<(&mut Cur, &mut Entry, u32)> {
        let Thread { cur, entries } = self;
        let e = entries.get_mut(cur.entry as usize)?;
        let id = site.id();
        e.active(site, id).then_some((cur, e, id))
    }
}

impl Site {
    /// Runs `f` on the current node, the current entry and the site id if
    /// this site is active in the current context.
    #[inline(always)]
    pub(crate) fn active<R>(
        &'static self,
        f: impl FnOnce(&mut Cur, &mut Entry, u32) -> R,
    ) -> Option<R> {
        if self.level.0 > MAX.load(Relaxed) {
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
        let cell = self.active(|cur, e, id| (e.stream, cur.node, id, e.extra(cur.node, id)));
        if let Some((stream, node, site, c)) = cell {
            // The value is evaluated outside the borrow, so it may enter
            // scopes itself.
            let v = value();
            // SAFETY: as in `Entry::cell`; the guard that made the entry
            // current outlives this call.
            let c = unsafe { &*c };
            bump(c, v);
            if stream {
                count(site, node, c.load(Relaxed));
            }
        }
    }
}

/// Appends the value of an `add!` site to this thread's stream items.
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

/// Runs `f` on the current node and entry if a context is current.
#[inline(always)]
pub(crate) fn with_current<R>(f: impl FnOnce(&mut Cur, &mut Entry) -> R) -> Option<R> {
    THREAD.with(|t| {
        let Thread { cur, entries } = &mut *t.borrow_mut();
        Some(f(cur, entries.get_mut(cur.entry as usize)?))
    })
}
