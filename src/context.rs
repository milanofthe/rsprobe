//! The context, the thread-local state and the guards.

use crate::record::{Calibration, Node, Param, Record};
use crate::{Level, Site};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// A clock: nanoseconds since an arbitrary fixed origin, non-decreasing.
pub type Clock = fn() -> u64;

/// Marks no entry, no parent, no site.
const NONE: u32 = u32::MAX;

/// Built-in measures; user measures follow with ids from 3.
const BUILTIN: [(&str, bool); 3] = [("calls", true), ("wall", false), ("busy", false)];
const CALLS: usize = 0;
const WALL: usize = 1;
const BUSY: usize = 2;

/// One thread's `calls`, `wall` and `busy` of one node.
type Base = [AtomicU64; 3];

/// Process-wide: the call sites, the user measures and the live contexts.
struct Registry {
    sites: Vec<&'static Site>,
    measures: Vec<&'static str>,
    contexts: Vec<(u64, Level, Weak<Shared>)>,
    next: u64,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    sites: Vec::new(),
    measures: Vec::new(),
    contexts: Vec::new(),
    next: 1,
});

/// Highest threshold of all live contexts; 0 when there is none.
static MAX: AtomicU8 = AtomicU8::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers a site as a scope site or as a measure (by name) and returns its id.
#[cold]
pub(crate) fn register(site: &'static Site, measure: bool) -> u32 {
    let mut r = lock(&REGISTRY);
    let id = match site.id.load(Relaxed) {
        0 if measure => {
            BUILTIN.len()
                + match r.measures.iter().position(|&m| m == site.name) {
                    Some(i) => i,
                    None => {
                        r.measures.push(site.name);
                        r.measures.len() - 1
                    }
                }
        }
        0 => {
            r.sites.push(site);
            r.sites.len() - 1
        }
        id => return id - 1,
    } as u32;
    site.id.store(id + 1, Relaxed);
    id
}

/// The default clock: `std::time::Instant` since the first read in the process.
#[cfg(not(target_arch = "wasm32"))]
pub fn now() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as u64
}

/// The default clock on wasm32: none, every time is zero. The host passes its
/// own clock to [`Context::with`].
#[cfg(target_arch = "wasm32")]
pub fn now() -> u64 {
    0
}

/// Shared by a context, its guards and the threads that entered it.
struct Shared {
    id: u64,
    threshold: Level,
    clock: Clock,
    tree: Mutex<Tree>,
}

/// The nodes and every thread's cells of one context.
struct Tree {
    /// (parent, site) per node id; the root is (NONE, NONE).
    keys: Vec<(u32, u32)>,
    index: BTreeMap<(u32, u32), u32>,
    bases: Vec<(u32, Box<Base>)>,
    /// (node, measure, value) of user measures.
    extras: Vec<(u32, u32, Box<AtomicU64>)>,
    threads: u32,
    params: Vec<(String, Param)>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let mut r = lock(&REGISTRY);
        r.contexts.retain(|c| c.0 != self.id);
        MAX.store(
            r.contexts.iter().map(|c| c.1 .0).max().unwrap_or(0),
            Relaxed,
        );
    }
}

/// This thread's state of one context.
struct Entry {
    id: u64,
    weak: Weak<Shared>,
    /// Held while a guard of this context is alive on this thread.
    live: Option<Arc<Shared>>,
    depth: u32,
    clock: Clock,
    threshold: Level,
    /// This thread's cells and child cache, by node id.
    nodes: Vec<Local>,
}

/// This thread's cells of one node and the children it has seen.
struct Local {
    base: *const Base,
    children: Vec<(u32, u32)>,
    extras: Vec<(u32, *const AtomicU64)>,
}

#[derive(Clone, Copy)]
struct Cur {
    entry: u32,
    node: u32,
    busy: bool,
}

const IDLE: Cur = Cur {
    entry: NONE,
    node: 0,
    busy: false,
};

struct Thread {
    cur: Cur,
    entries: Vec<Entry>,
}

thread_local! {
    static THREAD: RefCell<Thread> = const { RefCell::new(Thread { cur: IDLE, entries: Vec::new() }) };
}

fn bump(c: &AtomicU64, v: u64) {
    c.store(c.load(Relaxed).wrapping_add(v), Relaxed);
}

impl Entry {
    /// The shared state; alive while `depth > 0`, which holds wherever the
    /// entry is current.
    fn shared(&self) -> &Shared {
        self.live.as_deref().expect("entry is current")
    }

    /// This thread's base cells of `node`, allocated on first use.
    fn base(&mut self, node: u32) -> &Base {
        let n = node as usize;
        if self.nodes.len() <= n {
            self.nodes.resize_with(n + 1, || Local {
                base: std::ptr::null(),
                children: Vec::new(),
                extras: Vec::new(),
            });
        }
        if self.nodes[n].base.is_null() {
            let b = Box::<Base>::default();
            self.nodes[n].base = &*b;
            lock(&self.shared().tree).bases.push((node, b));
        }
        // SAFETY: the box is owned by the tree of the shared state, which
        // never drops a cell and is alive while the entry is current.
        unsafe { &*self.nodes[n].base }
    }

    /// The child of `node` for a scope site, interned on first use.
    fn child(&mut self, node: u32, site: u32) -> u32 {
        let local = &self.nodes[node as usize];
        if let Some(&(_, c)) = local.children.iter().find(|c| c.0 == site) {
            return c;
        }
        let c = {
            let mut t = lock(&self.shared().tree);
            let next = t.keys.len() as u32;
            let c = *t.index.entry((node, site)).or_insert(next);
            if c == next {
                t.keys.push((node, site));
            }
            c
        };
        self.nodes[node as usize].children.push((site, c));
        self.base(c);
        c
    }

    /// This thread's cell of a user measure at `node`, allocated on first use.
    fn extra(&mut self, node: u32, measure: u32) -> &AtomicU64 {
        let local = &self.nodes[node as usize];
        let p = match local.extras.iter().find(|e| e.0 == measure) {
            Some(&(_, p)) => p,
            None => {
                let b = Box::<AtomicU64>::default();
                let p: *const AtomicU64 = &*b;
                lock(&self.shared().tree).extras.push((node, measure, b));
                self.nodes[node as usize].extras.push((measure, p));
                p
            }
        };
        // SAFETY: as in `base`.
        unsafe { &*p }
    }
}

impl Thread {
    /// Index of this thread's entry of `shared`, created on first use in the
    /// slot of a dropped context or appended.
    fn entry(&mut self, shared: &Arc<Shared>) -> usize {
        if let Some(i) = self.entries.iter().position(|e| e.id == shared.id) {
            return i;
        }
        lock(&shared.tree).threads += 1;
        let e = Entry {
            id: shared.id,
            weak: Arc::downgrade(shared),
            live: None,
            depth: 0,
            clock: shared.clock,
            threshold: shared.threshold,
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

    /// The current entry if `site` is active in it.
    fn active(&mut self, site: &Site) -> Option<&mut Entry> {
        let e = self.entries.get_mut(self.cur.entry as usize)?;
        (site.level <= e.threshold).then_some(e)
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
            let prev = t.cur;
            let Some(e) = t.active(self) else {
                return Guard::IDLE;
            };
            let node = e.child(prev.node, self.id(false));
            let (ctx, start) = (e.id, (e.clock)());
            t.cur.node = node;
            Guard {
                ctx,
                cur: t.cur,
                prev,
                start,
                scope: true,
                _thread: PhantomData,
            }
        })
    }

    /// Adds `value()` to this site's measure at the current node; what
    /// [`add!`](crate::add) expands to.
    #[inline]
    pub fn add(&'static self, value: impl FnOnce() -> u64) {
        if self.level.0 > MAX.load(Relaxed) {
            return;
        }
        let cell = THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let node = t.cur.node;
            let e = t.active(self)?;
            Some(e.extra(node, self.id(true)) as *const AtomicU64)
        });
        if let Some(c) = cell {
            // The value is evaluated outside the borrow, so it may enter
            // scopes itself.
            let v = value();
            // SAFETY: as in `Entry::base`; the guard that made the entry
            // current outlives this call.
            bump(unsafe { &*c }, v);
        }
    }
}

/// Guard of a scope, of [`Context::enter`] or of [`Handle::enter`]: makes a
/// node current on this thread and restores the previous one on drop, adding
/// its span to `wall` (on a thread entered through the context) or `busy`
/// (through a handle) of that node, and for a scope one call.
#[must_use]
pub struct Guard {
    /// Id of the context the guard belongs to.
    ctx: u64,
    cur: Cur,
    prev: Cur,
    start: u64,
    scope: bool,
    _thread: PhantomData<*const ()>,
}

impl Guard {
    const IDLE: Guard = Guard {
        ctx: 0,
        cur: IDLE,
        prev: IDLE,
        start: 0,
        scope: false,
        _thread: PhantomData,
    };

    fn enter(shared: Arc<Shared>, node: u32, busy: bool) -> Guard {
        THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            let i = t.entry(&shared);
            let e = &mut t.entries[i];
            e.depth += 1;
            e.live.get_or_insert(shared);
            e.base(node);
            let cur = Cur {
                entry: i as u32,
                node,
                busy,
            };
            let prev = std::mem::replace(&mut t.cur, cur);
            Guard {
                ctx: e.id,
                cur,
                prev,
                start: (e.clock)(),
                scope: false,
                _thread: PhantomData,
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
        let released = THREAD.with(|t| {
            let t = &mut *t.borrow_mut();
            // A scope that outlives the guard of its context records nothing.
            let e = &mut t.entries[self.cur.entry as usize];
            if self.scope && (t.cur.entry != self.cur.entry || e.id != self.ctx) {
                return None;
            }
            let dt = (e.clock)().saturating_sub(self.start);
            let b = e.base(self.cur.node);
            bump(&b[CALLS], self.scope as u64);
            bump(&b[if self.cur.busy { BUSY } else { WALL }], dt);
            t.cur = self.prev;
            if self.scope {
                return None;
            }
            e.depth -= 1;
            e.live.take_if(|_| e.depth == 0)
        });
        drop(released);
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
pub fn current() -> Handle {
    THREAD.with(|t| {
        let t = t.borrow();
        match t.entries.get(t.cur.entry as usize) {
            Some(e) => Handle {
                ctx: e.id,
                node: t.cur.node,
            },
            None => Handle { ctx: 0, node: 0 },
        }
    })
}

impl Handle {
    /// Makes this node current on this thread until the guard drops. Scopes
    /// entered meanwhile hang under it and add their time to `busy`. Records
    /// nothing if the context has been dropped.
    pub fn enter(self) -> Guard {
        let local = THREAD.with(|t| {
            t.borrow()
                .entries
                .iter()
                .find(|e| e.id == self.ctx)?
                .weak
                .upgrade()
        });
        let shared = local.or_else(|| {
            let r = lock(&REGISTRY);
            r.contexts.iter().find(|c| c.0 == self.ctx)?.2.upgrade()
        });
        match shared {
            Some(s) => Guard::enter(s, self.node, true),
            None => Guard::IDLE,
        }
    }
}

/// The measurement state of one problem: its tree, every thread's values,
/// its parameters and the calibration constants.
pub struct Context {
    shared: Arc<Shared>,
}

impl Default for Context {
    fn default() -> Context {
        Context::new()
    }
}

impl Context {
    /// A context at threshold INFO with the default clock [`now`].
    pub fn new() -> Context {
        Context::with(Level::INFO, now)
    }

    /// A context with a threshold and a clock.
    pub fn with(threshold: Level, clock: Clock) -> Context {
        let mut r = lock(&REGISTRY);
        let id = r.next;
        r.next += 1;
        let shared = Arc::new(Shared {
            id,
            threshold,
            clock,
            tree: Mutex::new(Tree {
                keys: vec![(NONE, NONE)],
                index: BTreeMap::new(),
                bases: Vec::new(),
                extras: Vec::new(),
                threads: 0,
                params: Vec::new(),
            }),
        });
        r.contexts.push((id, threshold, Arc::downgrade(&shared)));
        MAX.fetch_max(threshold.0, Relaxed);
        Context { shared }
    }

    /// Makes the root current on this thread until the guard drops. Scopes
    /// entered meanwhile add their time to `wall`.
    pub fn enter(&self) -> Guard {
        Guard::enter(self.shared.clone(), 0, false)
    }

    /// Sets a parameter of the run (a number or a string), replacing one of
    /// the same name.
    pub fn param(&self, name: &str, value: impl Into<Param>) {
        let params = &mut lock(&self.shared.tree).params;
        params.retain(|p| p.0 != name);
        params.push((name.to_string(), value.into()));
    }

    /// The tree with every thread's values summed per node and measure. Spans
    /// of guards and scopes still open are not included. With the feature
    /// `on` the scope cost is calibrated here, with this context's clock and
    /// the processor in the state the run left it in.
    pub fn record(&self) -> Record {
        #[cfg(feature = "on")]
        let calibration = calibrate(self.shared.clock);
        #[cfg(not(feature = "on"))]
        let calibration = Calibration::default();
        self.sums(calibration)
    }

    /// [`record`](Self::record) with the given calibration constants.
    fn sums(&self, calibration: Calibration) -> Record {
        let t = lock(&self.shared.tree);
        let r = lock(&REGISTRY);
        let mut nodes: Vec<Node> = t
            .keys
            .iter()
            .map(|&(p, s)| Node {
                parent: (p != NONE).then_some(p),
                site: (s != NONE).then_some(s),
                values: BUILTIN.iter().map(|m| (m.0, 0)).collect(),
            })
            .collect();
        for (n, b) in &t.bases {
            for (v, c) in nodes[*n as usize].values.iter_mut().zip(b.iter()) {
                v.1 += c.load(Relaxed);
            }
        }
        let mut measures: Vec<(&'static str, bool)> = BUILTIN.to_vec();
        for (n, m, c) in &t.extras {
            let name = r.measures[*m as usize - BUILTIN.len()];
            let values = &mut nodes[*n as usize].values;
            match values.iter_mut().find(|v| v.0 == name) {
                Some(v) => v.1 += c.load(Relaxed),
                None => values.push((name, c.load(Relaxed))),
            }
            if !measures.iter().any(|m| m.0 == name) {
                measures.push((name, true));
            }
        }
        let mut sites: Vec<u32> = t.keys[1..].iter().map(|k| k.1).collect();
        sites.sort_unstable();
        sites.dedup();
        Record {
            params: t.params.clone(),
            calibration,
            threads: t.threads,
            measures,
            sites: sites
                .into_iter()
                .map(|s| (s, r.sites[s as usize]))
                .collect(),
            nodes,
        }
    }
}

/// Times empty scopes in a context of its own: the mean elapsed time an empty
/// scope records (inner) and the mean cost per scope its parent sees beyond
/// that (outer).
#[cfg(feature = "on")]
fn calibrate(clock: Clock) -> Calibration {
    let ctx = Context::with(Level::TRACE, clock);
    let _on = ctx.enter();
    let mut walls = [(0, 0); 2];
    for w in &mut walls {
        {
            let _parent = crate::scope!(ERROR, "calibration");
            for _ in 0..crate::CALIBRATION_SCOPES {
                let _s = crate::scope!(ERROR, "empty");
            }
        }
        let r = ctx.sums(Calibration::default());
        *w = (r.nodes[1].get("wall"), r.nodes[2].get("wall"));
    }
    let n = crate::CALIBRATION_SCOPES as f64;
    let inner = (walls[1].1 - walls[0].1) as f64 / n;
    let total = (walls[1].0 - walls[0].0) as f64 / n;
    Calibration {
        inner_ns: inner,
        outer_ns: total - inner,
    }
}
