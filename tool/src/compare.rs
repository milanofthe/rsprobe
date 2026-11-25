//! `compare`: two sets of records, node by node and measure by measure.

use crate::constants::{CONFIDENCE, GATE_SHARE, MIN_EFFECT, MIN_ROUNDS};
use crate::fmt::{apart, g, interval, path, ratio};
use crate::overhead::{corrected, share, Corrected};
use crate::records::{preorder, union, Nodes, Path, Run};
use crate::stats::{median, ratio_ci};
use rsprobe::units::{self, count, time};

/// A noisy measure at a node: medians in A and B, B/A with its bootstrap
/// interval (none with too few rounds), the verdict and the node's median
/// share of the run in A.
#[derive(Clone, Debug)]
pub struct Noisy {
    pub path: Path,
    pub measure: &'static str,
    pub a: f64,
    pub b: f64,
    pub ratio: Option<(f64, f64, f64)>,
    pub verdict: &'static str,
    pub share: f64,
}

/// The comparison of two sets: exact measures that differ between the sets
/// (node, measure, A, B) and within a set (node, measure, set, values),
/// nodes in one set only, the noisy measures, the nodes and measures failing
/// the gate, the measures of the gate, the records per set and the source
/// of the noise band.
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub exact: Vec<(Path, &'static str, f64, f64)>,
    pub nondeterministic: Vec<(Path, &'static str, &'static str, Vec<f64>)>,
    pub structure: Vec<(Path, &'static str)>,
    pub noisy: Vec<Noisy>,
    pub gate: Vec<(Path, &'static str)>,
    pub gated: Vec<String>,
    pub rounds: (usize, usize),
    pub band: String,
}

/// The value of measure x at a node in each record of a set, zero where the
/// node is absent.
fn series(set: &[Corrected], p: &Path, x: &str) -> Vec<f64> {
    set.iter()
        .map(|n| n.0.get(p).map_or(0.0, |v| v.get(x)))
        .collect()
}

/// Half-width of the noise band in log ratio at a node and measure: the
/// farther bound of the A/A interval when an A/A set is given, never below
/// MIN_EFFECT.
fn band(aa: Option<(&[Corrected], &[Corrected])>, p: &Path, x: &str) -> f64 {
    let floor = (1.0 + MIN_EFFECT).ln();
    let Some((a, a2)) = aa else { return floor };
    let (a, a2) = (series(a, p, x), series(a2, p, x));
    let m = median(&a);
    if a.len() < MIN_ROUNDS || a2.len() < MIN_ROUNDS || m <= 0.0 || m.is_nan() {
        return floor;
    }
    let (_, lo, hi) = ratio_ci(&a, &a2);
    let lo = if lo > 0.0 {
        lo.ln().abs()
    } else {
        f64::INFINITY
    };
    floor.max(lo).max(hi.ln().abs())
}

/// The comparison of record sets a and b, with aa a second set of a's build
/// for the noise band. Noisy measures added by the program (`time!`) are
/// compared as wall, busy and pool are; a slower verdict fails the gate only
/// for the measures in `gate`. The share of a node is its `wall` over the
/// run's wall time, any other measure over the run's thread time.
pub fn compare(a: &[Run], b: &[Run], aa: Option<&[Run]>, gate: &[&str]) -> Comparison {
    let (ca, cb): (Vec<_>, Vec<_>) = (
        a.iter().map(corrected).collect(),
        b.iter().map(corrected).collect(),
    );
    let caa: Option<Vec<_>> = aa.map(|aa| aa.iter().map(corrected).collect());
    let exact = union(a.iter().chain(b), |r| r.kind("exact"));
    let mut noisy = union(a.iter().chain(b), |r| r.kind("noisy"));
    let timed = ["wall", "busy", "pool"];
    noisy.retain(|m| !timed.contains(m));
    let all: Vec<&Corrected> = ca.iter().chain(&cb).collect();
    let mut out = Comparison {
        gated: gate
            .iter()
            .enumerate()
            .filter(|(i, m)| !gate[..*i].contains(m))
            .map(|m| m.1.to_string())
            .collect(),
        rounds: (a.len(), b.len()),
        band: aa.map_or("bootstrap alone".into(), |aa| {
            format!("A/A, {} records", aa.len())
        }),
        ..Comparison::default()
    };
    for p in preorder(all.iter().flat_map(|n| &n.0.keys)) {
        let (in_a, in_b) = (
            ca.iter().any(|n| n.0.get(&p).is_some()),
            cb.iter().any(|n| n.0.get(&p).is_some()),
        );
        if in_a != in_b {
            out.structure.push((p, if in_a { "A" } else { "B" }));
            continue;
        }
        let mut here: Vec<&'static str> = all
            .iter()
            .filter_map(|n| n.0.get(&p))
            .flat_map(|v| v.0.iter().map(|m| m.0))
            .collect();
        here.sort();
        here.dedup();
        for &m in here.iter().filter(|m| exact.contains(*m)) {
            let (va, vb) = (series(&ca, &p, m), series(&cb, &p, m));
            for (side, vs) in [("A", &va), ("B", &vb)] {
                let mut d = vs.clone();
                d.sort_by(|x, y| x.total_cmp(y));
                d.dedup();
                if d.len() > 1 {
                    out.nondeterministic.push((p.clone(), m, side, d));
                }
            }
            let one = |v: &[f64]| v.iter().all(|x| *x == v[0]);
            if one(&va) && one(&vb) && va[0] != vb[0] {
                out.exact.push((p.clone(), m, va[0], vb[0]));
            }
        }
        let xs = timed
            .into_iter()
            .chain(here.iter().copied().filter(|m| noisy.contains(m)));
        for x in xs {
            let (va, vb) = (series(&ca, &p, x), series(&cb, &p, x));
            let (ma, mb) = (median(&va), median(&vb));
            if !(ma > 0.0 && mb > 0.0) {
                continue;
            }
            let at = |n: &Nodes| n.get(&p).cloned().unwrap_or_default();
            let share = median(
                &ca.iter()
                    .map(|(n, run)| share(&at(n), run, x))
                    .collect::<Vec<_>>(),
            );
            let (path, measure, ratio, verdict) = (p.clone(), x, None, "too few rounds");
            let mut row = Noisy {
                path,
                measure,
                a: ma,
                b: mb,
                ratio,
                verdict,
                share,
            };
            if va.len().min(vb.len()) >= MIN_ROUNDS {
                let (r, lo, hi) = ratio_ci(&va, &vb);
                let w = band(caa.as_deref().map(|aa| (&ca[..], aa)), &p, x);
                row.verdict = match () {
                    _ if lo > w.exp() => "slower",
                    _ if hi < (-w).exp() => "faster",
                    _ => "within noise",
                };
                row.ratio = Some((r, lo, hi));
                if row.verdict == "slower" && row.share >= GATE_SHARE && gate.contains(&x) {
                    out.gate.push((p.clone(), x));
                }
            }
            out.noisy.push(row);
        }
    }
    out
}

fn gate(c: &Comparison) -> String {
    match c.gate.is_empty() {
        true => "passes".into(),
        false => {
            let g: Vec<String> = c
                .gate
                .iter()
                .map(|(p, x)| format!("{} {x}", path(p)))
                .collect();
            format!("fails on {}", g.join(", "))
        }
    }
}

/// The comparison as text.
pub fn text(c: &Comparison) -> String {
    let mut out = vec![
        format!(
            "A {} records, B {} records; noise band from the {}, at least {}%.",
            c.rounds.0,
            c.rounds.1,
            c.band,
            g(100.0 * MIN_EFFECT, 6)
        ),
        "exact measures, A -> B:".into(),
    ];
    for (p, m, x, y) in &c.exact {
        let xy = apart(&[*x, *y], |v| count(v).to_string());
        out.push(format!("  {:<32} {m:<12} {} -> {}", path(p), xy[0], xy[1]));
    }
    if c.exact.is_empty() {
        out.push("  no difference".into());
    }
    for (p, m, side, vs) in &c.nondeterministic {
        let vs = apart(vs, |v| count(v).to_string());
        out.push(format!(
            "  {:<32} {m:<12} differs within {side}: {}",
            path(p),
            vs.join(" ")
        ));
    }
    if !c.structure.is_empty() {
        out.push("structure:".into());
        out.extend(
            c.structure
                .iter()
                .map(|(p, s)| format!("  {:<32} only in {s}", path(p))),
        );
    }
    out.push(format!(
        "noisy measures, median over records, B/A with its {}% bootstrap interval:",
        g(100.0 * CONFIDENCE, 6)
    ));
    out.push(format!(
        "  {:<32} {:<9} {:>10} {:>10} {:>7} {:>17} {:>7}  verdict",
        "node", "", "A", "B", "B/A", "interval", "share"
    ));
    for n in &c.noisy {
        out.push(format!(
            "  {:<32} {:<9} {:>10} {:>10} {:>7} {:>17} {:>7}  {}",
            path(&n.path),
            n.measure,
            time(n.a),
            time(n.b),
            ratio(n.ratio.map(|r| r.0)),
            interval(n.ratio.map(|r| (r.1, r.2))),
            units::share(n.share),
            n.verdict
        ));
    }
    out.push(format!("gate: {}", gate(c)));
    out.push(rule(c));
    out.join("\n")
}

/// The gate's rule as a footer line.
fn rule(c: &Comparison) -> String {
    format!(
        "gate rule: fails where a node of at least {}% of the run is slower beyond the band in {}; other measures get a verdict only, --gate MEASURE adds one",
        g(100.0 * GATE_SHARE, 6),
        c.gated.join(", ")
    )
}

/// One line per (name, comparison): the run's wall time (the root's) with
/// its verdict, the number of exact differences and the gate.
pub fn table(cases: &[(String, Comparison)]) -> String {
    let mut out = vec![
        format!(
            "run wall time, median over records, B/A with its {}% bootstrap interval:",
            g(100.0 * CONFIDENCE, 6)
        ),
        format!(
            "{:<16} {:>10} {:>10} {:>7} {:>17} {:<13} {:>5}  gate",
            "case", "A", "B", "B/A", "interval", "verdict", "exact"
        ),
    ];
    for (n, c) in cases {
        let run = c
            .noisy
            .iter()
            .find(|r| r.path.is_empty() && r.measure == "wall");
        let (a, b, r, verdict) = run.map_or((0.0, 0.0, None, "no run time"), |r| {
            (r.a, r.b, r.ratio, r.verdict)
        });
        out.push(format!(
            "{n:<16} {:>10} {:>10} {:>7} {:>17} {verdict:<13} {:>5}  {}",
            time(a),
            time(b),
            ratio(r.map(|r| r.0)),
            interval(r.map(|r| (r.1, r.2))),
            c.exact.len() + c.structure.len(),
            gate(c)
        ));
    }
    if let Some((_, c)) = cases.first() {
        out.push(rule(c));
    }
    out.join("\n")
}
