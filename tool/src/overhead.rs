//! The scope cost subtracted from recorded times, and the shares of the run.

use crate::records::{children, Nodes, Path, Run, Values};
use crate::stats::sum;

/// A record's corrected nodes and its run totals, see [`corrected`].
pub type Corrected = (Nodes, Values);

/// Whether the worker instances below a node may lie in its own worker
/// instances or in its tasks: it has both. [`corrected`] takes them as run
/// in its tasks.
pub fn mixed(v: &Values) -> bool {
    v.get("worker_calls") > 0.0 && v.get("tasks") > 0.0
}

/// One record's nodes with the scope cost subtracted, and its run totals.
///
/// With `inner` and `full = inner + outer` the record's calibration
/// constants, c(n) = calls - worker_calls the instances of n on threads
/// entered through the context and w(n) = worker_calls those on threads
/// entered through a handle:
///
/// ```text
/// wall'(n) = wall(n) - c(n) inner - C(n) full
/// busy'(n) = busy(n) - w(n) inner - [tasks(n) = 0] W(n) full
/// pool'(n) = pool(n)              - [tasks(n) > 0] W(n) full
/// ```
///
/// where C(n) sums c over the descendants of n (on a context thread every
/// instance below n lies inside one of n), and W(n) sums w over the
/// descendants reached without passing a node with tasks: a worker instance
/// below a node with tasks ran under one of its handle guards, whose spans
/// are that node's pool. A node with tasks and worker instances of its own
/// ([`mixed`]) has the worker instances below it taken as run in its tasks.
/// A recorded zero stays zero (no interval to correct).
///
/// `time` = wall' + busy', the thread time in the node's own instances;
/// `self` = time minus wall' of the children where the node has wall time
/// and minus busy' of the children where it has busy time and no tasks;
/// `cost` = wall + busy - time.
///
/// Returns the nodes with these values and the run's totals: `wall`, the
/// root's wall' (summed over the top-level scopes when the record was taken
/// with the root still open), `pool`, pool' summed over the nodes, `thread`,
/// their sum, `raw`, the recorded thread time, and `cost`, calls times
/// `full` summed over all nodes. All of it is additive over records.
pub fn corrected(run: &Run) -> Corrected {
    let nodes = &run.nodes;
    let (inner, outer) = run.calibration();
    let full = inner + outer;
    let kids = children(nodes);
    let v = |i: usize, m: &str| nodes.vals[i].get(m);
    let ctx = |i: usize| v(i, "calls") - v(i, "worker_calls");
    fn sum_down_to(
        kids: &[Vec<usize>],
        i: usize,
        f: &dyn Fn(usize) -> f64,
        stop: &dyn Fn(usize) -> bool,
    ) -> f64 {
        let deeper = |k: usize| {
            if stop(k) {
                0.0
            } else {
                sum_down_to(kids, k, f, stop)
            }
        };
        sum(kids[i].iter().map(|&k| f(k) + deeper(k)))
    }
    let tasks = |i: usize| v(i, "tasks") > 0.0;
    let fix = |x: f64, d: f64| if x == 0.0 { 0.0 } else { x - d };
    let mut out = nodes.clone();
    for (i, o) in out.vals.iter_mut().enumerate() {
        let c = sum_down_to(&kids, i, &ctx, &|_| false);
        let w = sum_down_to(&kids, i, &|k| v(k, "worker_calls"), &tasks);
        let (bw, bp) = if tasks(i) { (0.0, w) } else { (w, 0.0) };
        let wall = fix(v(i, "wall"), ctx(i) * inner + c * full);
        let busy = fix(v(i, "busy"), v(i, "worker_calls") * inner + bw * full);
        o.set("wall", wall);
        o.set("busy", busy);
        o.set("pool", fix(v(i, "pool"), bp * full));
        o.set("time", wall + busy);
        o.set("cost", v(i, "wall") + v(i, "busy") - wall - busy);
    }
    let self_time: Vec<f64> = (0..out.vals.len())
        .map(|i| {
            let o = |x: &str, inside: bool| match v(i, x) == 0.0 || !inside {
                true => out.vals[i].get(x),
                false => out.vals[i].get(x) - sum(kids[i].iter().map(|&k| out.vals[k].get(x))),
            };
            o("wall", true) + o("busy", !tasks(i))
        })
        .collect();
    for (o, own) in out.vals.iter_mut().zip(self_time) {
        o.set("self", own);
    }
    let totals = |t: &Nodes| {
        let top = t.iter().filter(|(p, _)| p.len() == 1);
        let wall = match nodes.get(&Path::new()).map_or(0.0, |v| v.get("wall")) {
            0.0 => sum(top.map(|(_, v)| v.get("wall"))),
            _ => t.get(&Path::new()).map_or(0.0, |v| v.get("wall")),
        };
        (wall, sum(t.vals.iter().map(|v| v.get("pool"))))
    };
    let (wall, pool) = totals(&out);
    let (rw, rp) = totals(nodes);
    let cost = sum(nodes.vals.iter().map(|v| v.get("calls"))) * full;
    let run = Values(vec![
        ("wall", wall),
        ("pool", pool),
        ("thread", wall + pool),
        ("raw", rw + rp),
        ("cost", cost),
    ]);
    (out, run)
}

/// A node's measure x as a share of the run: `wall` of the run's wall time,
/// any other time (`time`, `busy`, `pool`, a `time!` measure) of the run's
/// thread time.
pub fn share(v: &Values, run: &Values, x: &str) -> f64 {
    let t = run.get(if x == "wall" { "wall" } else { "thread" });
    match t {
        0.0 => 0.0,
        t => v.get(x) / t,
    }
}

/// The estimated scope cost as a share of a node's recorded wall + busy.
pub fn cost_share(v: &Values) -> f64 {
    let raw = v.get("time") + v.get("cost");
    match raw {
        0.0 => 0.0,
        _ => v.get("cost") / raw,
    }
}
