"""Reading run records and summing them per node."""

import json
import os

# Version of the record's JSON layout this tool reads.
SCHEMA = 1


def load(path):
    """A record file as a dict: `file`, `params`, `inner` and `outer` (the
    calibration constants in ns), `threads`, `exact` (the names of the exact
    measures) and `nodes`, {path: {measure: value}}. A path is the tuple of
    scope names from the root, () for the root; sibling scopes of the same
    name from different call sites are summed into one node."""
    with open(path) as f:
        doc = json.load(f)
    if doc.get("schema") != SCHEMA:
        raise ValueError("%s: record schema %r, this tool reads %d"
                         % (path, doc.get("schema"), SCHEMA))
    names = {s["id"]: s["name"] for s in doc["sites"]}
    by_id = {n["id"]: n for n in doc["nodes"]}

    def path_of(n):
        if n["parent"] is None:
            return ()
        return path_of(by_id[n["parent"]]) + (names[n["site"]],)

    nodes = {}
    for n in doc["nodes"]:
        v = nodes.setdefault(path_of(n), {})
        for m, x in n["values"].items():
            v[m] = v.get(m, 0) + x
    c = doc["calibration"]
    return {
        "file": path,
        "params": doc["params"],
        "inner": c["inner_ns"] or 0.0,
        "outer": c["outer_ns"] or 0.0,
        "threads": doc["threads"],
        "exact": {m for m, k in doc["measures"].items() if k == "exact"},
        "nodes": nodes,
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


def total(trees):
    """{path: {measure: value}} summed over trees: measures are additive, so
    this merges rounds as it merges threads."""
    out = {}
    for t in trees:
        for p, v in t.items():
            o = out.setdefault(p, {})
            for m, x in v.items():
                o[m] = o.get(m, 0) + x
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
