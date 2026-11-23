//! The context: its settings, its tree, and the assembly of the record.

use crate::alloc::quiet;
use crate::events::Warnings;
use crate::record::{Calibration, Node, Param, Record};
use crate::site::{lock, registry, Registry};
use crate::stream::Stream;
use crate::thread::{Base, NONE, THREAD};
use crate::{Guard, Level, Sink};
use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// A clock: nanoseconds since an arbitrary fixed origin, non-decreasing.
pub type Clock = fn() -> u64;

/// Built-in measures, exact or not; user measures follow by name, of the
/// kind of the first site that added to them. The last
/// three are the counters of [`Alloc`](crate::Alloc), omitted from a record
/// where they are zero at every node.
const BUILTIN: [(&str, bool); 6] = [
    ("calls", true),
    ("wall", false),
    ("busy", false),
    ("bytes", true),
    ("allocs", true),
    ("freed", true),
];
pub(crate) const CALLS: usize = 0;
pub(crate) const WALL: usize = 1;
pub(crate) const BUSY: usize = 2;
pub(crate) const BYTES: usize = 3;

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
    pub(crate) cfg: Builder,
    pub(crate) tree: Mutex<Tree>,
    pub(crate) warnings: Mutex<Warnings>,
    pub(crate) stream: Option<Mutex<Stream>>,
}

/// The nodes and every thread's cells of one context.
pub(crate) struct Tree {
    /// (parent, site) per node id; the root is (NONE, NONE).
    keys: Vec<(u32, u32)>,
    index: BTreeMap<(u32, u32), u32>,
    pub(crate) bases: Vec<(u32, Box<Base>)>,
    /// (node, `add!` site, value) of user measures.
    pub(crate) extras: Vec<(u32, u32, Box<AtomicU64>)>,
    threads: u32,
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

    /// The index of a thread entering the context for the first time.
    pub(crate) fn thread(&mut self) -> u32 {
        self.threads += 1;
        self.threads - 1
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

impl Drop for Shared {
    fn drop(&mut self) {
        registry().remove(self.id);
    }
}

/// The settings of a context, fixed when it is built: threshold, filters,
/// clock and sinks.
pub struct Builder {
    threshold: Level,
    filters: Vec<(String, Level)>,
    pub(crate) clock: Clock,
    pub(crate) sinks: Vec<Box<dyn Sink>>,
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
    pub fn sink(mut self, sink: impl Sink + 'static) -> Builder {
        self.sinks.push(Box::new(sink));
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
            stream: self.stream.then(Mutex::default),
            cfg: self,
            tree: Mutex::new(Tree {
                keys: vec![(NONE, NONE)],
                index: BTreeMap::new(),
                bases: Vec::new(),
                extras: Vec::new(),
                threads: 0,
                params: Vec::new(),
            }),
            warnings: Mutex::default(),
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
        let calibration = match cfg!(feature = "on") {
            true => quiet(|| calibrate(self.shared.cfg.clock)),
            false => Calibration::default(),
        };
        self.sums(calibration)
    }

    /// The stream as Chrome trace event JSON, as read by Perfetto, speedscope
    /// and the Firefox Profiler: per thread an "M" event, per guard span an
    /// "X" event named by its scope (`root` for the root), per event an "i"
    /// event, per `add!` a "C" event. Times in microseconds of the context's
    /// clock. Includes what this thread has recorded so far and what other
    /// threads have recorded up to the drop of their outermost guard. Empty
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
        s.write(|n| t.name(&r, n), |m| r.name(m))
    }

    /// [`record`](Self::record) with the given calibration constants.
    fn sums(&self, calibration: Calibration) -> Record {
        let (t, r) = (lock(&self.shared.tree), registry());
        let node = |&(p, s): &(u32, u32)| Node {
            parent: (p != NONE).then_some(p),
            site: (s != NONE).then_some(s),
            values: BUILTIN.iter().map(|m| (m.0, 0)).collect(),
        };
        let mut nodes: Vec<Node> = t.keys.iter().map(node).collect();
        for (n, b) in &t.bases {
            for (v, c) in nodes[*n as usize].values.iter_mut().zip(b.iter()) {
                v.1 += c.load(Relaxed);
            }
        }
        let mut measures = BUILTIN.to_vec();
        for (n, s, c) in &t.extras {
            let site = r.sites[*s as usize];
            let (name, values) = (site.name, &mut nodes[*n as usize].values);
            match values.iter_mut().find(|v| v.0 == name) {
                Some(v) => v.1 += c.load(Relaxed),
                None => values.push((name, c.load(Relaxed))),
            }
            if !measures.iter().any(|m| m.0 == name) {
                measures.push((name, site.exact));
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
