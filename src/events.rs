//! Events: messages and progress at the current node.

use crate::alloc::uncounted;
use crate::context::Shared;
use crate::sinks::Path;
use crate::site::lock;
use crate::thread::{with_current, NONE};
use crate::{Level, Sink, Site};
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// What an event carries. Its `Display` is the text of a message, and for
/// progress `done/total` (`done` alone where the total is not known) followed
/// by the text.
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

#[derive(Default)]
pub(crate) struct Warnings {
    pub(crate) kept: Vec<(Event, u64)>,
    pub(crate) dropped: u64,
}

impl Warnings {
    fn count_and_suffix_for_sinks(&mut self, e: &Event) -> Option<String> {
        let same =
            |k: &Event| (k.file, k.line, k.module, &k.kind) == (e.file, e.line, e.module, &e.kind);
        let full = self.kept.len() >= crate::WARNINGS_KEPT;
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
            (false, _) => format!(" (not kept, {n} beyond {} distinct)", crate::WARNINGS_KEPT),
        })
    }
}

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
    uncounted(|| {
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
        let suffix = match (level <= Level::WARN, &e.kind) {
            (true, Kind::Message(_)) => lock(&shared.warnings).count_and_suffix_for_sinks(&e),
            _ => Some(String::new()),
        };
        let counted = suffix.as_ref().filter(|r| !r.is_empty()).map(|r| Event {
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
            } else if suffix.is_some() {
                s.1.event(counted.as_ref().unwrap_or(&e), &path);
            }
        }
        let ns = clock() - begin;
        with_current(|_, e| e.emitting_ns += ns);
        crate::time!(ERROR, "emit", ns);
    })
}

pub(crate) struct Paced {
    file_line_thread: (&'static str, u32, u32),
    running: bool,
    since_ns: u64,
    any_passed: bool,
    last_done: u64,
}

fn due(shared: &Shared, file_line_thread: (&'static str, u32, u32), done: u64, total: u64) -> bool {
    let mut sites = lock(&shared.progress);
    let now = (shared.cfg.clock)() - shared.start;
    let i = sites
        .iter()
        .position(|s| s.file_line_thread == file_line_thread)
        .unwrap_or_else(|| {
            sites.push(Paced {
                file_line_thread,
                running: false,
                since_ns: 0,
                any_passed: false,
                last_done: 0,
            });
            sites.len() - 1
        });
    let s = &mut sites[i];
    let count_went_down = total == 0 && done < s.last_done;
    if !s.running || count_went_down {
        (s.running, s.since_ns, s.any_passed) = (true, now, false);
    }
    let closing = total > 0 && done >= total;
    let pass = now - s.since_ns >= shared.cfg.progress || (closing && s.any_passed);
    if pass {
        (s.since_ns, s.any_passed) = (now, true);
    }
    (s.last_done, s.running) = (done, !closing);
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
            let run_thread = if total > 0 { NONE } else { here.2 };
            if due(&here.0, (self.file, self.line, run_thread), done, total) {
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
