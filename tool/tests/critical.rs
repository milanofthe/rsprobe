//! critical on a trace built by hand with a known critical path.

use rsprobe_tool::critical::{self, critical, Span};
use rsprobe_tool::fmt::name;

/// A phase on thread 0 over 0..100 us whose tasks run on two workers:
/// thread 1 runs the long chain 5..40 and 42..90 (with a kernel 50..80
/// inside the second), thread 2 the short one 6..30 and 31..60. A second
/// top-level scope follows the phase. Name, node, parent, [tid, ts, dur (us)].
const SPANS: [(&str, u32, Option<u32>, [u32; 3]); 8] = [
    ("root", 0, None, [0, 0, 120]),
    ("phase", 1, Some(0), [0, 0, 100]),
    ("task", 2, Some(1), [1, 5, 35]),
    ("task", 2, Some(1), [1, 42, 48]),
    ("kernel", 3, Some(2), [1, 50, 30]),
    ("task", 2, Some(1), [2, 6, 24]),
    ("task", 2, Some(1), [2, 31, 29]),
    ("after", 4, Some(0), [0, 105, 10]),
];

fn spans() -> Vec<Span> {
    let mut events: Vec<String> = (0..3)
        .map(|t| format!(r#"{{"pid":1,"tid":{t},"ph":"M","name":"thread_name","args":{{"name":"thread {t}"}}}}"#))
        .collect();
    events.extend(SPANS.iter().map(|(n, node, parent, [t, ts, d])| {
        let parent = parent.map_or("null".into(), |p| p.to_string());
        format!(r#"{{"pid":1,"tid":{t},"ph":"X","name":"{n}","ts":{ts},"dur":{d},"args":{{"node":{node},"parent":{parent}}}}}"#)
    }));
    critical::parse(&format!(r#"{{"displayTimeUnit":"ns","traceEvents":[{}]}}"#, events.join(",")))
        .unwrap()
}

/// (last name or none for a gap, thread, start and duration in us) per row
/// of a level.
fn rows(c: &critical::Critical, level: usize) -> Vec<(Option<&str>, Option<u64>, i64, i64)> {
    let rows = c.rows.iter().filter(|r| r.level == level);
    rows.map(|r| {
        (r.path.as_ref().map(|p| p[p.len() - 1].as_str()), r.tid, r.start / 1000, r.dur / 1000)
    })
    .collect()
}

fn totals(c: &critical::Critical) -> Vec<(String, i64)> {
    c.totals.iter().map(|(p, t)| (name(p), *t)).collect()
}

#[test]
fn chain_follows_the_long_worker() {
    let c = &critical(&spans(), Some("phase"), 1).unwrap()[0];
    let expected = [
        (None, None, 0, 5),
        (Some("task"), Some(1), 5, 35),
        (None, None, 40, 2),
        (Some("task"), Some(1), 42, 48),
        (None, None, 90, 10),
    ];
    assert_eq!(rows(c, 1), expected);
    assert!((c.rows[3].share - 0.48).abs() < 1e-12);
    assert_eq!(totals(c), [("phase/task".to_string(), 83000), ("phase".to_string(), 17000)]);
}

#[test]
fn depth_follows_into_the_elements() {
    let c = &critical(&spans(), Some("phase"), 2).unwrap()[0];
    let inner: Vec<_> = rows(c, 2).into_iter().map(|r| (r.0, r.2, r.3)).collect();
    assert_eq!(inner, [(None, 42, 8), (Some("kernel"), 50, 30), (None, 80, 10)]);
    let t = totals(c);
    let at = |p: &str| t.iter().find(|x| x.0 == p).unwrap().1;
    assert_eq!((at("phase/task/kernel"), at("phase/task")), (30000, 53000));
    assert_eq!(t.iter().map(|x| x.1).sum::<i64>(), 100000);
}

#[test]
fn default_is_each_top_level_scope() {
    let got: Vec<String> =
        critical(&spans(), None, 2).unwrap().iter().map(|c| name(&c.path)).collect();
    assert_eq!(got, ["phase", "after"]);
    assert!(critical::text(&spans(), Some("phase"), 2).unwrap().contains("phase/task"));
    assert!(critical(&spans(), Some("phase/none"), 2).is_err());
}

#[test]
fn a_trace_without_parents_is_refused() {
    let doc = r#"{"traceEvents":[{"pid":1,"tid":0,"ph":"X","name":"a","ts":0,"dur":1,"args":{"node":1}}]}"#;
    assert!(critical::parse(doc).is_err());
}
