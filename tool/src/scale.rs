//! `scale`: records over a run parameter, a problem size or a thread count,
//! or each node over a measure of its own.

use crate::constants::{DEPARTURE, FOLLOWS, MIN_FIT_POINTS};
use crate::fmt::{g, path};
use crate::overhead::{corrected, domain};
use crate::records::{median_nodes, preorder, union, Nodes, Path, Run, Set, Values};
use crate::stats::{karp_flatt, local_exponent, power_fit};
use crate::Result;
use rsprobe::Param;

/// A fit of c n^p log^q(n) to a measure at a node: node, measure, c, p, q,
/// residual.
pub type FitRow = (Path, &'static str, f64, f64, i32, f64);

/// The parameter's values in ascending order and per value the nodes with
/// the median of each measure over the records of that value.
pub fn points(runs: &[Run], by: &str) -> Result<Vec<(f64, Nodes)>> {
    let mut groups: Vec<(f64, Vec<Nodes>)> = Vec::new();
    for r in runs {
        let x = r.record.params.iter().find(|p| p.0 == by).map(|p| &p.1);
        let Some(&Param::Num(x)) = x.filter(|x| matches!(x, Param::Num(v) if v.is_finite())) else {
            return Err(format!("{}: no numeric parameter '{by}'", r.file));
        };
        match groups.iter_mut().find(|g| g.0 == x) {
            Some(g) => g.1.push(corrected(r).0),
            None => groups.push((x, vec![corrected(r).0])),
        }
    }
    groups.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(groups
        .into_iter()
        .map(|(x, recs)| (x, median_nodes(recs.iter(), &Set::new()).0))
        .collect())
}

/// (measure, c, p, q, residual) of the fit of c n^p log^q(n) to each of the
/// measures over a node's values vs at n; a measure zero or absent at any
/// point is not fitted.
fn node_fits(
    n: &[f64],
    vs: &[&Values],
    measures: &[&'static str],
) -> Vec<(&'static str, f64, f64, i32, f64)> {
    let fit = |&m: &&'static str| {
        let y: Vec<f64> = vs.iter().map(|v| v.get(m)).collect();
        y.iter().all(|&y| y > 0.0).then(|| {
            let (c, p, q, res) = power_fit(n, &y);
            (m, c, p, q, res)
        })
    };
    measures.iter().filter_map(fit).collect()
}

/// The fits over a run parameter n: its values, the fits per node (time in
/// its domain, calls, the exact measures), the dominant exponent (the local
/// exponent at the largest n of the node with the largest self time there),
/// that node, and the other scopes whose local exponent departs from it by
/// more than DEPARTURE, with theirs.
#[derive(Clone, Debug)]
pub struct Fits {
    pub n: Vec<f64>,
    pub rows: Vec<FitRow>,
    pub run: Option<f64>,
    pub dominant: Option<Path>,
    pub departs: Vec<(Path, f64)>,
}

/// Per node and measure the fit of c n^p log^q(n) over the parameter n.
/// Nodes absent at a point are not fitted.
pub fn fits(runs: &[Run], by: &str) -> Result<Fits> {
    let pts = points(runs, by)?;
    let n: Vec<f64> = pts.iter().map(|p| p.0).collect();
    if n.len() < MIN_FIT_POINTS {
        return Err(format!(
            "{} values of {by}; a fit needs at least {MIN_FIT_POINTS}",
            n.len()
        ));
    }
    let mut exact = union(runs, |r| r.kind("exact"));
    exact.remove("calls");
    let (mut rows, mut local) = (Vec::new(), Vec::<(Path, f64, f64)>::new());
    for p in preorder(pts.iter().flat_map(|x| &x.1.keys)) {
        let Some(vs) = pts.iter().map(|x| x.1.get(&p)).collect::<Option<Vec<_>>>() else {
            continue;
        };
        let last = vs[vs.len() - 1];
        let time = domain(last);
        let mut ms = vec![time, "calls"];
        ms.extend(exact.iter().filter(|m| last.has(m)));
        for (m, c, e, q, res) in node_fits(&n, &vs, &ms) {
            rows.push((p.clone(), m, c, e, q, res));
            if m == time {
                local.push((
                    p.clone(),
                    local_exponent(e, q, n[n.len() - 1]),
                    last.get("self"),
                ));
            }
        }
    }
    let dominant = local
        .iter()
        .fold(None::<&(Path, f64, f64)>, |b, l| match b {
            Some(b) if b.2 >= l.2 => Some(b),
            _ => Some(l),
        });
    let run = dominant.map(|d| d.1);
    let departs = local
        .iter()
        .filter(|l| !l.0.is_empty() && run.is_some_and(|r| (l.1 - r).abs() > DEPARTURE));
    Ok(Fits {
        departs: departs.map(|l| (l.0.clone(), l.1)).collect(),
        dominant: dominant.map(|d| d.0.clone()),
        run,
        rows,
        n,
    })
}

/// The fits against a node's own measure: (node, measure, points, c, p, q,
/// residual); the nodes without the measure or with fewer than
/// MIN_FIT_POINTS distinct values of it, with the reason; the nodes whose
/// time's residual exceeds FOLLOWS.
#[derive(Clone, Debug, Default)]
pub struct ByMeasure {
    pub rows: Vec<(Path, &'static str, usize, f64, f64, i32, f64)>,
    pub skipped: Vec<(Path, String)>,
    pub poor: Vec<(Path, f64)>,
}

/// Per node the fit of c x^p log^q(x) to its time in its domain, its calls
/// and its other exact and program-added noisy measures over x, the node's
/// own measure `unit`, one point per record that has unit at the node.
pub fn by_measure(runs: &[Run], unit: &str) -> ByMeasure {
    let recs: Vec<Nodes> = runs.iter().map(|r| corrected(r).0).collect();
    let mut other = union(runs, |r| &r.kind("exact") | &r.kind("noisy"));
    other.retain(|m| !["wall", "busy", "calls", unit].contains(m));
    let mut out = ByMeasure::default();
    for p in preorder(recs.iter().flat_map(|n| &n.keys)) {
        let mut vs: Vec<&Values> = recs
            .iter()
            .filter_map(|n| n.get(&p))
            .filter(|v| v.get(unit) > 0.0)
            .collect();
        vs.sort_by(|a, b| a.get(unit).total_cmp(&b.get(unit)));
        let x: Vec<f64> = vs.iter().map(|v| v.get(unit)).collect();
        let mut distinct = x.clone();
        distinct.dedup();
        if distinct.len() < MIN_FIT_POINTS {
            let why = if x.is_empty() {
                "absent".into()
            } else {
                format!("{} values", distinct.len())
            };
            out.skipped.push((p, why));
            continue;
        }
        let time = domain(vs[vs.len() - 1]);
        let mut ms = vec![time, "calls"];
        ms.extend(other.iter().filter(|m| vs.iter().any(|v| v.has(m))));
        for (m, c, e, q, res) in node_fits(&x, &vs, &ms) {
            out.rows.push((p.clone(), m, x.len(), c, e, q, res));
            if m == time && res > FOLLOWS {
                out.poor.push((p.clone(), res));
            }
        }
    }
    out
}

/// Node, t, wall, speedup, efficiency, serial fraction, waiting share.
pub type SpeedRow = (Path, f64, f64, f64, f64, f64, Option<f64>);

/// Speedup per node over a thread count: the smallest count t0, and (node,
/// t, wall, speedup, efficiency, serial fraction, waiting share).
#[derive(Clone, Debug)]
pub struct Threads {
    pub t0: f64,
    pub rows: Vec<SpeedRow>,
}

/// Per node timed on the calling thread and per thread count t above the
/// smallest t0: speedup S = T(t0)/T(t), efficiency S t0/t, the Karp-Flatt
/// serial fraction of S on t/t0, and the waiting share 1 - busy/(t wall)
/// where the node has worker time. T is the corrected wall time.
pub fn threads(runs: &[Run], by: &str) -> Result<Threads> {
    let pts = points(runs, by)?;
    if pts.len() < 2 {
        return Err(format!("one value of {by}; speedup needs two"));
    }
    let (t0, base) = &pts[0];
    let mut rows = Vec::new();
    for p in preorder(&base.keys) {
        let b = &base.get(&p).cloned().unwrap_or_default();
        if domain(b) != "wall" || b.get("wall") <= 0.0 {
            continue;
        }
        for (t, nodes) in &pts[1..] {
            let Some(v) = nodes
                .get(&p)
                .filter(|v| !v.0.is_empty() && v.get("wall") > 0.0)
            else {
                continue;
            };
            let s = b.get("wall") / v.get("wall");
            let wait = (v.get("busy") > 0.0).then(|| 1.0 - v.get("busy") / (t * v.get("wall")));
            rows.push((
                p.clone(),
                *t,
                v.get("wall"),
                s,
                s * t0 / t,
                karp_flatt(s, t / t0),
                wait,
            ));
        }
    }
    Ok(Threads { t0: *t0, rows })
}

fn model(p: f64, q: i32, x: &str) -> String {
    format!(
        "{x}^{p:.2}{}",
        if q != 0 {
            format!(" log {x}")
        } else {
            String::new()
        }
    )
}

/// The fits over a parameter as text.
pub fn text(runs: &[Run], by: &str) -> Result<String> {
    let f = fits(runs, by)?;
    let ns: Vec<String> = f.n.iter().map(|&x| g(x, 6)).collect();
    let mut out = vec![
        format!(
            "{} records over {by} = {}: {} points per fit, c n^p log^q(n) by least squares in log space, residual the rms of ln y.",
            runs.len(),
            ns.join(" "),
            f.n.len()
        ),
        format!("  {:<32} {:<12} {:>10} {:<16} {:>9}", "node", "measure", "c", "model", "residual"),
    ];
    for (p, m, c, e, q, res) in &f.rows {
        out.push(format!(
            "  {:<32} {m:<12} {:>10} {:<16} {res:>9.3}",
            path(p),
            g(*c, 3),
            model(*e, *q, "n")
        ));
    }
    let departs: Vec<String> = f
        .departs
        .iter()
        .map(|(p, e)| format!("{} ({e:.2})", path(p)))
        .collect();
    out.push(format!(
        "dominant: {}, local exponent {:.2} at {by} = {} (largest self time); departing by more than {}: {}",
        path(&f.dominant.unwrap_or_default()),
        f.run.unwrap_or(0.0),
        g(f.n[f.n.len() - 1], 6),
        g(DEPARTURE, 6),
        if departs.is_empty() { "none".into() } else { departs.join(", ") }
    ));
    Ok(out.join("\n"))
}

/// Speedup over a thread count as text.
pub fn text_threads(runs: &[Run], by: &str) -> Result<String> {
    let t = threads(runs, by)?;
    let mut out = vec![
        format!(
            "{} records over {by}, base {}; wall time corrected, median over records.",
            runs.len(),
            g(t.t0, 6)
        ),
        format!(
            "  {:<32} {by:>7} {:>10} {:>8} {:>10} {:>9} {:>6}",
            "node", "wall ms", "speedup", "efficiency", "serial", "wait"
        ),
    ];
    for (p, n, wall, s, e, f, wait) in &t.rows {
        out.push(format!(
            "  {:<32} {:>7} {:>10.3} {s:>8.2} {:>9.1}% {f:>9.3} {:>6}",
            path(p),
            g(*n, 6),
            wall / 1e6,
            100.0 * e,
            wait.map_or(String::new(), |w| format!("{:.1}%", 100.0 * w))
        ));
    }
    Ok(out.join("\n"))
}

/// The fits against a node's own measure as text.
pub fn text_measure(runs: &[Run], unit: &str) -> String {
    let f = by_measure(runs, unit);
    let mut out = vec![
        format!(
            "{} records, per node against its own {unit}: c x^p log^q(x) by least squares in log space over the records that have it, residual the rms of ln y.",
            runs.len()
        ),
        format!(
            "  {:<32} {:<12} {:>6} {:>10} {:<16} {:>9}",
            "node", "measure", "points", "c", "model", "residual"
        ),
    ];
    for (p, m, k, c, e, q, res) in &f.rows {
        out.push(format!(
            "  {:<32} {m:<12} {k:>6} {:>10} {:<16} {res:>9.3}",
            path(p),
            g(*c, 3),
            model(*e, *q, "x")
        ));
    }
    let list = |v: Vec<String>| {
        if v.is_empty() {
            "none".into()
        } else {
            v.join(", ")
        }
    };
    let absent = f
        .skipped
        .iter()
        .filter(|s| s.1 == "absent")
        .map(|s| path(&s.0))
        .collect();
    let few = f
        .skipped
        .iter()
        .filter(|s| s.1 != "absent")
        .map(|s| format!("{} ({})", path(&s.0), s.1))
        .collect();
    out.push(format!(
        "skipped: without {unit} {}; fewer than {MIN_FIT_POINTS} values {}",
        list(absent),
        list(few)
    ));
    let poor = f
        .poor
        .iter()
        .map(|(p, r)| format!("{} ({r:.3})", path(p)))
        .collect();
    out.push(format!(
        "time not following {unit}, residual above {}: {}",
        g(FOLLOWS, 6),
        list(poor)
    ));
    out.join("\n")
}
