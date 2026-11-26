//! The event stream: scope instances, events and `add!` values as Chrome
//! trace events.

use crate::record::{opt, Str};
use crate::Event;
use std::fmt::{self, Display, Formatter};

pub(crate) enum Item {
    Span {
        node: u32,
        start: u64,
        dur: u64,
    },
    Count {
        site: u32,
        node: u32,
        time: u64,
        value: u64,
    },
}

#[derive(Default)]
pub(crate) struct Stream {
    pub(crate) items: Vec<(u32, Item)>,
    pub(crate) events: Vec<(u32, Event)>,
}

struct NsAsUs(u64);

impl Display for NsAsUs {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}.{:03}", self.0 / 1000, self.0 % 1000)
    }
}

impl Stream {
    pub(crate) fn write(
        &self,
        origin: u64,
        name: impl Fn(u32) -> &'static str,
        parent: impl Fn(u32) -> Option<u32>,
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
                    "\"ph\":\"X\",\"name\":{},\"ts\":{},\"dur\":{},\"args\":{{\"node\":{node},\"parent\":{}}}",
                    Str(name(node)),
                    NsAsUs(start - origin),
                    NsAsUs(dur),
                    opt(parent(node))
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
                    NsAsUs(time - origin),
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
                    NsAsUs(e.time),
                    e.node,
                    e.level.name()
                ),
            )
        }));
        let events = out.join(",");
        format!("{{\"displayTimeUnit\":\"ns\",\"traceEvents\":[{events}]}}")
    }
}
