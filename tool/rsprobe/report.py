"""`report`: the tree of one record, the medians over several or their sum,
and each thread's values."""

import json

from .constants import OVERHEAD_MARK
from .overhead import corrected, cost_share, domain, share
from .records import children, name, preorder, total
from .stats import median

# Values of a node that are not user measures.
BUILTIN = ("calls", "wall", "busy", "self", "cost")


def summary(runs):
    """The corrected nodes and run totals summed over the records, `max`
    measures by maximum."""
    parts = [corrected(r) for r in runs]
    nodes = total((p[0] for p in parts), set().union(*(r["max"] for r in runs)))
    run = total({(): p[1]} for p in parts)[()]
    return nodes, run


def medians(runs):
    """The corrected nodes and run totals over the records: per node and
    measure the median (zero where a record lacks the node), the maximum for
    `max` measures; and per node {measure: (min, max)} for the others where
    the records differ."""
    parts = [corrected(r) for r in runs]
    maxima = set().union(*(r["max"] for r in runs))
    nodes, spread = {}, {}
    for p in preorder([p for ns, _ in parts for p in ns]):
        nodes[p], spread[p] = {}, {}
        for m in dict.fromkeys(m for ns, _ in parts for m in ns.get(p, {})):
            xs = [ns.get(p, {}).get(m, 0) for ns, _ in parts]
            nodes[p][m] = max(xs) if m in maxima else median(xs)
            if m not in maxima and min(xs) < max(xs):
                spread[p][m] = (min(xs), max(xs))
    run = {k: median([r[k] for _, r in parts]) for k in parts[0][1]}
    return nodes, run, spread


def ratios(v, noisy, own, asked=(), maxima=()):
    """Time per unit of each exact user measure (ns, in the node's domain)
    where the node has time of its own (`own`), each noisy one (a time) over
    that time, busy over wall where the node has both, and each asked (A, B)
    as `A/B` where B is not zero. A `max` measure is a level: no ratio unless
    asked."""
    d = domain(v)
    t = v[d]
    out = {}
    for m, x in v.items():
        if m in BUILTIN or m in maxima or not x:
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


def rows(runs, asked=(), summed=True):
    """One dict per node in preorder, with shares, the ratios and those asked
    for as (A, B) pairs: of the values summed over the records, or with
    `summed` false of their medians, with `spread` (see `medians`)."""
    nodes, run, spread = summary(runs) + ({},) if summed else medians(runs)
    noisy = set().union(*(r["noisy"] for r in runs))
    maxima = set().union(*(r["max"] for r in runs))
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
                   cost_share=cost_share(v),
                   ratios=ratios(v, noisy, own, asked, maxima),
                   measures={m: x for m, x in v.items() if m not in BUILTIN},
                   spread=spread.get(p, {}))
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


def text(runs, asked=(), summed=False):
    rs, run = rows(runs, asked, summed)
    k = len(runs)
    merged = (", summed" if summed else ", median over records") * (k > 1)
    inner = sum(r["inner"] for r in runs) / k
    outer = sum(r["outer"] for r in runs) / k
    ps = " ".join("%s=%s" % (n, ",".join(map(str, xs)))
                  for n, xs in params(runs).items())
    out = [
        "%d record%s%s. %s" % (k, "s" * (k > 1), merged, ps),
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
    noisy = set().union(*(r["noisy"] for r in runs)) | {"wall", "busy"}
    maxima = set().union(*(r["max"] for r in runs))

    def measure(r, m, x):
        lo, hi = r["spread"].get(m, (None, None))
        if m in maxima:
            return "%s max %d" % (m, x)
        if m in noisy:
            ms = "%s %.3f ms" % (m, x / 1e6) if m in r["measures"] else m
            return ms + (" [%.3f..%.3f]" % (lo / 1e6, hi / 1e6) if hi else "")
        return "%s=%d..%d differs" % (m, lo, hi) if hi else "%s=%d" % (m, x)
    for r in rs:
        spread = [m for m in ("calls", r["domain"]) if m in r["spread"]]
        user = [measure(r, m, r.get(m, 0)) for m in spread]
        user += [measure(r, m, x) for m, x in r["measures"].items()]
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
    if k > 1 and not summed:
        out.append("Medians over the records, [min..max] where they differ; "
                   "an exact measure that differs is shown as its range; "
                   "a max measure is the largest value. --sum: the sums.")
    ws, dropped = warnings(runs)
    if ws or dropped:
        out += ["", "warnings%s:" % (", summed" * (k > 1)),
                "%8s  %-28s %-6s %s" % ("count", "node", "level", "text")]
        out += ["%8d  %-28s %-6s %s" % (c, name(p), lv, t)
                for p, lv, t, c in ws]
        if dropped:
            out.append("%8d  not kept, beyond the record's bound" % dropped)
    return "\n".join(out)


def as_json(runs, asked=(), summed=False):
    rs, run = rows(runs, asked, summed)
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


def threads(runs, node=None):
    """Per record, node (the one named by its path `a/b`, or all) and thread
    the recorded values of each thread, with its busy time over the mean of
    the threads with busy time at that node."""
    out = []
    for r in runs:
        if not r["per_thread"]:
            raise ValueError("%s: no per-thread values; take the record with "
                             "Context::record_threads()" % r["file"])
        ts = r["per_thread"]
        paths = preorder([p for t in ts for p in t["nodes"]])
        paths = [p for p in paths if node is None or name(p) == node]
        if not paths:
            raise ValueError("%s: no node %s" % (r["file"], node))
        out += ["%s: %d threads, recorded values (scope cost not subtracted)"
                % (r["file"], len(ts)),
                "%-28s %6s %-7s %8s %10s %10s %9s  %s" % (
                    "node", "thread", "via", "calls", "wall ms", "busy ms",
                    "busy/mean", "measures")]
        for p in paths:
            vs = [(t, t["nodes"][p]) for t in ts if p in t["nodes"]]
            busy = [v["busy"] for _, v in vs if v.get("busy", 0) > 0]
            mean = sum(busy) / len(busy) if busy else 0
            label = "  " * len(p) + (p[-1] if p else "(run)")
            for t, v in vs:
                user = ["%s max %d" % (m, x) if m in r["max"] else
                        "%s %.3f ms" % (m, x / 1e6) if m in r["noisy"] else
                        "%s=%d" % (m, x) for m, x in v.items()
                        if m not in BUILTIN and x]
                out.append(("%-28s %6d %-7s %8d %10.3f %10.3f %9s  %s" % (
                    label, t["index"], "handle" if t["handle"] else "context",
                    v.get("calls", 0), v.get("wall", 0) / 1e6,
                    v.get("busy", 0) / 1e6,
                    "%.3f" % (v["busy"] / mean) if v.get("busy") else "",
                    ", ".join(user))).rstrip())
                label = ""
        out.append("")
    return "\n".join(out[:-1])
