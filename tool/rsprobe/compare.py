"""`compare`: two sets of records, node by node and measure by measure."""

import math

from .constants import CONFIDENCE, GATE, GATE_SHARE, MIN_EFFECT, MIN_ROUNDS
from .overhead import corrected, share
from .records import name, preorder
from .stats import median, ratio_ci


def band(aa, p, x):
    """Half-width of the noise band in log ratio at a node and measure: the
    farther bound of the A/A interval when an A/A set is given, never below
    MIN_EFFECT."""
    floor = math.log(1 + MIN_EFFECT)
    if aa is None:
        return floor
    a, a2 = series(aa[0], p, x), series(aa[1], p, x)
    if len(a) < MIN_ROUNDS or len(a2) < MIN_ROUNDS or not median(a) > 0:
        return floor
    _, lo, hi = ratio_ci(a, a2)
    return max(floor, abs(math.log(lo)) if lo > 0 else math.inf,
               abs(math.log(hi)))


def series(rounds, p, x):
    """The value of measure x at a node in each record of a set, zero where
    the node is absent."""
    return [nodes.get(p, {}).get(x, 0) for nodes, _ in rounds]


def compare(a, b, aa=None, gate=GATE):
    """The comparison of record sets a and b (lists of loaded records), with
    aa a second set of a's build for the noise band. Noisy measures added by
    the program (`time!`) are compared as wall and busy are; a slower verdict
    fails the gate only for the measures in gate. Returns a dict:
    `exact`, `nondeterministic`, `structure` and `noisy` lists, `band`
    (the band's source), `gated` (the measures of the gate) and `gate` (the
    nodes and measures failing it)."""
    A, B = [corrected(r) for r in a], [corrected(r) for r in b]
    AA = (A, [corrected(r) for r in aa]) if aa else None
    exact = set().union(*(r["exact"] for r in a + b))
    noisy = set().union(*(r["noisy"] for r in a + b)) - {"wall", "busy"}
    in_a = {p for nodes, _ in A for p in nodes}
    in_b = {p for nodes, _ in B for p in nodes}
    out = {"exact": [], "nondeterministic": [], "structure": [], "noisy": [],
           "gate": [], "gated": list(dict.fromkeys(gate)),
           "rounds": (len(a), len(b)),
           "band": "A/A, %d records" % len(aa) if aa else "bootstrap alone"}
    for p in preorder([p for nodes, _ in A + B for p in nodes]):
        if (p in in_a) != (p in in_b):
            out["structure"].append((p, "A" if p in in_a else "B"))
            continue
        here = sorted({m for nodes, _ in A + B for m in nodes.get(p, {})})
        for m in [m for m in here if m in exact]:
            va, vb = series(A, p, m), series(B, p, m)
            for side, vs in (("A", va), ("B", vb)):
                if len(set(vs)) > 1:
                    out["nondeterministic"].append((p, m, side, sorted(set(vs))))
            if len(set(va)) == 1 == len(set(vb)) and va[0] != vb[0]:
                out["exact"].append((p, m, va[0], vb[0]))
        for x in ["wall", "busy"] + [m for m in here if m in noisy]:
            va, vb = series(A, p, x), series(B, p, x)
            if not (median(va) > 0 and median(vb) > 0):
                continue
            timed = x in ("wall", "busy")
            sh = median([share(nodes.get(p, {}), run, x if timed else None)
                         for nodes, run in A])
            if min(len(va), len(vb)) < MIN_ROUNDS:
                row = (p, x, median(va), median(vb), None, None, None,
                       "too few rounds", sh)
                out["noisy"].append(row)
                continue
            r, lo, hi = ratio_ci(va, vb)
            w = band(AA, p, x)
            verdict = ("slower" if lo > math.exp(w) else
                       "faster" if hi < math.exp(-w) else "within noise")
            out["noisy"].append((p, x, median(va), median(vb), r, lo, hi,
                                 verdict, sh))
            if verdict == "slower" and sh >= GATE_SHARE and x in gate:
                out["gate"].append((p, x))
    return out


def text(c):
    out = ["A %d records, B %d records; noise band from the %s, at least %g%%."
           % (c["rounds"] + (c["band"], 100 * MIN_EFFECT))]
    out.append("exact measures, A -> B:")
    out += ["  %-32s %-12s %d -> %d" % (name(p), m, x, y)
            for p, m, x, y in c["exact"]] or ["  no difference"]
    for p, m, side, vs in c["nondeterministic"]:
        out.append("  %-32s %-12s differs within %s: %s"
                   % (name(p), m, side, " ".join(map(str, vs))))
    if c["structure"]:
        out.append("structure:")
        out += ["  %-32s only in %s" % (name(p), s) for p, s in c["structure"]]
    out.append("noisy measures, median over records (ms), B/A with its "
               "%g%% bootstrap interval:" % (100 * CONFIDENCE))
    out.append("  %-32s %-9s %10s %10s %7s %17s %6s  %s" % (
        "node", "", "A", "B", "B/A", "interval", "share", "verdict"))
    for p, x, ma, mb, r, lo, hi, verdict, sh in c["noisy"]:
        ci = "[%.3f, %.3f]" % (lo, hi) if r else ""
        out.append("  %-32s %-9s %10.3f %10.3f %7s %17s %5.1f%%  %s" % (
            name(p), x, ma / 1e6, mb / 1e6, "%.3f" % r if r else "", ci,
            100 * sh, verdict))
    out.append("gate: %s" % (
        "fails on " + ", ".join("%s %s" % (name(p), x) for p, x in c["gate"])
        if c["gate"] else "passes"))
    out.append(rule(c))
    return "\n".join(out)


def rule(c):
    """The gate's rule as a footer line."""
    return ("gate rule: fails where a node of at least %g%% of the run is "
            "slower beyond the band in %s; other measures get a verdict "
            "only, --gate MEASURE adds one"
            % (100 * GATE_SHARE, ", ".join(c["gated"])))


def table(cases):
    """One line per (name, comparison): the run's wall time (the root's)
    with its verdict, the number of exact differences and the gate."""
    out = ["run wall time, median over records (ms), B/A with its %g%% "
           "bootstrap interval:" % (100 * CONFIDENCE),
           "%-16s %10s %10s %7s %17s %-13s %5s  %s" % (
               "case", "A", "B", "B/A", "interval", "verdict", "exact",
               "gate")]
    for n, c in cases:
        run = [r for r in c["noisy"] if r[0] == () and r[1] == "wall"]
        _, _, ma, mb, r, lo, hi, verdict, _ = run[0] if run else (
            (), "", 0, 0, None, 0, 0, "no run time", 0)
        out.append("%-16s %10.3f %10.3f %7s %17s %-13s %5d  %s" % (
            n, ma / 1e6, mb / 1e6, "%.3f" % r if r else "",
            "[%.3f, %.3f]" % (lo, hi) if r else "", verdict,
            len(c["exact"]) + len(c["structure"]),
            "fails on " + ", ".join("%s %s" % (name(p), x)
                                    for p, x in c["gate"])
            if c["gate"] else "passes"))
    if cases:
        out.append(rule(cases[0][1]))
    return "\n".join(out)
