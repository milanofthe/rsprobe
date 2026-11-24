//! Sinks: where the events of a context go.

use crate::context::Shared;
use crate::site::lock;
use crate::{Event, Kind, Level};
use std::fmt::{self, Display, Formatter};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Receives the events of a context, on the thread that emits them.
pub trait Sink: Send + Sync {
    /// Takes one event; `path` renders the path of its node.
    fn event(&self, event: &Event, path: &Path);

    /// Whether the sink takes every event. `false`, the default, is for a
    /// sink that writes for a reader: of the events at WARN and above with
    /// the same site and text it gets the first, the tenth, the hundredth
    /// and so on, the count appended to the text (`small pivot (10 times)`).
    fn every(&self) -> bool {
        false
    }
}

/// The path of a node: `Display` writes the scope names from the root to the
/// node, joined by `/`, and nothing for the root. Writing it locks the
/// context's tree.
pub struct Path<'a> {
    pub(crate) shared: &'a Shared,
    pub(crate) node: u32,
}

impl Display for Path<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.shared.path(self.node, f)
    }
}

/// `path: text`, or `text` at the root.
pub(crate) struct Line<'a, P>(pub(crate) &'a Event, pub(crate) &'a P);

impl<P: Display> Display for Line<'_, P> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.node {
            0 => write!(f, "{}", self.0.kind),
            _ => write!(f, "{}: {}", self.1, self.0.kind),
        }
    }
}

/// Seconds since the context was built, level and [`Line`].
struct Stamped<'a, P>(&'a Event, &'a P);

impl<P: Display> Display for Stamped<'_, P> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let (s, level) = (self.0.time as f64 * 1e-9, self.0.level.name());
        write!(f, "{s:11.6} {level:<5} {}", Line(self.0, self.1))
    }
}

impl Event {
    /// The event at `path` without time and level, for a host that shows
    /// those itself: `a/b: text`, the text alone at the root.
    pub fn text<'a, P: Display>(&'a self, path: &'a P) -> impl Display + 'a {
        Line(self, path)
    }

    /// The line [`Stderr`] writes for the event at `path`, for a host that
    /// drains a [`Buffer`]: `   0.013880 warn  a/b: text`.
    pub fn line<'a, P: Display>(&'a self, path: &'a P) -> impl Display + 'a {
        Stamped(self, path)
    }
}

/// Writes each event to stderr as its [`line`](Event::line). When stderr is a terminal, progress is one line
/// rewritten in place until the next message, and a line is set in the style
/// of its level unless `NO_COLOR` is set.
#[derive(Default)]
pub struct Stderr {
    /// A progress line without a line feed is on the terminal.
    open: AtomicBool,
}

/// ANSI style of a terminal line by level: ERROR red, WARN yellow, DEBUG and
/// TRACE dim.
fn style(level: Level) -> &'static str {
    match level {
        Level::ERROR => "\x1b[31m",
        Level::WARN => "\x1b[33m",
        Level::INFO => "",
        _ => "\x1b[2m",
    }
}

impl Sink for Stderr {
    fn event(&self, e: &Event, path: &Path) {
        let mut w = std::io::stderr().lock();
        let terminal = w.is_terminal();
        let clear = match self.open.swap(false, Relaxed) {
            true => "\r\x1b[2K",
            false => "",
        };
        let on = match terminal && std::env::var_os("NO_COLOR").is_none() {
            true => style(e.level),
            false => "",
        };
        let off = if on.is_empty() { "" } else { "\x1b[0m" };
        let open = terminal && matches!(e.kind, Kind::Progress { .. });
        self.open.store(open, Relaxed);
        let end = if open { "" } else { "\n" };
        let _ = write!(w, "{clear}{on}{}{off}{end}", e.line(path));
    }
}

/// Keeps events with their rendered path until [`drain`](Buffer::drain).
/// Clones share one buffer: a host that must not be called from worker
/// threads (a Python interpreter, a wasm host) attaches a clone to the
/// context and drains the buffer on its own thread.
#[derive(Clone, Default)]
pub struct Buffer(Arc<Mutex<Vec<(Event, String)>>>);

impl Buffer {
    /// Removes the events kept so far and passes each with its path to `f`,
    /// in the order they were kept.
    pub fn drain(&self, mut f: impl FnMut(&Event, &str)) {
        let events = std::mem::take(&mut *lock(&self.0));
        for (e, path) in &events {
            f(e, path);
        }
    }
}

impl Sink for Buffer {
    fn event(&self, e: &Event, path: &Path) {
        let path = path.to_string();
        lock(&self.0).push((e.clone(), path));
    }

    /// Every event: what a host does with a repeated one is its own rule.
    fn every(&self) -> bool {
        true
    }
}
