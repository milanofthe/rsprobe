//! `report`: the tree of one record, the medians over several or their sum,
//! and each thread's values.

use crate::constants::OVERHEAD_MARK;
use crate::fmt::{apart, name, num};
use crate::overhead;
use crate::overhead::{corrected, cost_share, mixed};
use crate::records::{
    median_nodes, preorder, total, union, Map, Nodes, Path, Run, Set, Spread, Values,
};
use crate::stats::{median, sum};
use crate::Result;
use rsprobe::units::{bytes, count, share, time};
use rsprobe::Param;

/// Values of a node that are not user measures.
pub const BUILTIN: [&str; 9] = [
    "calls",
    "wall",
    "busy",
    "worker_calls",
    "pool",
    "tasks",
    "time",
    "self",
    "cost",
];

/// The corrected nodes and run totals summed over the records, `max`
/// measures by maximum.
pub fn summary(runs: &[Run]) -> (Nodes, Values) {
    let parts: Vec<_> = runs.iter().map(corrected).collect();
    let nodes = total(parts.iter().map(|p| &p.0), &union(runs, |r| r.kind("max")));
    let mut run = Values::default();
    for p in &parts {
        run.merge(&p.1, &Set::new());
    }
    (nodes, run)
}

/// The corrected nodes and run totals over the records: per node and
/// measure the median (zero where a record lacks the node), the maximum for
/// `max` measures; and per node the spread of the others where the records
/// differ.
pub fn medians(runs: &[Run]) -> (Nodes, Values, Map<Path, Spread>) {
    let parts: Vec<_> = runs.iter().map(corrected).collect();
    let (nodes, spread) = median_nodes(parts.iter().map(|p| &p.0), &union(runs, |r| r.kind("max")));
    let mut run = Values::default();
    for &(k, _) in parts.first().map_or(&[][..], |p| &p.1 .0) {
        run.set(
            k,
            median(&parts.iter().map(|p| p.1.get(k)).collect::<Vec<_>>()),
        );
    }
    (nodes, run, spread)
}

/// A row of the report: the node, its values (the [`BUILTIN`] ones, then the
/// user measures), whether the split of the worker instances below it is
/// assumed (see [`mixed`]), its time as a share of the run's thread time,
/// the scope cost as a share of its recorded time, the ratios (see
/// [`ratios`]) and the spread over the records.
#[derive(Clone, Debug)]
pub struct Row {
    pub path: Path,
    pub v: Values,
    pub mixed: bool,
    pub share: f64,
    pub cost_share: f64,
    pub ratios: Vec<(String, f64)>,
    pub spread: Spread,
}

impl Row {
    /// The user measures.
    pub fn measures(&self) -> impl Iterator<Item = &(&'static str, f64)> {
        self.v.0.iter().filter(|m| !BUILTIN.contains(&m.0))
    }
}

/// Time (wall + busy) per unit of each exact user measure in ns where the
/// node has time of its own (`own`), each noisy one (a time) over that time,
/// pool over wall where the node has both (the mean number of workers in its
/// tasks), and each asked (A, B) as `A/B` where B is not zero. A `max`
/// measure is a level: no ratio unless asked.
pub fn ratios(
    v: &Values,
    noisy: &Set,
    own: bool,
    asked: &[(String, String)],
    max: &Set,
) -> Vec<(String, f64)> {
    let t = v.get("time");
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut set = |k: String, x: f64| match out.iter_mut().find(|o| o.0 == k) {
        Some(o) => o.1 = x,
        None => out.push((k, x)),
    };
    for &(m, x) in &v.0 {
        if BUILTIN.contains(&m) || max.contains(m) || x == 0.0 {
            continue;
        }
        if !noisy.contains(m) {
            if own {
                set(format!("ns/{m}"), t / x);
            }
        } else if t != 0.0 {
            set(format!("{m}/time"), x / t);
        }
    }
    if v.get("wall") > 0.0 && v.get("pool") > 0.0 {
        set("pool/wall".into(), v.get("pool") / v.get("wall"));
    }
    for (a, b) in asked {
        if v.get(b) != 0.0 {
            set(format!("{a}/{b}"), v.get(a) / v.get(b));
        }
    }
    out
}

/// `A/B` as the pair (A, B) of measure names.
pub fn asked_ratio(text: &str) -> Result<(String, String)> {
    match text.rsplit_once('/') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() => Ok((a.into(), b.into())),
        _ => Err(format!("expected --ratio A/B, got '{text}'")),
    }
}

/// One row per node in preorder, with shares, the ratios and those asked
/// for: of the values summed over the records, or with `summed` false of
/// their medians, with the spread; and the run's totals.
pub fn rows(runs: &[Run], asked: &[(String, String)], summed: bool) -> (Vec<Row>, Values) {
    let (nodes, run, spread) = match summed {
        true => {
            let (n, r) = summary(runs);
            (n, r, Map::default())
        }
        false => medians(runs),
    };
    let noisy = union(runs, |r| r.kind("noisy"));
    let max = union(runs, |r| r.kind("max"));
    let mut out = Vec::new();
    for p in preorder(&nodes.keys) {
        let v = nodes.get(&p).cloned().unwrap_or_default();
        // Time of its own: a scope with self time and no tasks, so that no
        // part of its work lies in the pool time of its tasks.
        let own = v.get("calls") != 0.0 && v.get("self") > 0.0 && v.get("tasks") == 0.0;
        let mut row = Values(BUILTIN.iter().map(|&m| (m, v.get(m))).collect());
        row.0.extend(v.0.iter().filter(|m| !BUILTIN.contains(&m.0)));
        let v = &v;
        out.push(Row {
            mixed: mixed(v),
            share: overhead::share(v, &run, "time"),
            cost_share: cost_share(v),
            ratios: ratios(v, &noisy, own, asked, &max),
            spread: spread.get(&p).cloned().unwrap_or_default(),
            path: p,
            v: row,
        });
    }
    (out, run)
}

/// Each parameter with its distinct values over the records.
pub fn params(runs: &[Run]) -> Vec<(String, Vec<Param>)> {
    let mut out: Map<String, Vec<Param>> = Map::default();
    for (k, x) in runs.iter().flat_map(|r| &r.record.params) {
        let xs = out.entry(k.clone(), Vec::new);
        if !xs.contains(x) {
            xs.push(x.clone());
        }
    }
    out.keys.into_iter().zip(out.vals).collect()
}

/// A parameter's value as text.
pub fn param(x: &Param) -> String {
    match x {
        Param::Num(x) => num(*x),
        Param::Str(s) => s.clone(),
    }
}

/// The records' warnings summed per node, level and text, in order of
/// first appearance: ([(path, level, text, count)], dropped).
pub fn warnings(runs: &[Run]) -> (Vec<(Path, &'static str, String, u64)>, u64) {
    let mut out: Map<(Path, &'static str, String), u64> = Map::default();
    for (p, level, t, c) in runs.iter().flat_map(Run::warnings) {
        *out.entry((p, level, t), || 0) += c;
    }
    let dropped = runs.iter().map(|r| r.record.warnings_dropped).sum();
    (
        out.iter()
            .map(|((p, l, t), c)| (p.clone(), *l, t.clone(), *c))
            .collect(),
        dropped,
    )
}

/// The allocator's counters in bytes; its `allocs` is a count.
pub const BYTES: [&str; 3] = ["bytes", "freed", "peak"];

/// Widths of the columns of times, calls and shares: the widest time
/// below 1000 hours (`999:59:59`), count below a million (`999,999`) with
/// the name `calls`, share (`100.0 %`).
const TIME_WIDTH: usize = 9;
const CALLS_WIDTH: usize = 8;
const SHARE_WIDTH: usize = 7;

/// A value through its formatter, empty for zero.
pub fn cell<D: std::fmt::Display>(f: fn(f64) -> D, x: f64) -> String {
    if x == 0.0 {
        String::new()
    } else {
        f(x).to_string()
    }
}

/// A node's name indented by its depth, with `mark` after it.
fn indented(p: &Path, mark: &str) -> String {
    let name = p.last().map_or("(run)", |s| s.0);
    format!("{}{name}{mark}", "  ".repeat(p.len()))
}

/// The label of a row: its indented name, ` !` where the scope cost is
/// above [`OVERHEAD_MARK`] of its time and ` ?` where it is [`mixed`].
fn label(r: &Row) -> String {
    let marks = [(r.cost_share > OVERHEAD_MARK, " !"), (r.mixed, " ?")];
    let mark: String = marks.iter().filter(|m| m.0).map(|m| m.1).collect();
    indented(&r.path, &mark)
}

/// A value of measure `m`: a size for the allocator's counters in bytes, a
/// count otherwise.
fn value(m: &str, x: f64) -> String {
    match BYTES.contains(&m) {
        true => bytes(x).to_string(),
        false => count(x).to_string(),
    }
}

/// A measure as text: `NAME max VALUE` for a `max` one, `NAME TIME` for a
/// noisy one, followed by the range over the records `[lo..hi]` where given,
/// and `COUNT NAME` for an exact one (`NAME SIZE` in bytes), `lo..hi` and
/// `differs` where its records differ.
pub fn measure(m: &str, x: f64, range: Option<(f64, f64)>, noisy: &Set, max: &Set) -> String {
    let range = range.filter(|s| s.1 != 0.0);
    if max.contains(m) {
        return format!("{m} max {}", value(m, x));
    }
    if noisy.contains(m) {
        return match range {
            Some((lo, hi)) => format!("{m} {} [{}..{}]", time(x), time(lo), time(hi)),
            None => format!("{m} {}", time(x)),
        };
    }
    let x = match range {
        Some((lo, hi)) => apart(&[lo, hi], |v| value(m, v)).join(".."),
        None => value(m, x),
    };
    let differs = if range.is_some() { " differs" } else { "" };
    match BYTES.contains(&m) {
        true => format!("{m} {x}{differs}"),
        false => format!("{x} {m}{differs}"),
    }
}

/// The report as text.
pub fn text(runs: &[Run], asked: &[(String, String)], summed: bool) -> String {
    let (rs, run) = rows(runs, asked, summed);
    let k = runs.len();
    let merged = match (k > 1, summed) {
        (false, _) => "",
        (true, true) => ", summed",
        (true, false) => ", median over records",
    };
    let cal: Vec<(f64, f64)> = runs.iter().map(Run::calibration).collect();
    let inner = sum(cal.iter().map(|c| c.0)) / k as f64;
    let outer = sum(cal.iter().map(|c| c.1)) / k as f64;
    let ps = params(runs);
    let ps = ps.iter().map(|(n, xs)| {
        let xs: Vec<String> = xs.iter().map(param).collect();
        format!("{n}={}", xs.join(","))
    });
    let raw = run.get("raw");
    let cost = if raw != 0.0 {
        run.get("cost") / raw
    } else {
        0.0
    };
    let old = runs.iter().filter(|r| r.schema == 2).count();
    let old = match old {
        0 => String::new(),
        n => format!(" Schema 2 in {n} of {k}: no split by kind of thread, busy includes the handle guard spans (wall + busy counts a section twice), worker_calls taken as calls where wall is 0."),
    };
    // The time columns any node has a value in.
    let cols: Vec<&str> = ["wall", "busy", "pool"]
        .into_iter()
        .filter(|m| rs.iter().any(|r| r.v.get(m) != 0.0))
        .collect();
    let labels: Vec<String> = rs.iter().map(label).collect();
    let w = labels
        .iter()
        .map(String::len)
        .fold("node".len(), usize::max);
    let head: String = cols.iter().map(|m| format!(" {m:>TIME_WIDTH$}")).collect();
    let mut out = vec![
        format!(
            "{k} record{}{merged}. {}{old}",
            if k > 1 { "s" } else { "" },
            ps.collect::<Vec<_>>().join(" ")
        ),
        format!(
            "calibration: inner {}, outer {} (mean over records)",
            time(inner),
            time(outer)
        ),
        format!(
            "run: wall {}, thread time {}, scope cost {}",
            time(run.get("wall")),
            time(run.get("thread")),
            share(cost)
        ),
        String::new(),
        format!(
            "{:<w$} {:>CALLS_WIDTH$}{head} {:>TIME_WIDTH$} {:>SHARE_WIDTH$}   measures",
            "node", "calls", "self", "share"
        ),
    ];
    let mut noisy = union(runs, |r| r.kind("noisy"));
    noisy.extend(["wall", "busy", "pool", "time"]);
    let max = union(runs, |r| r.kind("max"));
    for (r, label) in rs.iter().zip(&labels) {
        let ratio = |k: &str| r.ratios.iter().find(|x| x.0 == k).map(|x| x.1);
        let range = |m: &str| r.spread.iter().find(|s| s.0 == m).map(|s| s.1);
        let mut user: Vec<String> = ratio("pool/wall")
            .map(|x| format!("{} workers", count(x)))
            .into_iter()
            .collect();
        let spread = ["calls", "time"]
            .into_iter()
            .filter_map(|m| range(m).map(|s| measure(m, r.v.get(m), Some(s), &noisy, &max)));
        user.extend(spread);
        user.extend(r.measures().map(|&(m, x)| {
            let text = measure(m, x, range(m), &noisy, &max);
            match (ratio(&format!("ns/{m}")), ratio(&format!("{m}/time"))) {
                (Some(t), _) => format!("{text}, {} each", time(t)),
                (_, Some(s)) => format!("{text} ({})", share(s)),
                _ => text,
            }
        }));
        user.extend(asked.iter().filter_map(|(a, b)| {
            let k = format!("{a}/{b}");
            ratio(&k).map(|x| format!("{k} {}", count(x)))
        }));
        let times: String = cols
            .iter()
            .map(|m| format!(" {:>TIME_WIDTH$}", cell(time, r.v.get(m))))
            .collect();
        out.push(
            format!(
                "{label:<w$} {:>CALLS_WIDTH$}{times} {:>TIME_WIDTH$} {:>SHARE_WIDTH$}   {}",
                cell(count, r.v.get("calls")),
                cell(time, r.v.get("self")),
                share(r.share),
                user.join(", ")
            )
            .trim_end()
            .to_string(),
        );
    }
    out.extend([
        String::new(),
        "wall, busy: time of the node's scope instances on threads entered through the context and through a handle; pool: time of the tasks (handle guards) entered at the node; thread time: wall of the run + pool of all nodes.".into(),
        "Scope cost subtracted, full = inner + outer: wall - (calls - worker_calls) inner - (calling-thread instances below) full; busy - worker_calls inner - (worker instances below, down to nodes with tasks) full; pool - (those instances below a node with tasks) full.".into(),
        "time = wall + busy; self = time - wall of the children - busy of the children of a node without tasks; share: time over the thread time.".into(),
        format!("!: estimated scope cost above {} of the node's recorded wall + busy. ?: tasks and worker instances of its own, the worker instances below taken as run in its tasks.", share(OVERHEAD_MARK)),
        "COUNT NAME, TIME each: an exact measure and the time per unit at scopes without tasks; NAME TIME (SHARE): a time! measure and its share of the node's time; NAME max VALUE: a max! measure; N workers: pool/wall, the mean workers in its tasks; others with --ratio A/B.".into(),
    ]);
    if k > 1 && !summed {
        out.push(
            "Medians over the records, [min..max] where they differ; an exact measure that differs is shown as its range; a max measure is the largest value. --sum: the sums."
                .into(),
        );
    }
    let (ws, dropped) = warnings(runs);
    if !ws.is_empty() || dropped > 0 {
        // The node column as wide as its longest entry.
        let names: Vec<String> = ws.iter().map(|w| name(w.0.iter().map(|s| s.0))).collect();
        let w = names.iter().map(String::len).fold("node".len(), usize::max);
        out.push(String::new());
        out.push(format!("warnings{}:", if k > 1 { ", summed" } else { "" }));
        out.push(format!(
            "{:>8}  {:<w$} {:<6} text",
            "count", "node", "level"
        ));
        for ((_, level, t, c), n) in ws.iter().zip(&names) {
            out.push(format!("{c:>8}  {n:<w$} {level:<6} {t}"));
        }
        if dropped > 0 {
            out.push(format!("{dropped:>8}  not kept, beyond the record's bound"));
        }
    }
    out.join("\n")
}

/// Folded stacks, one line per node with its self time from wall + busy in
/// ns: the flame graph's widths are thread time in scope instances.
pub fn folded(runs: &[Run]) -> String {
    let (nodes, _) = summary(runs);
    let lines = preorder(&nodes.keys).into_iter().filter_map(|p| {
        let own = nodes
            .get(&p)
            .map_or(0.0, |v| v.get("self"))
            .round_ties_even();
        let stack: Vec<&str> = std::iter::once("run")
            .chain(p.iter().map(|s| s.0))
            .collect();
        (own > 0.0).then(|| format!("{} {}", stack.join(";"), own as i64))
    });
    lines.collect::<Vec<_>>().join("\n")
}

/// Per record, node (the one named by its path `a/b`, or all) and thread
/// the recorded values of each thread, with its worker time (busy + pool)
/// over the mean of the threads with worker time at that node.
pub fn threads(runs: &[Run], node: Option<&str>) -> Result<String> {
    let mut out = Vec::new();
    for r in runs {
        let ts = r.per_thread();
        if ts.is_empty() {
            return Err(format!(
                "{}: no per-thread values; take the record with Context::record_threads()",
                r.file
            ));
        }
        let paths = preorder(ts.iter().flat_map(|t| &t.1.keys));
        let named = |p: &Path| name(p.iter().map(|s| s.0));
        let paths: Vec<Path> = paths
            .into_iter()
            .filter(|p| node.is_none_or(|n| named(p) == n))
            .collect();
        if paths.is_empty() {
            return Err(format!("{}: no node {}", r.file, node.unwrap_or("None")));
        }
        let (mut noisy, max) = (r.kind("noisy"), r.kind("max"));
        noisy.extend(["wall", "busy", "pool"]);
        out.push(format!(
            "{}: {} threads, recorded values (scope cost not subtracted)",
            r.file,
            ts.len()
        ));
        let labels: Vec<String> = paths.iter().map(|p| indented(p, "")).collect();
        let w = labels
            .iter()
            .map(String::len)
            .fold("node".len(), usize::max);
        let head: String = ["wall", "busy", "pool"]
            .iter()
            .map(|m| format!(" {m:>TIME_WIDTH$}"))
            .collect();
        out.push(format!(
            "{:<w$} {:>6} {:<7} {:>CALLS_WIDTH$}{head} {:>9}  measures",
            "node", "thread", "via", "calls", "load"
        ));
        for (p, label) in paths.iter().zip(labels) {
            let vs: Vec<_> = ts
                .iter()
                .filter_map(|(t, n)| n.get(p).map(|v| (t, v)))
                .collect();
            let work = |v: &Values| v.get("busy") + v.get("pool");
            let busy: Vec<f64> = vs.iter().map(|v| work(v.1)).filter(|&b| b > 0.0).collect();
            let mean = if busy.is_empty() {
                0.0
            } else {
                sum(busy.iter().copied()) / busy.len() as f64
            };
            let mut label = label;
            for (t, v) in vs {
                let user =
                    v.0.iter()
                        .filter(|m| !BUILTIN.contains(&m.0) && m.1 != 0.0)
                        .map(|&(m, x)| measure(m, x, None, &noisy, &max));
                let times: String = ["wall", "busy", "pool"]
                    .iter()
                    .map(|m| format!(" {:>TIME_WIDTH$}", cell(time, v.get(m))))
                    .collect();
                let b = work(v);
                let line = format!(
                    "{label:<w$} {:>6} {:<7} {:>CALLS_WIDTH$}{times} {:>9}  {}",
                    t.index,
                    if t.handle { "handle" } else { "context" },
                    cell(count, v.get("calls")),
                    if b != 0.0 {
                        format!("{:.3}", b / mean)
                    } else {
                        String::new()
                    },
                    user.collect::<Vec<_>>().join(", ")
                );
                out.push(line.trim_end().to_string());
                label.clear();
            }
        }
        out.push(String::new());
    }
    out.pop();
    Ok(out.join("\n"))
}
