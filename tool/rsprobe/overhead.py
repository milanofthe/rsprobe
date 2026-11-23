"""The scope cost subtracted from recorded times, and the shares of the run."""

from .records import children


def domain(v):
    """The time measure a node's own calls are timed in: `busy` for a node
    with busy time and no wall time (its scopes ran only on threads entered
    through a handle), else `wall`."""
    return "busy" if v.get("wall", 0) == 0 and v.get("busy", 0) > 0 else "wall"


def corrected(run):
    """One record's nodes with the scope cost subtracted, and its run totals.

    With `inner` and `full = inner + outer` the record's calibration
    constants, for a node n and a time measure X (wall or busy):

        X'(n) = X(n) - calls(n) inner [domain(n) = X] - below_X(n) full

    where below_X(n) counts the scope instances under n that are timed in X
    and reached through nodes timed in X: the instances inside n's X
    intervals. A node without time in X has no X interval: X'(n) = 0 (a scope
    on the calling thread whose children ran on workers has no busy time).
    The self time in X is X'(n) minus X' of the children timed in X, zero
    without time in X. In the node's domain D, `cost` = D(n) - D'(n) is the
    estimated instrumentation and `self` the self time in D.

    Returns ({path: values}, run): values are the record's with `wall` and
    `busy` corrected, plus `self` and `cost`; run holds `wall`, the run's wall
    time (the root's; summed over the top-level scopes when the record was
    taken with the root still open), `busy`, the worker time (busy of the
    nodes in the wall domain, the spans of the parallel sections), both
    corrected, `raw`, their recorded sum, and `cost`, calls times `full`
    summed over all nodes. All of it is additive over records."""
    nodes = run["nodes"]
    inner, full = run["inner"], run["inner"] + run["outer"]
    kids = children(list(nodes))

    def below(p, x):
        return sum(nodes[k].get("calls", 0) + below(k, x)
                   for k in kids[p] if domain(nodes[k]) == x)

    out = {}
    for p, v in nodes.items():
        d = domain(v)
        o = dict(v)
        for x in ("wall", "busy"):
            own = v.get("calls", 0) * inner if d == x else 0
            o[x] = v.get(x, 0) and v[x] - own - below(p, x) * full
        o["cost"] = v.get(d, 0) - o[d]
        out[p] = o

    def own(p, x):
        return nodes[p].get(x, 0) and out[p][x] - sum(
            out[k][x] for k in kids[p] if domain(nodes[k]) == x)

    for p, o in out.items():
        o["self"] = own(p, domain(nodes[p]))

    def totals(t):
        top = [v for p, v in t.items() if len(p) == 1]
        wall = t[()].get("wall", 0) if nodes[()].get("wall", 0) else \
            sum(v.get("wall", 0) for v in top)
        busy = sum(v.get("busy", 0) for p, v in t.items()
                   if domain(nodes[p]) == "wall")
        return wall, busy

    wall, busy = totals(out)
    raw = sum(totals(nodes))
    cost = sum(v.get("calls", 0) for v in nodes.values()) * full
    return out, {"wall": wall, "busy": busy, "raw": raw, "cost": cost}


def share(v, run, x=None):
    """A node's time in measure x (default its domain) as a share of the
    run's time in that measure: of the run's wall time for nodes timed on the
    calling thread, of the worker time for nodes timed on workers."""
    x = x or domain(v)
    return v.get(x, 0) / run[x] if run[x] else 0.0


def cost_share(v):
    """The estimated scope cost as a share of a node's recorded time in its
    domain."""
    raw = v.get(domain(v), 0) + v["cost"]
    return v["cost"] / raw if raw else 0.0
