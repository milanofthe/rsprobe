"""Tests of the tool; run from the repository root with
python3 -m unittest discover -s tool/tests -t tool"""

import json
import os


def write(directory, file, nodes, inner=0.0, outer=0.0, params=None,
          noisy=(), maxima=()):
    """Writes a record built by hand: nodes as {path tuple: {measure:
    value}}, the root () included; wall, busy and the measures in noisy are
    declared noisy (as the crate declares those added by `time!`), those in
    maxima max (`max!`, schema 2), every other one exact (`add!`)."""
    paths = sorted(nodes, key=len)
    names = sorted({p[-1] for p in paths if p})
    site = {n: i for i, n in enumerate(names)}
    ids = {p: i for i, p in enumerate(paths)}
    measures = {"calls": "exact", "wall": "noisy", "busy": "noisy"}
    measures.update((m, "noisy") for m in noisy)
    measures.update((m, "max") for m in maxima)
    for v in nodes.values():
        for m in v:
            measures.setdefault(m, "exact")
    doc = {
        "schema": 2 if maxima else 1,
        "params": params or {},
        "calibration": {"inner_ns": inner, "outer_ns": outer},
        "threads": 1,
        "measures": measures,
        "sites": [{"id": i, "name": n, "module": "m", "file": "f", "line": 1,
                   "level": "info"} for n, i in site.items()],
        "nodes": [{"id": ids[p], "parent": ids[p[:-1]] if p else None,
                   "site": site[p[-1]] if p else None,
                   "values": dict({"calls": 0, "wall": 0, "busy": 0},
                                  **nodes[p])} for p in paths],
    }
    path = os.path.join(directory, file)
    with open(path, "w") as f:
        json.dump(doc, f)
    return path
