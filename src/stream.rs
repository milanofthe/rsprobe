//! The event stream: scope instances, events and `add!` values as Chrome
//! trace events.

use crate::record::Str;
use crate::Event;
use std::fmt::{self, Display, Formatter};

/// One entry of a thread's stream buffer.
pub(crate) enum Item {
    /// A guard's span at a node.
    Span { node: u32, start: u64, dur: u64 },
    /// The thread's value of an `add!` site at a node after an add; a
    /// counter series per node and thread.
    Count {
        site: u32,
        node: u32,
        time: u64,
        value: u64,
    },
}

/// The stream of a context: items flushed by the threads, and events.
#[derive(Default)]
pub(crate) struct Stream {
    pub(crate) items: Vec<(u32, Item)>,
    pub(crate) events: Vec<(u32, Event)>,
}

/// Nanoseconds as microseconds with three decimals.
struct Us(u64);

impl Display for Us {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}.{:03}", self.0 / 1000, self.0 % 1000)
    }
}

impl Stream {
    /// The Chrome trace event JSON: one "M" event per thread, "X" per span,
    /// "i" per event, "C" per `add!` (one series per node and thread). `name`
    /// names a node, `measure` an `add!` site.
    pub(crate) fn write(
        &self,
        name: impl Fn(u32) -> &'static str,
        measure: impl Fn(u32) -> &'static str,
    ) -> String {
        let mut threads: Vec<u32> = self.items.iter().map(|i| i.0).collect();
        threads.extend(self.events.iter().map(|e| e.0));
        threads.sort_unstable();
        threads.dedup();
        let ev = |tid: u32, body: fmt::Arguments| format!("{{\"pid\":1,\"tid\":{tid},{body}}}");
        let mut out: Vec<String> = threads
            .into_iter()
            .map(|t| {
                let args = format!("{{\"name\":\"thread {t}\"}}");
                ev(
                    t,
                    format_args!("\"ph\":\"M\",\"name\":\"thread_name\",\"args\":{args}"),
                )
            })
            .collect();
        out.extend(self.items.iter().map(|&(t, ref i)| match *i {
            Item::Span { node, start, dur } => ev(
                t,
                format_args!(
                    "\"ph\":\"X\",\"name\":{},\"ts\":{},\"dur\":{},\"args\":{{\"node\":{node}}}",
                    Str(name(node)),
                    Us(start),
                    Us(dur)
                ),
            ),
            Item::Count {
                site,
                node,
                time,
                value,
            } => ev(
                t,
                format_args!(
                    "\"ph\":\"C\",\"name\":{},\"ts\":{},\"args\":{{{}:{value}}}",
                    Str(measure(site)),
                    Us(time),
                    Str(&format!("{} {t}", name(node)))
                ),
            ),
        }));
        out.extend(self.events.iter().map(|(t, e)| {
            ev(
                *t,
                format_args!(
                    "\"ph\":\"i\",\"s\":\"t\",\"name\":{},\"ts\":{},\"args\":{{\"node\":{},\"level\":\"{}\"}}",
                    Str(&e.kind.to_string()),
                    Us(e.time),
                    e.node,
                    e.level.name()
                ),
            )
        }));
        let events = out.join(",");
        format!("{{\"displayTimeUnit\":\"ns\",\"traceEvents\":[{events}]}}")
    }
}
