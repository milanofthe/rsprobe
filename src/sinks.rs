//! Sinks: where the events of a context go.

use crate::context::{lock, Shared};
use crate::{Event, Kind};
use std::fmt::{self, Display, Formatter};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

/// Receives the events of a context, on the thread that emits them.
pub trait Sink: Send + Sync {
    /// Takes one event; `path` renders the path of its node.
    fn event(&self, event: &Event, path: &Path);
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
pub(crate) struct Line<'a>(pub(crate) &'a Event, pub(crate) &'a Path<'a>);

impl Display for Line<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.0.node {
            0 => write!(f, "{}", self.0.kind),
            _ => write!(f, "{}: {}", self.1, self.0.kind),
        }
    }
}

/// Writes each event to stderr as one line: level, path, text. When stderr
/// is a terminal, progress is one line rewritten in place until the next
/// message.
#[derive(Default)]
pub struct Stderr {
    /// A progress line without a line feed is on the terminal.
    open: AtomicBool,
}

impl Sink for Stderr {
    fn event(&self, e: &Event, path: &Path) {
        let mut w = std::io::stderr().lock();
        let clear = match self.open.swap(false, Relaxed) {
            true => "\r\x1b[2K",
            false => "",
        };
        let (level, line) = (e.level.name(), Line(e, path));
        let _ = match e.kind {
            Kind::Progress { .. } if w.is_terminal() => {
                self.open.store(true, Relaxed);
                write!(w, "{clear}{level} {line}")
            }
            _ => writeln!(w, "{clear}{level} {line}"),
        };
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
}
