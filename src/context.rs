//! The context: its settings, its tree, and the assembly of the record.

use crate::alloc::quiet;
use crate::events::{Paced, Warnings};
use crate::record::{Calibration, Node, Param, Record, ThreadNodes};
use crate::site::{lock, registry, Registry};
use crate::stream::Stream;
use crate::thread::{Base, NONE, THREAD};
use crate::{Guard, Level, Sink};
use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// A clock: nanoseconds since an arbitrary fixed origin, non-decreasing.
pub type Clock = fn() -> u64;

/// Built-in measures, exact or not; user measures follow by name, of the kind
/// of their first site. The last four are the counters of [`Alloc`](
/// crate::Alloc), omitted from a record where they are zero at every node;
/// `peak` is merged by maximum.
const BUILTIN: [(&str, bool); 7] = [
    ("calls", true),
    ("wall", false),
    ("busy", false),
    ("bytes", true),
    ("allocs", true),
    ("freed", true),
    ("peak", true),
];
pub(crate) const CALLS: usize = 0;
pub(crate) const WALL: usize = 1;
pub(crate) const BUSY: usize = 2;
pub(crate) const BYTES: usize = 3;
pub(crate) const PEAK: usize = 6;

/// Two values of a measure merged: by maximum or summed.
fn merge(max: bool, a: u64, b: u64) -> u64 {
    if max {
        a.max(b)
    } else {
        a + b
    }
}

/// The default clock: `std::time::Instant` since the first read in the
/// process. On wasm32 none, every time is zero: the host passes its own clock
/// to [`Builder::clock`].
pub fn now() -> u64 {
    if cfg!(target_arch = "wasm32") {
        return 0;
    }
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_nanos() as u64
}

/// Shared by a context, its guards and the threads that entered it.
pub(crate) struct Shared {
    pub(crate) id: u64,
    /// Set when the [`Context`] drops.
    pub(crate) closed: AtomicBool,
    pub(crate) cfg: Builder,
    /// The clock when the context was built.
    pub(crate) start: u64,
    pub(crate) tree: Mutex<Tree>,
    pub(crate) warnings: Mutex<Warnings>,
    pub(crate) progress: Mutex<Vec<Paced>>,
    pub(crate) stream: Option<Mutex<Stream>>,
}

/// The nodes and every thread's cells of one context.
pub(crate) struct Tree {
    /// (parent, site) per node id; the root is (NONE, NONE).
    keys: Vec<(u32, u32)>,
    index: BTreeMap<(u32, u32), u32>,
    /// (thread, node, cells).
    pub(crate) bases: Vec<(u32, u32, Box<Base>)>,
    /// (thread, node, measure site, value) of user measures.
    pub(crate) extras: Vec<(u32, u32, u32, Box<AtomicU64>)>,
    /// Per thread in order of first entry: whether through a handle.
    handles: Vec<bool>,
    params: Vec<(String, Param)>,
}

impl Tree {
    /// The child of `node` for a scope site, added on first use.
    pub(crate) fn intern(&mut self, node: u32, site: u32) -> u32 {
        let next = self.keys.len() as u32;
        let c = *self.index.entry((node, site)).or_insert(next);
        if c == next {
            self.keys.push((node, site));
        }
        c
    }

    /// The index of a thread entering the context for the first time,
    /// through a handle if `handle`.
    pub(crate) fn thread(&mut self, handle: bool) -> u32 {
        self.handles.push(handle);
        self.handles.len() as u32 - 1
    }

    /// The name of the scope of `node`; `root` for the root.
    fn name(&self, r: &Registry, node: u32) -> &'static str {
        match self.keys[node as usize].1 {
            NONE => "root",
            s => r.name(s),
        }
    }

    /// Writes the scope names from the root to `node`, joined by `/`.
    fn path(&self, r: &Registry, node: u32, f: &mut fmt::Formatter) -> fmt::Result {
        let parent = self.keys[node as usize].0;
        if parent != 0 {
            self.path(r, parent, f)?;
            f.write_char('/')?;
        }
        f.write_str(self.name(r, node))
    }
}

impl Shared {
    /// The level up to which sites of `module` are active: that of the
    /// longest filter prefix covering the module, else the threshold.
    pub(crate) fn level(&self, module: &str) -> Level {
        let covers = |p: &str| {
            let rest = module.strip_prefix(p);
            rest.is_some_and(|r| r.is_empty() || r.starts_with("::"))
        };
        let filters = self.cfg.filters.iter().filter(|f| covers(&f.0));
        let longest = filters.max_by_key(|f| f.0.len());
        longest.map_or(self.cfg.threshold, |f| f.1)
    }

    /// Writes the scope names from the root to `node`, joined by `/`; nothing
    /// for the root.
    pub(crate) fn path(&self, node: u32, f: &mut fmt::Formatter) -> fmt::Result {
        match node {
            0 => Ok(()),
            n => lock(&self.tree).path(&registry(), n, f),
        }
    }
}

/// The settings of a context, fixed when it is built: threshold, filters,
/// clock and sinks.
pub struct Builder {
    threshold: Level,
    filters: Vec<(String, Level)>,
    pub(crate) clock: Clock,
    pub(crate) sinks: Vec<(Level, Box<dyn Sink>)>,
    pub(crate) progress: u64,
    stream: bool,
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
    pub fn sink(self, sink: impl Sink + 'static) -> Builder {
        self.sink_at(Level::TRACE, sink)
    }

    /// Adds a sink that receives the events up to `level`: with the
    /// threshold at INFO and a terminal sink at WARN the scopes are recorded
    /// and a warning is shown with its path, without the INFO messages.
    pub fn sink_at(mut self, level: Level, sink: impl Sink + 'static) -> Builder {
        self.sinks.push((level, Box::new(sink)));
        self
    }

    /// The least time in ns between two progress events of a call site; one
    /// second by default, 0 for every event. A run of a site begins with its
    /// first event and ends with its closing one (`done >= total`); without
    /// a total each thread has a run of its own, and a new one begins where
    /// the units go down. An event passes
    /// when the interval has gone by since the run began or its last event
    /// passed, and the closing event where one of the run passed before: a
    /// run shorter than the interval reports nothing, however often it
    /// repeats, a longer one at most once an interval and its end.
    pub fn progress_interval(mut self, ns: u64) -> Builder {
        self.progress = ns;
        self
    }

    /// Keeps every guard's span, every event and every `add!` value with its
    /// time, for [`Context::trace`]. Each thread appends to a buffer of its
    /// own and moves it to the context when its outermost guard drops.
    pub fn stream(mut self) -> Builder {
        self.stream = true;
        self
    }

    /// The context.
    pub fn build(self) -> Context {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Relaxed);
        let max = self.filters.iter().map(|f| f.1);
        let max = max.fold(self.threshold, Level::max);
        let shared = Arc::new(Shared {
            id,
            closed: AtomicBool::new(false),
            stream: self.stream.then(Mutex::default),
            start: (self.clock)(),
            cfg: self,
            tree: Mutex::new(Tree {
                keys: vec![(NONE, NONE)],
                index: BTreeMap::new(),
                bases: Vec::new(),
                extras: Vec::new(),
                handles: Vec::new(),
                params: Vec::new(),
            }),
            warnings: Mutex::default(),
            progress: Mutex::default(),
        });
        registry().insert(id, max, Arc::downgrade(&shared));
        Context { shared }
    }
}

/// The measurement state of one problem: its tree, every thread's values,
/// its parameters and the calibration constants.
pub struct Context {
    shared: Arc<Shared>,
}

impl Drop for Context {
    /// Unregisters the context; threads still holding it record nothing more
    /// in it and let it go when they next enter a context.
    fn drop(&mut self) {
        self.shared.closed.store(true, Relaxed);
        registry().remove(self.shared.id);
    }
}

impl Default for Context {
    fn default() -> Context {
        Context::new()
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
            progress: crate::PROGRESS_NS,
            stream: false,
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
        self.take(false)
    }

    /// [`record`](Self::record) with [`Record::per_thread`], each thread's
    /// values by node.
    pub fn record_threads(&self) -> Record {
        self.take(true)
    }

    fn take(&self, per_thread: bool) -> Record {
        let calibration = match cfg!(feature = "on") {
            true => quiet(|| calibrate(self.shared.cfg.clock)),
            false => Calibration::default(),
        };
        self.sums(calibration, per_thread)
    }

    /// The stream as Chrome trace event JSON, as read by Perfetto, speedscope
    /// and the Firefox Profiler: per thread an "M" event, per guard span an
    /// "X" event named by its scope (`root` for the root) with its node and the
    /// node's parent, per event an "i" event, per `add!` a "C" event. Times in
    /// microseconds since the context was built. Includes what this thread has
    /// recorded so far and what other threads have recorded up to the drop of
    /// their outermost guard. Empty
    /// without [`Builder::stream`].
    pub fn trace(&self) -> String {
        let Some(stream) = &self.shared.stream else {
            return String::new();
        };
        THREAD.with(|t| {
            let entries = &mut t.borrow_mut().entries;
            let e = entries.iter_mut().find(|e| e.id == self.shared.id);
            e.map(|e| e.flush())
        });
        let (s, t, r) = (lock(stream), lock(&self.shared.tree), registry());
        let parent = |n: u32| Some(t.keys[n as usize].0).filter(|&p| p != NONE);
        s.write(self.shared.start, |n| t.name(&r, n), parent, |m| r.name(m))
    }

    /// [`record`](Self::record) with the given calibration constants, and
    /// with each thread's values if `per_thread`.
    fn sums(&self, calibration: Calibration, per_thread: bool) -> Record {
        let (t, r) = (lock(&self.shared.tree), registry());
        // The nodes with the cells of the threads `of` accepts merged.
        let merged = |of: &dyn Fn(u32) -> bool| {
            let node = |&(p, s): &(u32, u32)| Node {
                parent: (p != NONE).then_some(p),
                site: (s != NONE).then_some(s),
                values: BUILTIN.iter().map(|m| (m.0, 0)).collect(),
            };
            let mut nodes: Vec<Node> = t.keys.iter().map(node).collect();
            for (_, n, b) in t.bases.iter().filter(|c| of(c.0)) {
                let values = nodes[*n as usize].values.iter_mut();
                for (i, (v, c)) in values.zip(b.iter()).enumerate() {
                    v.1 = merge(i == PEAK, v.1, c.load(Relaxed));
                }
            }
            for (_, n, s, c) in t.extras.iter().filter(|c| of(c.0)) {
                let site = r.sites[*s as usize];
                let (name, values) = (site.name, &mut nodes[*n as usize].values);
                match values.iter_mut().find(|v| v.0 == name) {
                    Some(v) => v.1 = merge(site.max, v.1, c.load(Relaxed)),
                    None => values.push((name, c.load(Relaxed))),
                }
            }
            nodes
        };
        let mut nodes = merged(&|_| true);
        let (mut measures, mut max) = (BUILTIN.to_vec(), vec![BUILTIN[PEAK].0]);
        for site in t.extras.iter().map(|c| r.sites[c.2 as usize]) {
            if !measures.iter().any(|m| m.0 == site.name) {
                measures.push((site.name, site.exact));
                max.extend(site.max.then_some(site.name));
            }
        }
        let unused: Vec<&str> = BUILTIN[BYTES..]
            .iter()
            .map(|m| m.0)
            .filter(|&m| nodes.iter().all(|n| n.get(m) == 0))
            .collect();
        let strip = |nodes: &mut Vec<Node>| {
            for n in nodes {
                n.values.retain(|v| !unused.contains(&v.0));
            }
        };
        strip(&mut nodes);
        measures.retain(|m| !unused.contains(&m.0));
        max.retain(|m| !unused.contains(m));
        let threads = t.handles.iter().enumerate().filter(|_| per_thread);
        let per_thread = threads.map(|(i, &handle)| {
            let mut nodes = merged(&|th| th == i as u32);
            strip(&mut nodes);
            let index = i as u32;
            ThreadNodes {
                index,
                handle,
                nodes,
            }
        });
        let mut sites: Vec<u32> = t.keys[1..].iter().map(|k| k.1).collect();
        sites.sort_unstable();
        sites.dedup();
        let w = lock(&self.shared.warnings);
        Record {
            warnings: w.kept.clone(),
            warnings_dropped: w.dropped,
            params: t.params.clone(),
            calibration,
            threads: t.handles.len() as u32,
            per_thread: per_thread.collect(),
            measures,
            max,
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
        let r = ctx.sums(Calibration::default(), false);
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
