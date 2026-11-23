//! The bridge to the `log` facade, both ways (feature `log`).

use crate::events::emit;
use crate::sinks::Line;
use crate::{Event, Kind, Level, Path, Sink};
use std::cell::Cell;

thread_local! {
    /// Set while [`LogSink`] passes a record to the logger on this thread.
    static FORWARDING: Cell<bool> = const { Cell::new(false) };
}

/// A `log::Log` that turns each record into a message event at the current
/// node, active under the threshold and filters of the current context like a
/// site of the record's level and module. Records [`LogSink`] forwards on the
/// same thread are ignored. Installed with
/// `log::set_logger(&rsprobe::Logger)` and `log::set_max_level`.
pub struct Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() as u8 <= crate::context::MAX.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn log(&self, r: &log::Record) {
        if FORWARDING.get() || !self.enabled(r.metadata()) {
            return;
        }
        let site = (
            Level(r.level() as u8),
            r.module_path_static().unwrap_or(""),
            r.file_static().unwrap_or(""),
            r.line().unwrap_or(0),
        );
        emit(None, site, || Kind::Message(r.args().to_string()));
    }

    fn flush(&self) {}
}

/// Passes each message event to the installed `log` logger as a record of
/// the event's level, module, file and line, with the text prefixed by the
/// node's path. Progress events are not passed.
pub struct LogSink;

impl Sink for LogSink {
    fn event(&self, e: &Event, path: &Path) {
        if !matches!(e.kind, Kind::Message(_)) {
            return;
        }
        use log::Level::*;
        let level = [Error, Warn, Info, Debug, Trace][e.level.0 as usize - 1];
        FORWARDING.set(true);
        log::logger().log(
            &log::Record::builder()
                .args(format_args!("{}", Line(e, path)))
                .level(level)
                .target(e.module)
                .module_path_static(Some(e.module))
                .file_static(Some(e.file))
                .line(Some(e.line))
                .build(),
        );
        FORWARDING.set(false);
    }
}
