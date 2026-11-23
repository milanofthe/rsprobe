"""`scale`: records over a run parameter, a problem size or a thread count."""

from .constants import DEPARTURE, MIN_FIT_POINTS
from .overhead import corrected, domain
from .records import name, preorder
from .stats import karp_flatt, local_exponent, median, power_fit


def points(runs, by):
    """The parameter's values in ascending order and per value the nodes
    with the median of each measure over the records of that value."""
    groups = {}
    for r in runs:
        x = r["params"].get(by)
        if not isinstance(x, (int, float)):
            raise ValueError("%s: no numeric parameter %r" % (r["file"], by))
        groups.setdefault(x, []).append(corrected(r)[0])
    out = []
    for x in sorted(groups):
        recs = groups[x]
        paths = dict.fromkeys(p for nodes in recs for p in nodes)
        out.append((x, {p: {m: median([n.get(p, {}).get(m, 0) for n in recs])
                            for m in {m for n in recs for m in n.get(p, {})}}
                        for p in paths}))
    return out


def fits(runs, by):
    """Per node and measure the fit of c n^p log^q(n) (stats.power_fit) over
    the parameter n: the node's time in its domain, calls and the exact
    measures. Nodes absent or zero at a point are not fitted. Returns a dict:
    `n`, `rows` (path, measure, c, p, q, residual), `run` (the dominant
    exponent: the local exponent at the largest n of the node with the
    largest self time there), `dominant` (that node) and `departs` (the
    other scopes whose time's local exponent differs from it by more than
    DEPARTURE, with theirs)."""
    pts = points(runs, by)
    n = [x for x, _ in pts]
    if len(n) < MIN_FIT_POINTS:
        raise ValueError("%d values of %s; a fit needs at least %d"
                         % (len(n), by, MIN_FIT_POINTS))
    exact = set().union(*(r["exact"] for r in runs)) - {"calls"}
    rows, local = [], {}
    for p in preorder([p for _, nodes in pts for p in nodes]):
        vs = [nodes.get(p) for _, nodes in pts]
        if None in vs:
            continue
        time = domain(vs[-1])
        for m in [time, "calls"] + sorted(exact & set(vs[-1])):
            y = [v.get(m, 0) for v in vs]
            if min(y) <= 0:
                continue
            c, e, q, res = power_fit(n, y)
            rows.append((p, m, c, e, q, res))
            if m == time:
                local[p] = (local_exponent(e, q, n[-1]), vs[-1]["self"])
    dominant = max(local, key=lambda p: local[p][1], default=None)
    run = local[dominant][0] if local else None
    departs = [(p, x) for p, (x, _) in local.items()
               if p and abs(x - run) > DEPARTURE]
    return {"n": n, "rows": rows, "run": run, "dominant": dominant,
            "departs": departs}


def threads(runs, by):
    """Per node timed on the calling thread and per thread count t above the
    smallest t0: speedup S = T(t0)/T(t), efficiency S t0/t, the Karp-Flatt
    serial fraction of S on t/t0, and the waiting share 1 - busy/(t wall)
    where the node has worker time. T is the corrected wall time."""
    pts = points(runs, by)
    if len(pts) < 2:
        raise ValueError("one value of %s; speedup needs two" % by)
    t0, base = pts[0]
    rows = []
    for p in preorder(list(base)):
        if domain(base[p]) != "wall" or base[p]["wall"] <= 0:
            continue
        for t, nodes in pts[1:]:
            v = nodes.get(p)
            if not v or v["wall"] <= 0:
                continue
            s = base[p]["wall"] / v["wall"]
            wait = 1 - v["busy"] / (t * v["wall"]) if v["busy"] > 0 else None
            rows.append((p, t, v["wall"], s, s * t0 / t,
                         karp_flatt(s, t / t0), wait))
    return {"t0": t0, "rows": rows}


def model(p, q):
    return "n^%.2f%s" % (p, " log n" if q else "")


def text(runs, by):
    f = fits(runs, by)
    out = ["%s over %s = %s: %d points per fit, c n^p log^q(n) by least "
           "squares in log space, residual the rms of ln y." % (
               "%d records" % len(runs), by,
               " ".join("%g" % x for x in f["n"]), len(f["n"])),
           "  %-32s %-12s %10s %-16s %9s" % (
               "node", "measure", "c", "model", "residual")]
    for p, m, c, e, q, res in f["rows"]:
        out.append("  %-32s %-12s %10.3g %-16s %9.3f" % (
            name(p), m, c, model(e, q), res))
    out.append("dominant: %s, local exponent %.2f at %s = %g (largest self "
               "time); departing by more than %g: %s" % (
                   name(f["dominant"] or ()), f["run"] or 0, by, f["n"][-1],
                   DEPARTURE,
                   ", ".join("%s (%.2f)" % (name(p), e)
                             for p, e in f["departs"]) or "none"))
    return "\n".join(out)


def text_threads(runs, by):
    t = threads(runs, by)
    out = ["%d records over %s, base %g; wall time corrected, median over "
           "records." % (len(runs), by, t["t0"]),
           "  %-32s %7s %10s %8s %10s %9s %6s" % (
               "node", by, "wall ms", "speedup", "efficiency",
               "serial", "wait")]
    for p, n, wall, s, e, f, wait in t["rows"]:
        out.append("  %-32s %7g %10.3f %8.2f %9.1f%% %9.3f %6s" % (
            name(p), n, wall / 1e6, s, 100 * e, f,
            "%.1f%%" % (100 * wait) if wait is not None else ""))
    return "\n".join(out)
