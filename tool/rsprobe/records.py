"""Reading run records and summing them per node."""

import json
import os

# Versions of the record's JSON layout this tool reads: 2 adds the measure
# kind `max`.
SCHEMAS = (1, 2)


class Scope(str):
    """A scope name in a path, carrying the module of its call site. Two
    scopes are equal where module and name are; a plain string matches the
    name alone, so a path can be written as a tuple of names."""

    def __new__(cls, name, module):
        s = super().__new__(cls, name)
        s.module = module
        return s

    def __eq__(self, other):
        if not isinstance(other, str):
            return NotImplemented
        return (str.__eq__(self, other)
                and getattr(other, "module", self.module) == self.module)

    def __ne__(self, other):
        eq = self.__eq__(other)
        return eq if eq is NotImplemented else not eq

    __hash__ = str.__hash__


def load(path):
    """A record file as a dict: `file`, `params`, `inner` and `outer` (the
    calibration constants in ns), `threads`, `exact`, `noisy` and `max` (the
    names of the exact, the noisy and the maximum measures), `nodes`, {path: {measure: value}}, `warnings`, a list of
    (path, level, text, count), `warnings_dropped`, and `per_thread`, per
    thread of a record taken with `record_threads` a dict of `index`,
    `handle` and `nodes` as above (empty otherwise). A path is the tuple of
    scopes (`Scope`) from the root, () for the root; sibling scopes of the
    same module and name from different call sites are merged into one node
    (summed, `max` measures by maximum), in a record and across records."""
    with open(path) as f:
        doc = json.load(f)
    if doc.get("schema") not in SCHEMAS:
        raise ValueError("%s: record schema %r, this tool reads %s"
                         % (path, doc.get("schema"), SCHEMAS))
    kinds = doc["measures"]
    maxima = {m for m, k in kinds.items() if k == "max"}
    names = {s["id"]: Scope(s["name"], s["module"]) for s in doc["sites"]}
    by_id = {n["id"]: n for n in doc["nodes"]}

    def path_of(n):
        if n["parent"] is None:
            return ()
        return path_of(by_id[n["parent"]]) + (names[n["site"]],)

    nodes = total(({path_of(n): n["values"]} for n in doc["nodes"]), maxima)
    c = doc["calibration"]
    return {
        "file": path,
        "params": doc["params"],
        "inner": c["inner_ns"] or 0.0,
        "outer": c["outer_ns"] or 0.0,
        "threads": doc["threads"],
        "exact": {m for m, k in kinds.items() if k == "exact"},
        "noisy": {m for m, k in kinds.items() if k == "noisy"},
        "max": maxima,
        "nodes": nodes,
        "warnings": [(path_of(by_id[w["node"]]), w["level"], w["text"],
                      w["count"]) for w in doc.get("warnings", [])],
        "warnings_dropped": doc.get("warnings_dropped", 0),
        "per_thread": [
            {"index": t["index"], "handle": t["handle"],
             "nodes": total(({path_of(by_id[n["id"]]): n["values"]}
                             for n in t["nodes"]), maxima)}
            for t in doc.get("per_thread", [])],
    }


def load_set(args):
    """The records of a set: each argument a record file or a directory whose
    `*.json` files are taken in name order."""
    files = []
    for a in args:
        if os.path.isdir(a):
            files += sorted(os.path.join(a, f) for f in os.listdir(a)
                            if f.endswith(".json"))
        else:
            files.append(a)
    if not files:
        raise ValueError("no records in %s" % " ".join(args))
    return [load(f) for f in files]


def total(trees, maxima=()):
    """{path: {measure: value}} merged over trees, the measures in `maxima`
    by maximum and the others summed, as the crate merges threads."""
    out = {}
    for t in trees:
        for p, v in t.items():
            o = out.setdefault(p, {})
            for m, x in v.items():
                o[m] = max(o.get(m, 0), x) if m in maxima else o.get(m, 0) + x
    return out


def children(paths):
    """{path: [child paths]} in order of first appearance."""
    kids = {p: [] for p in paths}
    for p in paths:
        if p:
            kids[p[:-1]].append(p)
    return kids


def preorder(paths):
    """The distinct paths, parents first, siblings in order of first
    appearance."""
    paths = list(dict.fromkeys(paths))
    first = {p: i for i, p in enumerate(paths)}
    return sorted(paths, key=lambda p: [first[p[:k]] for k in range(len(p) + 1)])


def name(path):
    """A path as text: scope names joined by '/', the root as '(run)'."""
    return "/".join(path) or "(run)"
