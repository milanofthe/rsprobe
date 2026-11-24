//! Events: messages and progress at the current node.

use crate::alloc::quiet;
use crate::context::Shared;
use crate::sinks::Path;
use crate::site::lock;
use crate::thread::with_current;
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
    /// it while fewer than [`WARNINGS`](crate::WARNINGS) are kept. Returns
    /// what the sinks are to be told: nothing at the first event of a site
    /// and text, the count when it reaches a power of ten, and the number of
    /// events not kept when that reaches one; `None` for the events between,
    /// which the sinks do not get.
    fn add(&mut self, e: &Event) -> Option<String> {
        let same =
            |k: &Event| (k.file, k.line, k.module, &k.kind) == (e.file, e.line, e.module, &e.kind);
        let full = self.kept.len() >= crate::WARNINGS;
        let (n, kept) = match self.kept.iter_mut().find(|k| same(&k.0)) {
            Some(k) => {
                k.1 += 1;
                (k.1, true)
            }
            None if full => {
                self.dropped += 1;
                (self.dropped, false)
            }
            None => {
                self.kept.push((e.clone(), 1));
                (1, true)
            }
        };
        (10u64.pow(n.ilog10()) == n).then(|| match (kept, n) {
            (true, 1) => String::new(),
            (true, _) => format!(" ({n} times)"),
            (false, _) => format!(" (not kept, {n} beyond {} distinct)", crate::WARNINGS),
        })
    }
}

/// Emits the event `kind()` at `node` of `shared` from the thread of index
/// `thread`; `kind` is called outside the thread-local state. The event is
/// kept among the warnings if its level is WARN or above, in the stream if
/// there is one, and passed to every sink of its level; with none of the
/// three `kind` is not called. Of the events at WARN and above with the same
/// site and text the sinks get the first, the tenth, the hundredth and so
/// on, with the count appended. Its allocations, and those of the sinks, are
/// not counted by [`Alloc`](crate::Alloc); its time is the measure `emit` of
/// the node and not part of the spans open on this thread.
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
        let clock = shared.cfg.clock;
        let begin = clock();
        let mut e = Event {
            node,
            level,
            module,
            file,
            line,
            time: begin - shared.start,
            kind: kind(),
        };
        if let Kind::Progress { done, total } = e.kind {
            if !due(&shared, (file, line), e.time, done >= total) {
                return;
            }
        }
        if let Some(s) = &shared.stream {
            lock(s).events.push((thread, e.clone()));
        }
        let repeat = match (level <= Level::WARN, &e.kind) {
            (true, Kind::Message(_)) => lock(&shared.warnings).add(&e),
            _ => Some(String::new()),
        };
        if let Some(repeat) = repeat {
            if let Kind::Message(text) = &mut e.kind {
                text.push_str(&repeat);
            }
            let path = Path {
                shared: &shared,
                node,
            };
            for s in shared.cfg.sinks.iter().filter(shown) {
                s.1.event(&e, &path);
            }
        }
        let ns = clock() - begin;
        with_current(|_, e| e.out += ns);
        crate::time!(ERROR, "emit", ns);
    })
}

/// A progress site of a context: when its last event passed, in ns since
/// the context was built, and whether an event of its current run passed.
pub(crate) struct Paced {
    site: (&'static str, u32),
    at: u64,
    open: bool,
}

/// Whether the progress event of the site at (file, line) at `now` passes,
/// by the rule of [`Builder::progress_interval`](crate::Builder::progress_interval);
/// `last` for its closing event.
fn due(shared: &Shared, site: (&'static str, u32), now: u64, last: bool) -> bool {
    let mut sites = lock(&shared.progress);
    let i = sites
        .iter()
        .position(|s| s.site == site)
        .unwrap_or_else(|| {
            sites.push(Paced {
                site,
                at: 0,
                open: false,
            });
            sites.len() - 1
        });
    let s = &mut sites[i];
    let pass = now - s.at >= shared.cfg.progress || (last && s.open);
    if pass {
        (s.at, s.open) = (now, !last);
    }
    pass
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
