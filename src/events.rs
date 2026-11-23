//! Events: messages and progress at the current node.

use crate::context::{here, lock, Shared, MAX};
use crate::sinks::Path;
use crate::{Level, Site};
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::Ordering::Relaxed;

/// What an event carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A formatted message.
    Message(String),
    /// `done` of `total` units of work.
    Progress {
        /// Units done.
        done: u64,
        /// Units in total.
        total: u64,
    },
}

impl Display for Kind {
    /// The text of a message; `done/total` for progress.
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Kind::Message(t) => f.write_str(t),
            Kind::Progress { done, total } => write!(f, "{done}/{total}"),
        }
    }
}

/// An event at a node of a context.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Node id, as in [`Record::nodes`](crate::Record::nodes).
    pub node: u32,
    /// Level of the call site.
    pub level: Level,
    /// Module path of the call site.
    pub module: &'static str,
    /// File of the call site.
    pub file: &'static str,
    /// Line of the call site.
    pub line: u32,
    /// The context's clock when the event was emitted, in ns.
    pub time: u64,
    /// Message or progress.
    pub kind: Kind,
}

/// Distinct events at WARN and above, with their counts.
#[derive(Default)]
pub(crate) struct Warnings {
    pub(crate) kept: Vec<(Event, u64)>,
    pub(crate) dropped: u64,
}

impl Warnings {
    /// Counts `e` with an earlier event of the same site and text, or keeps
    /// it while fewer than [`WARNINGS`](crate::WARNINGS) are kept.
    fn add(&mut self, e: &Event) {
        let same =
            |k: &Event| (k.file, k.line, k.module, &k.kind) == (e.file, e.line, e.module, &e.kind);
        let full = self.kept.len() >= crate::WARNINGS;
        match self.kept.iter_mut().find(|k| same(&k.0)) {
            Some(k) => k.1 += 1,
            None if full => self.dropped += 1,
            None => self.kept.push((e.clone(), 1)),
        }
    }
}

/// Emits an event at the current node if `site` (or, for `None`, a site of
/// `level` in `module`) is active in the current context. `kind` is called
/// only then, outside the thread-local state. Its allocations, and
/// those of the sinks, are not counted by [`Alloc`](crate::Alloc).
pub(crate) fn emit(
    site: Option<&'static Site>,
    (level, module, file, line): (Level, &'static str, &'static str, u32),
    kind: impl FnOnce() -> Kind,
) {
    let Some((shared, node, thread)) = here(site, level, module) else {
        return;
    };
    crate::alloc::quiet(|| {
        let kind = kind();
        let e = Event {
            node,
            level,
            module,
            file,
            line,
            time: (shared.clock)(),
            kind,
        };
        send(&shared, thread, &e);
    })
}

/// Keeps `e` among the warnings if its level is WARN or above, in the stream
/// if there is one, and passes it to every sink.
fn send(shared: &Shared, thread: u32, e: &Event) {
    if let Some(s) = &shared.stream {
        lock(s).events.push((thread, e.clone()));
    }
    if e.level <= Level::WARN && matches!(e.kind, Kind::Message(_)) {
        lock(&shared.warnings).add(e);
    }
    let path = Path {
        shared,
        node: e.node,
    };
    for s in &shared.sinks {
        s.event(e, &path);
    }
}

impl Site {
    /// Emits the event `kind()` at the current node if this site is active;
    /// what [`error!`](crate::error) and [`progress!`](crate::progress)
    /// expand to.
    #[inline]
    pub fn event(&'static self, kind: impl FnOnce() -> Kind) {
        if self.level.0 > MAX.load(Relaxed) {
            return;
        }
        emit(
            Some(self),
            (self.level, self.module, self.file, self.line),
            kind,
        )
    }
}
