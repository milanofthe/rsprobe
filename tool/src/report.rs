//! `report`: the tree of one record, the medians over several or their sum,
//! and each thread's values.

use crate::constants::OVERHEAD_MARK;
use crate::fmt::{g, name, num};
use crate::overhead::{corrected, cost_share, mixed, share};
use crate::records::{
    median_nodes, preorder, total, union, Map, Nodes, Path, Run, Set, Spread, Values,
};
use crate::stats::{median, sum};
use crate::Result;
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
            share: share(v, &run, "time"),
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
        100.0 * run.get("cost") / raw
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
    let head: String = cols
        .iter()
        .map(|m| format!(" {:>10}", format!("{m} ms")))
        .collect();
    let mut out = vec![
        format!(
            "{k} record{}{merged}. {}{old}",
            if k > 1 { "s" } else { "" },
            ps.collect::<Vec<_>>().join(" ")
        ),
        format!("calibration: inner {inner:.1} ns, outer {outer:.1} ns (mean over records)"),
        format!(
            "run: wall {:.3} ms, thread time {:.3} ms, scope cost {cost:.2}% of the thread time",
            run.get("wall") / 1e6,
            run.get("thread") / 1e6
        ),
        String::new(),
        format!(
            "{:<28} {:>8}{head} {:>10} {:>7} {:>6}   measures",
            "node", "calls", "self ms", "share", "scope"
        ),
    ];
    let mut noisy = union(runs, |r| r.kind("noisy"));
    noisy.extend(["wall", "busy", "pool", "time"]);
    let max = union(runs, |r| r.kind("max"));
    let measure = |r: &Row, m: &str, x: f64| {
        let s = r.spread.iter().find(|s| s.0 == m).map(|s| s.1);
        let hi = s.filter(|s| s.1 != 0.0);
        if max.contains(m) {
            return format!("{m} max {}", x as i64);
        }
        if noisy.contains(m) {
            let ms = match r.measures().any(|v| v.0 == m) {
                true => format!("{m} {:.3} ms", x / 1e6),
                false => m.to_string(),
            };
            return ms
                + &hi.map_or(String::new(), |(lo, hi)| {
                    format!(" [{:.3}..{:.3}]", lo / 1e6, hi / 1e6)
                });
        }
        match hi {
            Some((lo, hi)) => format!("{m}={}..{} differs", lo as i64, hi as i64),
            None => format!("{m}={}", x as i64),
        }
    };
    for r in &rs {
        let spread = ["calls", "time"]
            .into_iter()
            .filter(|m| r.spread.iter().any(|s| s.0 == *m));
        let mut user: Vec<String> = spread.map(|m| measure(r, m, r.v.get(m))).collect();
        user.extend(r.measures().map(|&(m, x)| measure(r, m, x)));
        user.extend(r.ratios.iter().map(|(k, x)| format!("{k} {}", g(*x, 4))));
        let mark = [(r.cost_share > OVERHEAD_MARK, '!'), (r.mixed, '?')];
        let mark: String = mark.iter().map(|m| if m.0 { m.1 } else { ' ' }).collect();
        let times: String = cols
            .iter()
            .map(|m| format!(" {:>10.3}", r.v.get(m) / 1e6))
            .collect();
        let label = format!(
            "{}{}",
            "  ".repeat(r.path.len()),
            r.path.last().map_or("(run)", |s| s.0)
        );
        out.push(
            format!(
                "{label:<28} {:>8}{times} {:>10.3} {:>6.1}% {:>5.1}%{mark} {}",
                r.v.get("calls") as i64,
                r.v.get("self") / 1e6,
                100.0 * r.share,
                100.0 * r.cost_share,
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
        format!("scope: estimated scope cost in the node's recorded wall + busy; ! above {}%. ?: tasks and worker instances of its own, the worker instances below taken as run in its tasks.", (100.0 * OVERHEAD_MARK) as i64),
        "ns/X: time per unit of an exact measure at scopes without tasks; pool/wall: mean workers in its tasks; others with --ratio A/B.".into(),
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
        let (noisy, max) = (r.kind("noisy"), r.kind("max"));
        out.push(format!(
            "{}: {} threads, recorded values (scope cost not subtracted)",
            r.file,
            ts.len()
        ));
        out.push(format!(
            "{:<28} {:>6} {:<7} {:>8} {:>10} {:>10} {:>10} {:>9}  measures",
            "node", "thread", "via", "calls", "wall ms", "busy ms", "pool ms", "load"
        ));
        for p in &paths {
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
            let mut label = format!(
                "{}{}",
                "  ".repeat(p.len()),
                p.last().map_or("(run)", |s| s.0)
            );
            for (t, v) in vs {
                let user =
                    v.0.iter()
                        .filter(|m| !BUILTIN.contains(&m.0) && m.1 != 0.0)
                        .map(|&(m, x)| match (max.contains(m), noisy.contains(m)) {
                            (true, _) => format!("{m} max {}", x as i64),
                            (_, true) => format!("{m} {:.3} ms", x / 1e6),
                            _ => format!("{m}={}", x as i64),
                        });
                let b = work(v);
                let line = format!(
                    "{label:<28} {:>6} {:<7} {:>8} {:>10.3} {:>10.3} {:>10.3} {:>9}  {}",
                    t.index,
                    if t.handle { "handle" } else { "context" },
                    v.get("calls") as i64,
                    v.get("wall") / 1e6,
                    v.get("busy") / 1e6,
                    v.get("pool") / 1e6,
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
