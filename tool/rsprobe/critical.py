"""`critical`: the chain of spans that bounds a scope instance's elapsed
time, from the trace a context built with `.stream()` writes."""

import bisect
import json

from .constants import CRITICAL_DEPTH
from .records import name


def load(path):
    """The trace's spans ("X" events) as dicts of `node`, `parent` (node ids,
    parent None at the root), `name`, `tid`, `start` and `end` (ns)."""
    with open(path) as f:
        doc = json.load(f)
    spans = []
    for e in doc["traceEvents"]:
        if e.get("ph") != "X":
            continue
        if "parent" not in e["args"]:
            raise ValueError("%s: a span without its node's parent, from a "
                             "crate before the field" % path)
        start = round(e["ts"] * 1000)
        spans.append({"node": e["args"]["node"], "parent": e["args"]["parent"],
                      "name": e["name"], "tid": e["tid"], "start": start,
                      "end": start + round(e["dur"] * 1000)})
    return spans


def paths(spans):
    """{node id: path}, the path a tuple of scope names from the root, () for
    the root; a node no span names is written `#id`."""
    names = {s["node"]: s["name"] for s in spans}
    parents = {s["node"]: s["parent"] for s in spans}

    def path(n):
        p = parents.get(n)
        if p is None:
            return ()
        return path(p) + (names.get(n, "#%d" % n),)

    return {n: path(n) for n in names}


def chain(spans, starts, inst):
    """The critical chain of inst among spans (the spans of its node's
    children sorted by start, starts their starts): from inst's end back,
    the span lying inside inst on any thread that ends last at or before
    the current time, then on from that span's start. Returns the elements
    in time order, spans and the gaps between them as (start, end)."""
    lo = bisect.bisect_left(starts, inst["start"])
    hi = bisect.bisect_right(starts, inst["end"])
    inside = [s for s in spans[lo:hi] if s["end"] <= inst["end"]]
    inside.sort(key=lambda s: (-s["end"], s["start"]))
    t, out = inst["end"], []
    for s in inside:
        if s["end"] > t:
            continue
        if s["end"] < t:
            out.append((s["end"], t))
        out.append(s)
        t = s["start"]
    if t > inst["start"]:
        out.append((inst["start"], t))
    return out[::-1]


def critical(spans, node=None, depth=CRITICAL_DEPTH):
    """Per chosen instance (the longest span of the node at path `node`, a/b,
    else the longest of each top-level scope) its chain to depth levels:
    `instance`, `path`, `rows` as (level, path or None for a gap, thread or
    None, start, duration, share of the enclosing element's duration) in
    time order with each element's own chain after it, and `totals`, per
    path the time on the chain where it is the innermost element followed
    (the gaps of a chain count to the element enclosing them), largest
    first."""
    at = paths(spans)
    kids = {}
    for s in sorted(spans, key=lambda s: s["start"]):
        kids.setdefault(s["parent"], []).append(s)
    starts = {n: [s["start"] for s in ks] for n, ks in kids.items()}
    if node is None:
        chosen = sorted({at[n] for n in at if len(at[n]) == 1},
                        key=lambda p: min(s["start"] for s in spans
                                          if at[s["node"]] == p))
    else:
        want = tuple(node.split("/"))
        chosen = [p for p in dict.fromkeys(at.values()) if p == want]
        if not chosen:
            raise ValueError("no span of %s in the trace" % node)
    out = []
    for p in chosen:
        inst = max((s for s in spans if at[s["node"]] == p),
                   key=lambda s: (s["end"] - s["start"], -s["start"]))
        rows, totals = [], {}

        def within(el):
            n = el["node"]
            return chain(kids.get(n, []), starts.get(n, []), el)

        def follow(el, elements, level):
            """Rows of el's chain elements at level; an element with spans
            inside it is followed below level up to depth, else its time
            counts to its own node."""
            dur = el["end"] - el["start"]
            for e in elements:
                if isinstance(e, tuple):
                    d = e[1] - e[0]
                    rows.append((level, None, None, e[0], d,
                                 d / dur if dur else 0.0))
                    totals[at[el["node"]]] = totals.get(at[el["node"]], 0) + d
                    continue
                d = e["end"] - e["start"]
                rows.append((level, at[e["node"]], e["tid"], e["start"], d,
                             d / dur if dur else 0.0))
                inner = within(e) if level < depth else []
                if any(isinstance(x, dict) for x in inner):
                    follow(e, inner, level + 1)
                else:
                    totals[at[e["node"]]] = totals.get(at[e["node"]], 0) + d

        follow(inst, within(inst), 1)
        out.append({"instance": inst, "path": p, "rows": rows,
                    "totals": sorted(totals.items(), key=lambda t: -t[1])})
    return out


def text(spans, node=None, depth=CRITICAL_DEPTH):
    out = []
    for c in critical(spans, node, depth):
        inst = c["instance"]
        el = inst["end"] - inst["start"]
        out += ["%s: the instance on thread %d at %.3f ms, elapsed %.3f ms; "
                "its chain to depth %d, share of the enclosing element's "
                "time:" % (name(c["path"]), inst["tid"], inst["start"] / 1e6,
                           el / 1e6, depth),
                "  %-36s %9s %9s %6s %6s" % ("element", "start ms", "time ms",
                                             "share", "thread")]
        for level, p, tid, start, d, sh in c["rows"]:
            label = "  " * (level - 1) + (p[-1] if p else "(gap)")
            out.append(("  %-36s %9.3f %9.3f %5.1f%% %6s" % (
                label, (start - inst["start"]) / 1e6, d / 1e6, 100 * sh,
                "" if tid is None else tid)).rstrip())
        out.append("  per node over the chain (a gap counts to the element "
                   "it lies in):")
        out += ["  %-36s %9s %9.3f %5.1f%%" % (name(p), "", t / 1e6,
                                               100 * t / el if el else 0)
                for p, t in c["totals"]]
    return "\n".join(out)
