//! The scope cost subtracted from recorded times, and the shares of the run.

use crate::records::{children, Nodes, Path, Run, Values};
use crate::stats::sum;

/// The time measure a node's own calls are timed in: `busy` for a node with
/// busy time and no wall time (its scopes ran only on threads entered
/// through a handle), else `wall`.
pub fn domain(v: &Values) -> &'static str {
    match v.get("wall") == 0.0 && v.get("busy") > 0.0 {
        true => "busy",
        false => "wall",
    }
}

/// A record's corrected nodes and its run totals, see [`corrected`].
pub type Corrected = (Nodes, Values);

/// One record's nodes with the scope cost subtracted, and its run totals.
///
/// With `inner` and `full = inner + outer` the record's calibration
/// constants, for a node n and a time measure X (wall or busy):
///
/// ```text
/// X'(n) = X(n) - calls(n) inner [domain(n) = X] - below_X(n) full
/// ```
///
/// where below_X(n) counts the scope instances under n that are timed in X
/// and reached through nodes timed in X: the instances inside n's X
/// intervals. A node without time in X has no X interval: X'(n) = 0 (a scope
/// on the calling thread whose children ran on workers has no busy time).
/// The self time in X is X'(n) minus X' of the children timed in X, zero
/// without time in X. In the node's domain D, `cost` = D(n) - D'(n) is the
/// estimated instrumentation and `self` the self time in D.
///
/// Returns the nodes, the record's values with `wall` and `busy` corrected
/// plus `cost` and `self`, and the run's totals: `wall`, the run's wall time
/// (the root's; summed over the top-level scopes when the record was taken
/// with the root still open), `busy`, the worker time (busy of the nodes in
/// the wall domain, the spans of the parallel sections), both corrected,
/// `raw`, their recorded sum, and `cost`, calls times `full` summed over all
/// nodes. All of it is additive over records.
pub fn corrected(run: &Run) -> Corrected {
    let nodes = &run.nodes;
    let (inner, outer) = run.calibration();
    let full = inner + outer;
    let kids = children(nodes);
    let d: Vec<&str> = nodes.vals.iter().map(domain).collect();
    fn below(kids: &[Vec<usize>], d: &[&str], nodes: &Nodes, i: usize, x: &str) -> f64 {
        let k = kids[i].iter().filter(|&&k| d[k] == x);
        sum(k.map(|&k| nodes.vals[k].get("calls") + below(kids, d, nodes, k, x)))
    }
    let mut out = nodes.clone();
    for (i, o) in out.vals.iter_mut().enumerate() {
        let v = &nodes.vals[i];
        for x in ["wall", "busy"] {
            let own = if d[i] == x {
                v.get("calls") * inner
            } else {
                0.0
            };
            let c = match v.get(x) {
                0.0 => 0.0,
                t => t - own - below(&kids, &d, nodes, i, x) * full,
            };
            o.set(x, c);
        }
        o.set("cost", v.get(d[i]) - o.get(d[i]));
    }
    for i in 0..out.vals.len() {
        let x = d[i];
        let own = match nodes.vals[i].get(x) {
            0.0 => 0.0,
            _ => {
                let k = kids[i].iter().filter(|&&k| d[k] == x);
                out.vals[i].get(x) - sum(k.map(|&k| out.vals[k].get(x)))
            }
        };
        out.vals[i].set("self", own);
    }
    let totals = |t: &Nodes| {
        let root = |t: &Nodes| t.get(&Path::new()).map_or(0.0, |v| v.get("wall"));
        let top = t.iter().filter(|(p, _)| p.len() == 1);
        let wall = match root(nodes) {
            0.0 => sum(top.map(|(_, v)| v.get("wall"))),
            _ => root(t),
        };
        let busy = t.vals.iter().enumerate().filter(|(i, _)| d[*i] == "wall");
        (wall, sum(busy.map(|(_, v)| v.get("busy"))))
    };
    let (wall, busy) = totals(&out);
    let (rw, rb) = totals(nodes);
    let cost = sum(nodes.vals.iter().map(|v| v.get("calls"))) * full;
    let run = Values(vec![
        ("wall", wall),
        ("busy", busy),
        ("raw", rw + rb),
        ("cost", cost),
    ]);
    (out, run)
}

/// A node's time in measure x (default its domain) as a share of the run's
/// time in that measure: of the run's wall time for nodes timed on the
/// calling thread, of the worker time for nodes timed on workers.
pub fn share(v: &Values, run: &Values, x: Option<&str>) -> f64 {
    let x = x.unwrap_or(domain(v));
    match run.get(x) {
        0.0 => 0.0,
        t => v.get(x) / t,
    }
}

/// The estimated scope cost as a share of a node's recorded time in its
/// domain.
pub fn cost_share(v: &Values) -> f64 {
    let raw = v.get(domain(v)) + v.get("cost");
    match raw {
        0.0 => 0.0,
        _ => v.get("cost") / raw,
    }
}
