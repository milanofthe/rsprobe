//! Messages and progress: node, filters, formatting, warnings, sinks.
#![cfg(feature = "on")]

use rsprobe::{current, debug, info, progress, scope, trace, warn, Buffer, Context, Kind, Level};
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Events of a buffer as (path, text).
fn drained(b: &Buffer) -> Vec<(String, String)> {
    let mut v = Vec::new();
    b.drain(|e, path| v.push((path.to_string(), e.kind.to_string())));
    v
}

#[test]
fn a_message_lands_at_its_node_also_through_a_handle() {
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .progress_interval(0)
        .sink(buffer.clone())
        .build();
    {
        let _on = ctx.enter();
        info!("at the root");
        let _a = scope!("a");
        let h = current();
        std::thread::spawn(move || {
            let _g = h.enter();
            let _b = scope!("b");
            info!("on a worker {}", 7);
        })
        .join()
        .unwrap();
        progress!(3, 4);
    }
    info!("no context: dropped");
    let r = ctx.record();
    let v = drained(&buffer);
    assert_eq!(
        v,
        [
            ("".into(), "at the root".into()),
            ("a/b".into(), "on a worker 7".into()),
            ("a".into(), "3/4".into()),
        ]
    );
    assert!(r.find(&["a", "b"]).is_some());
}

mod numeric {
    pub fn work() {
        let _s = rsprobe::scope!(DEBUG, "fine");
        rsprobe::debug!("numeric");
    }
}

mod other {
    pub fn work() {
        let _s = rsprobe::scope!(DEBUG, "fine");
        rsprobe::debug!("other");
    }
}

#[test]
fn a_filter_switches_scopes_and_messages_of_a_module_together() {
    let buffer = Buffer::default();
    let ctx = Context::builder()
        .filter("events::numeric", Level::DEBUG)
        .filter("events::numeric::deeper", Level::ERROR)
        .filter("events::num", Level::TRACE)
        .sink(buffer.clone())
        .build();
    {
        let _on = ctx.enter();
        for _ in 0..2 {
            numeric::work();
            other::work();
        }
        debug!("root module at INFO");
    }
    let r = ctx.record();
    let n = &r.nodes[r.find(&["fine"]).unwrap() as usize];
    assert_eq!(n.get("calls"), 2);
    assert_eq!(r.nodes.len(), 2);
    let v = drained(&buffer);
    assert_eq!(v, vec![("fine".to_string(), "numeric".to_string()); 2]);
}

/// Counts how often it is formatted.
struct Counted<'a>(&'a AtomicU32);

impl Display for Counted<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        self.0.fetch_add(1, Relaxed);
        f.write_str("x")
    }
}

#[test]
fn a_site_below_the_threshold_formats_nothing() {
    let n = AtomicU32::new(0);
    let ctx = Context::builder().sink(Buffer::default()).build();
    let _on = ctx.enter();
    debug!("{}", Counted(&n));
    trace!("{}", Counted(&n));
    assert_eq!(n.load(Relaxed), 0);
    info!("{}", Counted(&n));
    assert_eq!(n.load(Relaxed), 1);
}

#[test]
fn warnings_are_deduplicated_and_bounded() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _s = scope!("solve");
        for _ in 0..3 {
            warn!("repeated");
        }
        for i in 0..200 {
            warn!("distinct {i}");
        }
        rsprobe::error!("after the bound");
        info!("not a warning");
    }
    let r = ctx.record();
    assert_eq!(r.warnings.len(), 100);
    assert_eq!(r.warnings[0].1, 3);
    assert_eq!(r.warnings[0].0.kind, Kind::Message("repeated".into()));
    assert_eq!(r.warnings[0].0.level, Level::WARN);
    assert_eq!(r.warnings[0].0.node, r.find(&["solve"]).unwrap());
    assert_eq!(r.warnings_dropped, 101 + 1);
    let v: serde_json::Value = serde_json::from_str(&r.to_string()).unwrap();
    assert_eq!(v["warnings"][0]["text"], "repeated");
    assert_eq!(v["warnings"][0]["count"], 3);
    assert_eq!(v["warnings"][0]["level"], "warn");
    assert_eq!(v["warnings_dropped"], 102);
}

/// Nanoseconds, set by the test of the line.
static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[test]
fn a_sink_takes_events_up_to_its_level_as_stamped_lines() {
    let (all, warnings) = (Buffer::default(), Buffer::default());
    let n = AtomicU32::new(0);
    CLOCK.store(5_000_000, Relaxed);
    let ctx = Context::builder()
        .clock(|| CLOCK.load(Relaxed))
        .sink_at(Level::WARN, warnings.clone())
        .build();
    let quiet = Context::builder().sink_at(Level::WARN, all.clone()).build();
    {
        let _on = quiet.enter();
        info!("{}", Counted(&n));
        assert_eq!(n.load(Relaxed), 0, "no sink at INFO: not formatted");
    }
    let _on = ctx.enter();
    let _s = scope!("solve");
    info!("shown to no sink");
    CLOCK.store(5_013_880, Relaxed);
    warn!("small pivot");
    let mut lines = Vec::new();
    warnings.drain(|e, path| lines.push(e.line(&path).to_string()));
    assert_eq!(lines, ["   0.000014 warn  solve: small pivot"]);
}

/// A sink for a reader: the texts it gets.
#[derive(Clone, Default)]
struct Reader(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl rsprobe::Sink for Reader {
    fn event(&self, e: &rsprobe::Event, _: &rsprobe::Path) {
        self.0.lock().unwrap().push(e.kind.to_string());
    }
}

#[test]
fn a_reader_gets_a_repeated_warning_at_powers_of_ten_a_buffer_every_one() {
    let (reader, buffer) = (Reader::default(), Buffer::default());
    let ctx = Context::builder()
        .sink(reader.clone())
        .sink(buffer.clone())
        .build();
    let _on = ctx.enter();
    for _ in 0..12 {
        warn!("repeated");
        info!("each time");
    }
    for i in 0..111 {
        warn!("distinct {i}");
    }
    let texts = reader.0.lock().unwrap().clone();
    assert_eq!(texts.iter().filter(|t| *t == "each time").count(), 12);
    let repeated: Vec<_> = texts.iter().filter(|t| t.starts_with("repeated")).collect();
    assert_eq!(repeated, ["repeated", "repeated (10 times)"]);
    let distinct: Vec<_> = texts.iter().filter(|t| t.starts_with("distinct")).collect();
    assert_eq!(distinct.len(), 99 + 2);
    assert_eq!(distinct[98], "distinct 98");
    assert_eq!(
        distinct[99],
        "distinct 99 (not kept, 1 beyond 100 distinct)"
    );
    assert_eq!(
        distinct[100],
        "distinct 108 (not kept, 10 beyond 100 distinct)"
    );
    let all = drained(&buffer);
    assert_eq!(all.iter().filter(|e| e.1 == "repeated").count(), 12);
    assert_eq!(
        all.iter().filter(|e| e.1.starts_with("distinct")).count(),
        111
    );
    let r = ctx.record();
    assert_eq!((r.warnings[0].1, r.warnings_dropped), (12, 12));
}

#[test]
fn the_buffer_drains_in_order() {
    let buffer = Buffer::default();
    let ctx = Context::builder().sink(buffer.clone()).build();
    let _on = ctx.enter();
    for i in 0..5 {
        info!("{i}");
    }
    let mut v = drained(&buffer);
    info!("5");
    v.extend(drained(&buffer));
    let texts: Vec<String> = v.into_iter().map(|e| e.1).collect();
    assert_eq!(texts, ["0", "1", "2", "3", "4", "5"]);
    assert!(drained(&buffer).is_empty());
}
