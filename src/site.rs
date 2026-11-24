//! Levels, call sites and the process registry of sites and live contexts.

use crate::context::Shared;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

/// Importance of a call site. A site is active when its level is at most the
/// threshold of the current context.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Level(pub(crate) u8);

impl Level {
    /// Below every site of the macros: as a threshold nothing is active, as
    /// the level of a sink nothing is shown.
    pub const OFF: Level = Level(0);
    /// Errors.
    pub const ERROR: Level = Level(1);
    /// Warnings.
    pub const WARN: Level = Level(2);
    /// Phases of a run; the default level of a site and threshold of a context.
    pub const INFO: Level = Level(3);
    /// Finer scopes, per section.
    pub const DEBUG: Level = Level(4);
    /// Finest scopes, per iteration.
    pub const TRACE: Level = Level(5);

    /// The levels from OFF to TRACE.
    pub const ALL: [Level; 6] = [
        Level::OFF,
        Level::ERROR,
        Level::WARN,
        Level::INFO,
        Level::DEBUG,
        Level::TRACE,
    ];

    /// Lower-case name, as written in the record.
    pub fn name(self) -> &'static str {
        ["off", "error", "warn", "info", "debug", "trace"][self.0 as usize]
    }
}

impl std::str::FromStr for Level {
    type Err = String;

    /// The level of a [`name`](Level::name), in any case: what a host reads
    /// from an environment variable, a project file or its own API.
    fn from_str(name: &str) -> Result<Level, String> {
        let known = |l: &Level| l.name().eq_ignore_ascii_case(name.trim());
        Level::ALL.into_iter().find(known).ok_or_else(|| {
            let names = Level::ALL.map(Level::name).join(", ");
            format!("unknown level '{name}' ({names})")
        })
    }
}

/// A call site: one static per macro expansion, given a dense id on first use.
/// The id indexes the sites of the record and the per-thread decision whether
/// the site is active.
#[derive(Debug)]
pub struct Site {
    /// Scope or measure name; empty for an event site.
    pub name: &'static str,
    /// `module_path!()` of the expansion.
    pub module: &'static str,
    /// `file!()` of the expansion.
    pub file: &'static str,
    /// `line!()` of the expansion.
    pub line: u32,
    /// Level against the context's threshold.
    pub level: Level,
    /// For a measure site: exact (a count, [`add!`](crate::add)) or noisy (a
    /// time, [`time!`](crate::time)).
    pub exact: bool,
    /// For a measure site: merged by maximum ([`max!`](crate::max)) instead
    /// of summed; `exact` is true.
    pub max: bool,
    /// Id plus one; zero until registered.
    id: AtomicU32,
}

impl Site {
    /// A site that is not yet registered.
    pub const fn new(
        name: &'static str,
        module: &'static str,
        file: &'static str,
        line: u32,
        level: Level,
    ) -> Site {
        let id = AtomicU32::new(0);
        Site {
            name,
            module,
            file,
            line,
            level,
            exact: true,
            max: false,
            id,
        }
    }

    /// This site as a noisy measure.
    pub const fn noisy(mut self) -> Site {
        self.exact = false;
        self
    }

    /// This site as a measure merged by maximum.
    pub const fn maximum(mut self) -> Site {
        self.max = true;
        self
    }

    /// The id, registered under the registry's lock on first use.
    #[inline]
    pub(crate) fn id(&'static self) -> u32 {
        match self.id.load(Relaxed) {
            0 => crate::alloc::quiet(|| self.register()),
            id => id - 1,
        }
    }

    #[cold]
    fn register(&'static self) -> u32 {
        let mut r = registry();
        if self.id.load(Relaxed) == 0 {
            r.sites.push(self);
            self.id.store(r.sites.len() as u32, Relaxed);
        }
        self.id.load(Relaxed) - 1
    }
}

/// Process-wide: the call sites by id and the live contexts with their
/// highest level.
pub(crate) struct Registry {
    pub(crate) sites: Vec<&'static Site>,
    contexts: Vec<(u64, Level, Weak<Shared>)>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    sites: Vec::new(),
    contexts: Vec::new(),
});

/// Highest threshold or filter level of all live contexts; 0 when there is none.
pub(crate) static MAX: AtomicU8 = AtomicU8::new(0);

/// A lock that ignores poisoning: every value behind one stays consistent
/// across a panic.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn registry() -> MutexGuard<'static, Registry> {
    lock(&REGISTRY)
}

impl Registry {
    /// The name of a site.
    pub(crate) fn name(&self, site: u32) -> &'static str {
        self.sites[site as usize].name
    }

    /// Adds a live context whose sites are active up to `max`.
    pub(crate) fn insert(&mut self, id: u64, max: Level, shared: Weak<Shared>) {
        self.contexts.push((id, max, shared));
        MAX.fetch_max(max.0, Relaxed);
    }

    /// Removes a context and lowers [`MAX`] to the highest of the rest.
    pub(crate) fn remove(&mut self, id: u64) {
        self.contexts.retain(|c| c.0 != id);
        let max = self.contexts.iter().map(|c| c.1 .0).max();
        MAX.store(max.unwrap_or(0), Relaxed);
    }

    /// The context of `id` if it is alive.
    pub(crate) fn find(&self, id: u64) -> Option<Arc<Shared>> {
        self.contexts.iter().find(|c| c.0 == id)?.2.upgrade()
    }
}
