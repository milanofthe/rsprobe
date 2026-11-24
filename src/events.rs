//! Events: messages and progress at the current node.

use crate::alloc::quiet;
use crate::context::Shared;
use crate::sinks::Path;
use crate::site::lock;
use crate::{Level, Sink, Site};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

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
    /// Time since the context was built, in ns of its clock.
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

/// Emits the event `kind()` at `node` of `shared` from the thread of index
/// `thread`; `kind` is called outside the thread-local state. The event is
/// kept among the warnings if its level is WARN or above, in the stream if
/// there is one, and passed to every sink of its level; with none of the
/// three `kind` is not called. Its allocations, and those of the sinks, are
/// not counted by [`Alloc`](crate::Alloc).
pub(crate) fn emit(
    (shared, node, thread): (Arc<Shared>, u32, u32),
    (level, module, file, line): (Level, &'static str, &'static str, u32),
    kind: impl FnOnce() -> Kind,
) {
    let shown = |s: &&(Level, Box<dyn Sink>)| level <= s.0;
    if level > Level::WARN && shared.stream.is_none() && !shared.cfg.sinks.iter().any(|s| shown(&s))
    {
        return;
    }
    quiet(|| {
        let kind = kind();
        let time = (shared.cfg.clock)() - shared.start;
        let e = Event {
            node,
            level,
            module,
            file,
            line,
            time,
            kind,
        };
        if let Some(s) = &shared.stream {
            lock(s).events.push((thread, e.clone()));
        }
        if e.level <= Level::WARN && matches!(e.kind, Kind::Message(_)) {
            lock(&shared.warnings).add(&e);
        }
        let path = Path {
            shared: &shared,
            node,
        };
        for s in shared.cfg.sinks.iter().filter(shown) {
            s.1.event(&e, &path);
        }
    })
}

impl Site {
    /// Emits the event `kind()` at the current node if this site is active;
    /// what [`error!`](crate::error) and [`progress!`](crate::progress)
    /// expand to.
    #[inline]
    pub fn event(&'static self, kind: impl FnOnce() -> Kind) {
        let here = self.active(|cur, e, _| (e.shared().clone(), cur.node, e.thread));
        if let Some(here) = here {
            emit(here, (self.level, self.module, self.file, self.line), kind)
        }
    }
}
