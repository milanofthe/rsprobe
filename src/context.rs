//! The context, the thread-local state and the guards.

use crate::alloc::{counts, quiet, N};
use crate::events::Warnings;
use crate::record::{Calibration, Node, Param, Record};
use crate::{Level, Sink, Site};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// A clock: nanoseconds since an arbitrary fixed origin, non-decreasing.
pub type Clock = fn() -> u64;

/// Marks no entry, no parent, no site.
const NONE: u32 = u32::MAX;

/// Built-in measures; user measures follow by name. The last three
/// are the counters of [`Alloc`](crate::Alloc), omitted from a record where
/// they are zero at every node.
const BUILTIN: [(&str, bool); 6] = [
    ("calls", true),
    ("wall", false),
    ("busy", false),
    ("bytes", true),
    ("allocs", true),
    ("freed", true),
];
const CALLS: usize = 0;
const WALL: usize = 1;
const BUSY: usize = 2;
const BYTES: usize = 3;

/// One thread's built-in measures of one node.
type Base = [AtomicU64; 6];

/// Process-wide: the call sites and the live contexts with their highest level.
struct Registry {
    sites: Vec<&'static Site>,
    contexts: Vec<(u64, Level, Weak<Shared>)>,
    next: u64,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    sites: Vec::new(),
    contexts: Vec::new(),
    next: 1,
});

/// Highest threshold or filter level of all live contexts; 0 when there is none.
pub(crate) static MAX: AtomicU8 = AtomicU8::new(0);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers a site and returns its id.
#[cold]
pub(crate) fn register(site: &'static Site) -> u32 {
    let mut r = lock(&REGISTRY);
    if let id @ 1.. = site.id.load(Relaxed) {
        return id - 1;
    }
    r.sites.push(site);
    let id = r.sites.len() as u32;
    site.id.store(id, Relaxed);
    id - 1
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
/// own clock to [`Builder::clock`].
#[cfg(target_arch = "wasm32")]
pub fn now() -> u64 {
    0
}

/// Shared by a context, its guards and the threads that entered it.
pub(crate) struct Shared {
    id: u64,
    threshold: Level,
    filters: Vec<(String, Level)>,
    pub(crate) clock: Clock,
    tree: Mutex<Tree>,
    pub(crate) sinks: Vec<Box<dyn Sink>>,
    pub(crate) warnings: Mutex<Warnings>,
}

impl Shared {
    /// The level up to which sites of `module` are active: that of the
    /// longest filter prefix covering the module, else the threshold.
    pub(crate) fn level(&self, module: &str) -> Level {
        let covers = |p: &str| {
            module
                .strip_prefix(p)
                .is_some_and(|r| r.is_empty() || r.starts_with("::"))
        };
        self.filters
            .iter()
            .filter(|f| covers(&f.0))
            .max_by_key(|f| f.0.len())
            .map_or(self.threshold, |f| f.1)
    }

    /// Writes the scope names from the root to `node`, joined by `/`.
    pub(crate) fn path(&self, node: u32, f: &mut fmt::Formatter) -> fmt::Result {
        let t = lock(&self.tree);
        let r = lock(&REGISTRY);
        let mut names = Vec::new();
        let mut n = node;
        while n != 0 {
            let (p, s) = t.keys[n as usize];
            names.push(r.sites[s as usize].name);
            n = p;
        }
        for (i, name) in names.iter().rev().enumerate() {
            if i > 0 {
                f.write_char('/')?;
            }
            f.write_str(name)?;
        }
        Ok(())
    }
}

/// The nodes and every thread's cells of one context.
struct Tree {
    /// (parent, site) per node id; the root is (NONE, NONE).
    keys: Vec<(u32, u32)>,
    index: BTreeMap<(u32, u32), u32>,
    bases: Vec<(u32, Box<Base>)>,
    /// (node, `add!` site, value) of user measures.
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
    /// By site id: 0 undecided, 1 inactive, 2 active in this context.
    active: Vec<u8>,
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
        quiet(|| self.intern(node, site))
    }

    #[cold]
    fn intern(&mut self, node: u32, site: u32) -> u32 {
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

    /// This thread's cell of an `add!` site at `node`, allocated on first use.
    fn extra(&mut self, node: u32, site: u32) -> &AtomicU64 {
        let local = &self.nodes[node as usize];
        let p = match local.extras.iter().find(|e| e.0 == site) {
            Some(&(_, p)) => p,
            None => quiet(|| {
                let b = Box::<AtomicU64>::default();
                let p: *const AtomicU64 = &*b;
                lock(&self.shared().tree).extras.push((node, site, b));
                self.nodes[node as usize].extras.push((site, p));
                p
            }),
        };
        // SAFETY: as in `base`.
        unsafe { &*p }
    }

    /// Decides once whether `site` is active in this context.
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
            active: Vec::new(),
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

    /// The current entry and the site id if `site` is active in it.
    #[inline]
    fn active(&mut self, site: &'static Site) -> Option<(&mut Entry, u32)> {
        let e = self.entries.get_mut(self.cur.entry as usize)?;
        let id = site.id();
        let on = match e.active.get(id as usize) {
            Some(&d) if d != 0 => d == 2,
            _ => e.decide(site, id as usize),
        };
        on.then_some((e, id))
    }
}

/// The current context and node of this thread if `site` is active there,
/// or for `None` if a site of `level` in `module` would be.
pub(crate) fn here(
    site: Option<&'static Site>,
    level: Level,
    module: &str,
) -> Option<(Arc<Shared>, u32)> {
    THREAD.with(|t| {
        let t = &mut *t.borrow_mut();
        let node = t.cur.node;
        let e = match site {
            Some(s) => t.active(s)?.0,
            None => t.entries.get_mut(t.cur.entry as usize)?,
        };
        let shared = e.live.clone()?;
        (site.is_some() || level <= shared.level(module)).then_some((shared, node))
    })
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
            let Some((e, id)) = t.active(self) else {
                return Guard::IDLE;
            };
            let node = e.child(prev.node, id);
            let (ctx, start) = (e.id, (e.clock)());
            t.cur.node = node;
            Guard {
                ctx,
                cur: t.cur,
                prev,
                start,
                mem: counts(),
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
            let (e, id) = t.active(self)?;
            Some(e.extra(node, id) as *const AtomicU64)
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

    /// The allocations of a thread's first entry are not counted.
    fn enter(shared: Arc<Shared>, node: u32, busy: bool) -> Guard {
        let mut g = quiet(|| Guard::begin(shared, node, busy));
        g.mem = counts();
        g
    }

    fn begin(shared: Arc<Shared>, node: u32, busy: bool) -> Guard {
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
                mem: [0; N],
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
        let mem = counts();
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
            for (k, m) in mem.iter().enumerate() {
                bump(&b[BYTES + k], m.wrapping_sub(self.mem[k]));
            }
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

/// The settings of a context, fixed when it is built: threshold, filters,
/// clock and sinks.
pub struct Builder {
    threshold: Level,
    filters: Vec<(String, Level)>,
    clock: Clock,
    sinks: Vec<Box<dyn Sink>>,
}

impl Builder {
    /// The level up to which sites outside every filter are active; INFO by
    /// default.
    pub fn threshold(mut self, level: Level) -> Builder {
        self.threshold = level;
        self
    }

    /// The level up to which sites in module `prefix` and its submodules are
    /// active (`a::b` covers `a::b` and `a::b::c`, not `a::bc`). Of several
    /// filters covering a module the longest prefix applies.
    pub fn filter(mut self, prefix: &str, level: Level) -> Builder {
        self.filters.push((prefix.to_string(), level));
        self
    }

    /// The clock; [`now`] by default.
    pub fn clock(mut self, clock: Clock) -> Builder {
        self.clock = clock;
        self
    }

    /// Adds a sink that receives every event of the context.
    pub fn sink(mut self, sink: impl Sink + 'static) -> Builder {
        self.sinks.push(Box::new(sink));
        self
    }

    /// The context.
    pub fn build(self) -> Context {
        let max = self
            .filters
            .iter()
            .map(|f| f.1)
            .fold(self.threshold, Level::max);
        let mut r = lock(&REGISTRY);
        let id = r.next;
        r.next += 1;
        let shared = Arc::new(Shared {
            id,
            threshold: self.threshold,
            filters: self.filters,
            clock: self.clock,
            tree: Mutex::new(Tree {
                keys: vec![(NONE, NONE)],
                index: BTreeMap::new(),
                bases: Vec::new(),
                extras: Vec::new(),
                threads: 0,
                params: Vec::new(),
            }),
            sinks: self.sinks,
            warnings: Mutex::default(),
        });
        r.contexts.push((id, max, Arc::downgrade(&shared)));
        MAX.fetch_max(max.0, Relaxed);
        Context { shared }
    }
}

impl Context {
    /// A context at threshold INFO with the default clock [`now`], no filter
    /// and no sink.
    pub fn new() -> Context {
        Context::builder().build()
    }

    /// The builder of a context with other settings.
    pub fn builder() -> Builder {
        Builder {
            threshold: Level::INFO,
            filters: Vec::new(),
            clock: now,
            sinks: Vec::new(),
        }
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
        let calibration = quiet(|| calibrate(self.shared.clock));
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
        for (n, s, c) in &t.extras {
            let name = r.sites[*s as usize].name;
            let values = &mut nodes[*n as usize].values;
            match values.iter_mut().find(|v| v.0 == name) {
                Some(v) => v.1 += c.load(Relaxed),
                None => values.push((name, c.load(Relaxed))),
            }
            if !measures.iter().any(|m| m.0 == name) {
                measures.push((name, true));
            }
        }
        let unused: Vec<&str> = BUILTIN[BYTES..]
            .iter()
            .map(|m| m.0)
            .filter(|&m| nodes.iter().all(|n| n.get(m) == 0))
            .collect();
        for n in &mut nodes {
            n.values.retain(|v| !unused.contains(&v.0));
        }
        measures.retain(|m| !unused.contains(&m.0));
        let mut sites: Vec<u32> = t.keys[1..].iter().map(|k| k.1).collect();
        sites.sort_unstable();
        sites.dedup();
        let w = lock(&self.shared.warnings);
        Record {
            warnings: w.kept.clone(),
            warnings_dropped: w.dropped,
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
    let ctx = Context::builder()
        .threshold(Level::TRACE)
        .clock(clock)
        .build();
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
