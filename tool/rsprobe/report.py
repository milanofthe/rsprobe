"""`report`: the tree of one record or the sum of several."""

import json

from .constants import OVERHEAD_MARK
from .overhead import corrected, cost_share, domain, share
from .records import children, name, preorder, total

# Values of a node that are not user measures.
BUILTIN = ("calls", "wall", "busy", "self", "cost")


def summary(runs):
    """The corrected nodes and run totals summed over the records, `max`
    measures by maximum."""
    parts = [corrected(r) for r in runs]
    nodes = total((p[0] for p in parts), set().union(*(r["max"] for r in runs)))
    run = total({(): p[1]} for p in parts)[()]
    return nodes, run


def ratios(v, noisy, own, asked=()):
    """Time per unit of each exact user measure (ns, in the node's domain)
    where the node has time of its own (`own`), each noisy one (a time) over
    that time, busy over wall where the node has both, and each asked (A, B)
    as `A/B` where B is not zero."""
    d = domain(v)
    t = v[d]
    out = {}
    for m, x in v.items():
        if m in BUILTIN or not x:
            continue
        if m not in noisy:
            if own:
                out["ns/" + m] = t / x
        elif t:
            out["%s/%s" % (m, d)] = x / t
    if v["wall"] > 0 and v["busy"] > 0:
        out["busy/wall"] = v["busy"] / v["wall"]
    for a, b in asked:
        if v.get(b):
            out["%s/%s" % (a, b)] = v.get(a, 0) / v[b]
    return out


def asked_ratio(text):
    """`A/B` as the pair (A, B) of measure names."""
    a, _, b = text.rpartition("/")
    if not a or not b:
        raise ValueError("expected --ratio A/B, got %r" % text)
    return a, b


def rows(runs, asked=()):
    """One dict per node in preorder, with shares, the ratios and those asked
    for as (A, B) pairs."""
    nodes, run = summary(runs)
    noisy = set().union(*(r["noisy"] for r in runs))
    kids = children(list(nodes))
    out = []
    for p in preorder(nodes):
        v = nodes[p]
        # Time of its own: a scope with self time whose children ran in its
        # domain, so that no part of it is waiting for work timed elsewhere.
        own = v["calls"] and v["self"] > 0 and all(
            domain(nodes[k]) == domain(v) for k in kids[p])
        row = {m: v[m] for m in BUILTIN}
        row.update(path=list(p), domain=domain(v), share=share(v, run),
                   cost_share=cost_share(v), ratios=ratios(v, noisy, own, asked),
                   measures={m: x for m, x in v.items() if m not in BUILTIN})
        out.append(row)
    return out, run


def params(runs):
    """Each parameter with its distinct values over the records."""
    out = {}
    for r in runs:
        for k, x in r["params"].items():
            out.setdefault(k, [])
            if x not in out[k]:
                out[k].append(x)
    return out


def warnings(runs):
    """The records' warnings summed per node, level and text, in order of
    first appearance: ([(path, level, text, count)], dropped)."""
    out = {}
    for r in runs:
        for path, level, t, count in r["warnings"]:
            out[path, level, t] = out.get((path, level, t), 0) + count
    dropped = sum(r["warnings_dropped"] for r in runs)
    return [k + (c,) for k, c in out.items()], dropped


def text(runs, asked=()):
    rs, run = rows(runs, asked)
    k = len(runs)
    inner = sum(r["inner"] for r in runs) / k
    outer = sum(r["outer"] for r in runs) / k
    ps = " ".join("%s=%s" % (n, ",".join(map(str, xs)))
                  for n, xs in params(runs).items())
    out = [
        "%d record%s%s. %s" % (k, "s" * (k > 1), ", summed" * (k > 1), ps),
        "calibration: inner %.1f ns, outer %.1f ns (mean over records)"
        % (inner, outer),
        "run: wall %.3f ms, busy %.3f ms, scope cost %.2f%% of the thread time"
        % (run["wall"] / 1e6, run["busy"] / 1e6,
           100 * run["cost"] / run["raw"] if run["raw"] else 0),
        "",
        "%-28s %8s %10s %10s %10s %7s %6s  %s" % (
            "node", "calls", "wall ms", "busy ms", "self ms", "share",
            "scope", "measures"),
    ]
    noisy = set().union(*(r["noisy"] for r in runs))
    for r in rs:
        user = ["%s %.3f ms" % (m, x / 1e6) if m in noisy else "%s=%d" % (m, x)
                for m, x in r["measures"].items()]
        user += ["%s %.4g" % kv for kv in r["ratios"].items()]
        mark = "!" if r["cost_share"] > OVERHEAD_MARK else " "
        label = "  " * len(r["path"]) + (r["path"][-1] if r["path"] else "(run)")
        out.append("%-28s %8d %10.3f %10.3f %10.3f %6.1f%% %5.1f%%%s %s" % (
            label, r["calls"], r["wall"] / 1e6, r["busy"] / 1e6,
            r["self"] / 1e6, 100 * r["share"], 100 * r["cost_share"], mark,
            ", ".join(user)))
    out += [
        "",
        "Times with the scope cost subtracted: X - calls inner - (scope "
        "instances below) (inner + outer).",
        "self and share in the node's own measure: wall, or busy for nodes "
        "timed only on workers (share of the run's busy).",
        "scope: estimated scope cost in the node's recorded time; ! above "
        "%d%%." % (100 * OVERHEAD_MARK),
        "ns/X: time per unit of an exact measure at scopes whose children "
        "ran in their domain; others with --ratio A/B.",
    ]
    ws, dropped = warnings(runs)
    if ws or dropped:
        out += ["", "warnings%s:" % (", summed" * (k > 1)),
                "%8s  %-28s %-6s %s" % ("count", "node", "level", "text")]
        out += ["%8d  %-28s %-6s %s" % (c, name(p), lv, t)
                for p, lv, t, c in ws]
        if dropped:
            out.append("%8d  not kept, beyond the record's bound" % dropped)
    return "\n".join(out)


def as_json(runs, asked=()):
    rs, run = rows(runs, asked)
    ws, dropped = warnings(runs)
    ws = [{"path": list(p), "level": lv, "text": t, "count": c}
          for p, lv, t, c in ws]
    return json.dumps({"records": len(runs), "params": params(runs),
                       "run": run, "nodes": rs, "warnings": ws,
                       "warnings_dropped": dropped}, indent=1)


def folded(runs):
    """Folded stacks, one line per node with self time in ns: the flame
    graph's widths are thread time (wall on the calling thread, busy on
    workers)."""
    nodes, _ = summary(runs)
    return "\n".join("%s %d" % (";".join(("run",) + p), round(nodes[p]["self"]))
                     for p in preorder(nodes) if round(nodes[p]["self"]) > 0)
