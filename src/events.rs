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
    /// `done` of `total` units of work, with a text or an empty one.
    Progress {
        /// Units done.
        done: u64,
        /// Units in total; 0 where the total is not known.
        total: u64,
        /// What the work is at: a residual, a frequency.
        text: String,
    },
}

impl Display for Kind {
    /// The text of a message; for progress `done/total` (`done` alone
    /// where the total is not known) and its text.
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Kind::Message(t) => f.write_str(t),
            Kind::Progress { done, total, text } => {
                write!(f, "{done}")?;
                if *total > 0 {
                    write!(f, "/{total}")?;
                }
                if !text.is_empty() {
                    write!(f, " {text}")?;
                }
                Ok(())
            }
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
/// site and text a sink gets the first, the tenth, the hundredth and so on,
/// with the count appended, unless it takes [`every`](Sink::every) event.
/// Its allocations, and those of the sinks, are
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
        let e = Event {
            node,
            level,
            module,
            file,
            line,
            time: begin - shared.start,
            kind: kind(),
        };
        if let Some(s) = &shared.stream {
            lock(s).events.push((thread, e.clone()));
        }
        let repeat = match (level <= Level::WARN, &e.kind) {
            (true, Kind::Message(_)) => lock(&shared.warnings).add(&e),
            _ => Some(String::new()),
        };
        // The event with the count in its text, where it has one.
        let counted = repeat.as_ref().filter(|r| !r.is_empty()).map(|r| Event {
            kind: Kind::Message(format!("{}{r}", e.kind)),
            ..e.clone()
        });
        let path = Path {
            shared: &shared,
            node,
        };
        for s in shared.cfg.sinks.iter().filter(shown) {
            if s.1.every() {
                s.1.event(&e, &path);
            } else if repeat.is_some() {
                s.1.event(counted.as_ref().unwrap_or(&e), &path);
            }
        }
        let ns = clock() - begin;
        with_current(|_, e| e.out += ns);
        crate::time!(ERROR, "emit", ns);
    })
}

/// A progress site of a context and its current run, times in ns since the
/// context was built.
pub(crate) struct Paced {
    /// File and line of the site, and for a site without a total the
    /// thread: a count without a total is one loop's own.
    site: (&'static str, u32, u32),
    /// Whether a run is going: begun by an event and not closed.
    running: bool,
    /// When the run began or its last event passed, whichever is later.
    since: u64,
    /// Whether an event of the run passed.
    open: bool,
    /// Units done at the site's last event.
    done: u64,
}

/// Whether the progress event of the site at (file, line) with `done` of
/// `total` units passes now, by the rule of
/// [`Builder::progress_interval`](crate::Builder::progress_interval).
fn due(shared: &Shared, site: (&'static str, u32, u32), done: u64, total: u64) -> bool {
    let now = (shared.cfg.clock)() - shared.start;
    let mut sites = lock(&shared.progress);
    let i = sites
        .iter()
        .position(|s| s.site == site)
        .unwrap_or_else(|| {
            sites.push(Paced {
                site,
                running: false,
                since: 0,
                open: false,
                done: 0,
            });
            sites.len() - 1
        });
    let s = &mut sites[i];
    // A run begins with the first event after a closing one, and without a
    // total where the units go down. With a total they may arrive out of
    // order from the threads of one run.
    if !s.running || (total == 0 && done < s.done) {
        (s.running, s.since, s.open) = (true, now, false);
    }
    let last = total > 0 && done >= total;
    let pass = now - s.since >= shared.cfg.progress || (last && s.open);
    if pass {
        (s.since, s.open) = (now, true);
    }
    (s.done, s.running) = (done, !last);
    pass
}

impl Site {
    /// Emits the event `kind()` at the current node if this site is active;
    /// what [`error!`](crate::error) expands to.
    #[inline]
    pub fn event(&'static self, kind: impl FnOnce() -> Kind) {
        let here = self.active(|cur, e, _| (e.shared().clone(), cur.node, e.thread));
        if let Some(here) = here {
            emit(here, (self.level, self.module, self.file, self.line), kind)
        }
    }

    /// Emits a progress event of `units()` (done, total) at the current node
    /// if this site is active and the event is due; `text` is called only
    /// then. What [`progress!`](crate::progress) expands to.
    #[inline]
    pub fn progress(
        &'static self,
        units: impl FnOnce() -> (u64, u64),
        text: impl FnOnce() -> String,
    ) {
        let here = self.active(|cur, e, _| (e.shared().clone(), cur.node, e.thread));
        if let Some(here) = here {
            let (done, total) = units();
            // With a total the threads of a pool report one run; without
            // one each thread's loop is a run of its own.
            let thread = if total > 0 { u32::MAX } else { here.2 };
            if due(&here.0, (self.file, self.line, thread), done, total) {
                let site = (self.level, self.module, self.file, self.line);
                emit(here, site, || Kind::Progress {
                    done,
                    total,
                    text: text(),
                })
            }
        }
    }
}
