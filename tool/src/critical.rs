//! `critical`: the chain of spans that bounds a scope instance's elapsed
//! time, from the trace a context built with `.stream()` writes.

use crate::fmt::name;
use crate::json::Json;
use crate::records::{get, Map};
use crate::Result;
use std::rc::Rc;

/// A scope instance of the trace (an "X" event): node id, the node's parent
/// (none at the root), scope name, thread, start and end in ns.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub node: u64,
    pub parent: Option<u64>,
    pub name: String,
    pub tid: u64,
    pub start: i64,
    pub end: i64,
}

/// The spans of a trace document.
pub fn parse(text: &str) -> Result<Vec<Span>> {
    let doc = Json::parse(text)?;
    let events = doc.get("traceEvents").and_then(Json::arr).ok_or("no traceEvents")?;
    let mut spans = Vec::new();
    for e in events.iter().filter(|e| e.get("ph").and_then(Json::str) == Some("X")) {
        let args = get(e, "args", Some)?;
        let parent = args
            .get("parent")
            .ok_or("a span without its node's parent, from a crate before the field")?;
        let ns = |k| get(e, k, |x| x.f64().map(|x| (x * 1000.0).round_ties_even() as i64));
        let (node, name, tid) = (
            get(args, "node", Json::u64)?,
            get(e, "name", Json::str)?.into(),
            get(e, "tid", Json::u64)?,
        );
        let start = ns("ts")?;
        spans.push(Span { node, parent: parent.u64(), name, tid, start, end: start + ns("dur")? });
    }
    Ok(spans)
}

/// The spans of a trace file.
pub fn load(path: &str) -> Result<Vec<Span>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse(&text).map_err(|e| format!("{path}: {e}"))
}

/// A path of scope names from the root.
pub type Names = Vec<String>;

/// {node id: path}; a node whose parent has no span hangs at the root.
fn paths(spans: &[Span]) -> Map<u64, Names> {
    let mut names: Map<u64, (&str, Option<u64>)> = Map::default();
    for s in spans {
        *names.entry(s.node, || (&s.name, s.parent)) = (&s.name, s.parent);
    }
    let path = |mut n: u64| {
        let mut p = Names::new();
        while let Some(&(name, Some(up))) = names.get(&n) {
            p.push(name.to_string());
            n = up;
        }
        p.reverse();
        p
    };
    let mut out = Map::default();
    names.keys.iter().for_each(|&n| _ = out.entry(n, || path(n)));
    out
}

/// An element of a chain: a span (its index) or a gap (start, end).
#[derive(Clone, Copy, Debug)]
enum Element {
    Span(usize),
    Gap(i64, i64),
}

/// The critical chain of `inst` among `kids` (the spans of its node's
/// children, by start): from inst's end back, the span lying inside inst on
/// any thread that ends last at or before the current time, then on from
/// that span's start. The elements in time order, gaps between them.
fn chain(spans: &[Span], kids: &[usize], inst: &Span) -> Vec<Element> {
    let lo = kids.partition_point(|&k| spans[k].start < inst.start);
    let hi = kids.partition_point(|&k| spans[k].start <= inst.end);
    let mut inside: Vec<usize> =
        kids[lo..hi.max(lo)].iter().copied().filter(|&k| spans[k].end <= inst.end).collect();
    inside.sort_by_key(|&k| (-spans[k].end, spans[k].start));
    let (mut t, mut out) = (inst.end, Vec::new());
    for k in inside {
        let s = &spans[k];
        if s.end > t {
            continue;
        }
        if s.end < t {
            out.push(Element::Gap(s.end, t));
        }
        out.push(Element::Span(k));
        t = s.start;
    }
    if t > inst.start {
        out.push(Element::Gap(inst.start, t));
    }
    out.reverse();
    out
}

/// A row of a chain: level from 1, the element's path and thread (none for a
/// gap), start and duration in ns, share of the enclosing element's time.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub level: usize,
    pub path: Option<Names>,
    pub tid: Option<u64>,
    pub start: i64,
    pub dur: i64,
    pub share: f64,
}

/// The chain of one instance (the longest span of the scope): the instance,
/// the scope's path, the rows in time order with each element's own chain
/// after it, and per path the time on the chain where it is the innermost
/// element followed (a gap counts to the element enclosing it), largest
/// first.
#[derive(Clone, Debug)]
pub struct Critical {
    pub instance: Span,
    pub path: Names,
    pub rows: Vec<Row>,
    pub totals: Vec<(Names, i64)>,
}

/// Per chosen instance (the longest span of the node at path `node`, a/b,
/// else the longest of each top-level scope) its chain to `depth` levels.
pub fn critical(spans: &[Span], node: Option<&str>, depth: usize) -> Result<Vec<Critical>> {
    let at = paths(spans);
    let path = |s: &Span| at.get(&s.node).cloned().unwrap_or_default();
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&i| spans[i].start);
    let mut kids: Map<Option<u64>, Vec<usize>> = Map::default();
    for &i in &order {
        kids.entry(spans[i].parent, Vec::new).push(i);
    }
    let chosen: Vec<Names> = match node {
        None => {
            let mut top: Map<Names, i64> = Map::default();
            for s in spans.iter().filter(|s| path(s).len() == 1) {
                let t = top.entry(path(s), || s.start);
                *t = (*t).min(s.start);
            }
            let mut c: Vec<(Names, i64)> = top.keys.into_iter().zip(top.vals).collect();
            c.sort_by_key(|c| c.1);
            c.into_iter().map(|c| c.0).collect()
        }
        Some(n) => {
            let want: Names = n.split('/').map(String::from).collect();
            if !at.vals.contains(&want) {
                return Err(format!("no span of {n} in the trace"));
            }
            vec![want]
        }
    };
    let within = |el: &Span| chain(spans, kids.get(&Some(el.node)).map_or(&[][..], |v| v), el);
    let mut out = Vec::new();
    for p in chosen {
        // The first of the longest, by (duration, -start).
        let key = |s: &Span| (s.end - s.start, -s.start);
        let longest =
            spans
                .iter()
                .filter(|s| path(s) == p)
                .reduce(|i, s| if key(s) > key(i) { s } else { i });
        let Some(inst) = longest else { continue };
        let (mut rows, mut totals) = (Vec::new(), Map::<Names, i64>::default());
        let mut stack = vec![(inst, Rc::new(within(inst)), 0usize, 1usize)];
        // Depth first: each element's own chain right after its row.
        while let Some((el, elements, i, level)) = stack.pop() {
            let Some(&e) = elements.get(i) else { continue };
            stack.push((el, Rc::clone(&elements), i + 1, level));
            let dur = el.end - el.start;
            let (s, tid, start, d, inner) = match e {
                Element::Gap(a, b) => (None, None, a, b - a, Vec::new()),
                Element::Span(k) => {
                    let s = &spans[k];
                    (
                        Some(s),
                        Some(s.tid),
                        s.start,
                        s.end - s.start,
                        if level < depth { within(s) } else { Vec::new() },
                    )
                }
            };
            let share = if dur != 0 { d as f64 / dur as f64 } else { 0.0 };
            rows.push(Row { level, path: s.map(path), tid, start, dur: d, share });
            match (s, inner.iter().any(|x| matches!(x, Element::Span(_)))) {
                (Some(s), true) => stack.push((s, Rc::new(inner), 0, level + 1)),
                _ => *totals.entry(path(s.unwrap_or(el)), || 0) += d,
            }
        }
        let mut totals: Vec<(Names, i64)> = totals.keys.into_iter().zip(totals.vals).collect();
        totals.sort_by_key(|t| -t.1);
        out.push(Critical { instance: inst.clone(), path: p, rows, totals });
    }
    Ok(out)
}

/// The chains as text.
pub fn text(spans: &[Span], node: Option<&str>, depth: usize) -> Result<String> {
    let mut out = Vec::new();
    for c in critical(spans, node, depth)? {
        let inst = &c.instance;
        let el = inst.end - inst.start;
        out.push(format!(
            "{}: the instance on thread {} at {:.3} ms, elapsed {:.3} ms; its chain to depth {depth}, share of the enclosing element's time:",
            name(&c.path),
            inst.tid,
            inst.start as f64 / 1e6,
            el as f64 / 1e6
        ));
        out.push(format!(
            "  {:<36} {:>9} {:>9} {:>6} {:>6}",
            "element", "start ms", "time ms", "share", "thread"
        ));
        for r in &c.rows {
            let label = r.path.as_ref().and_then(|p| p.last()).map_or("(gap)", |s| s);
            let label = format!("{}{label}", "  ".repeat(r.level - 1));
            let line = format!(
                "  {label:<36} {:>9.3} {:>9.3} {:>5.1}% {:>6}",
                (r.start - inst.start) as f64 / 1e6,
                r.dur as f64 / 1e6,
                100.0 * r.share,
                r.tid.map_or(String::new(), |t| t.to_string())
            );
            out.push(line.trim_end().to_string());
        }
        out.push("  per node over the chain (a gap counts to the element it lies in):".into());
        for (p, t) in &c.totals {
            let share = if el != 0 { 100.0 * *t as f64 / el as f64 } else { 0.0 };
            out.push(format!("  {:<36} {:>9} {:>9.3} {share:>5.1}%", name(p), "", *t as f64 / 1e6));
        }
    }
    Ok(out.join("\n"))
}
