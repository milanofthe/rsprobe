"""The correction formula, shares and ratios on a record built by hand."""

import json
import tempfile
import unittest

from rsprobe import report
from rsprobe.records import load_set

from . import write

# inner 10 ns, outer 5 ns, so full 15 ns. A serial solve with iterations
# and a parallel section whose four tasks ran on workers.
NODES = {
    (): {"wall": 10000},
    ("solve",): {"calls": 2, "wall": 8000},
    ("solve", "it"): {"calls": 10, "wall": 5000, "iterations": 10},
    ("par",): {"calls": 1, "wall": 1000, "busy": 3600, "wait": 198},
    ("par", "task"): {"calls": 4, "busy": 3500, "nnz": 700},
}

# By hand, X' = X - calls inner [own domain] - (instances below) full:
# it:    5000 - 10*10            = 4900, self 4900
# solve: 8000 - 2*10 - 10*15     = 7830, self 7830 - 4900 = 2930
# task:  busy 3500 - 4*10        = 3460, self 3460
# par:   wall 1000 - 1*10        = 990 (task is on workers), self 990
#        busy 3600 - 4*15        = 3540
# run:   10000 - (2+10+1)*15     = 9805, self 9805 - 7830 - 990 = 985
# run busy = busy of par = 3540; cost 17*15 = 255 of 10000 + 3600.
EXPECTED = {
    (): (9805, 0, 985, 1.0),
    ("solve",): (7830, 0, 2930, 7830 / 9805),
    ("solve", "it"): (4900, 0, 4900, 4900 / 9805),
    ("par",): (990, 3540, 990, 990 / 9805),
    ("par", "task"): (0, 3460, 3460, 3460 / 3540),
}


class Report(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.runs = load_set([write(self.dir.name, "r.json", NODES, 10, 5,
                                    noisy=("wait",))])

    def tearDown(self):
        self.dir.cleanup()

    def test_correction_and_shares(self):
        rows, run = report.rows(self.runs)
        got = {tuple(r["path"]): r for r in rows}
        for p, (wall, busy, own, share) in EXPECTED.items():
            r = got[p]
            self.assertAlmostEqual(r["wall"], wall, msg=p)
            self.assertAlmostEqual(r["busy"], busy, msg=p)
            self.assertAlmostEqual(r["self"], own, msg=p)
            self.assertAlmostEqual(r["share"], share, msg=p)
        self.assertEqual((run["wall"], run["busy"]), (9805, 3540))
        self.assertAlmostEqual(run["cost"] / run["raw"], 255 / 13600)
        # Scope cost of it: 100 of 5000.
        self.assertAlmostEqual(got[("solve", "it")]["cost_share"], 0.02)
        self.assertEqual([tuple(r["path"]) for r in rows], list(EXPECTED))

    def test_ratios(self):
        rows, _ = report.rows(self.runs)
        got = {tuple(r["path"]): r["ratios"] for r in rows}
        self.assertEqual(got[("solve", "it")], {"ns/iterations": 490})
        self.assertEqual(got[("par", "task")], {"ns/nnz": 3460 / 700})
        # a noisy measure is a time: its share of the node's, not per unit
        self.assertEqual(got[("par",)], {"wait/wall": 198 / 990,
                                         "busy/wall": 3540 / 990})
        line = [x for x in report.text(self.runs).splitlines() if "par" in x][0]
        self.assertIn("wait 0.000 ms", line)

    def test_records_are_summed(self):
        rows, run = report.rows(self.runs * 3)
        self.assertEqual(run["wall"], 3 * 9805)
        self.assertEqual(rows[2]["calls"], 30)
        self.assertAlmostEqual(rows[2]["self"], 3 * 4900)
        self.assertAlmostEqual(rows[2]["share"], 4900 / 9805)

    def test_mark_above_the_stated_share(self):
        nodes = {(): {"wall": 1000}, ("tiny",): {"calls": 10, "wall": 200}}
        runs = load_set([write(self.dir.name, "t.json", nodes, 3, 2)])
        # Scope cost 10*3 of 200 = 15 percent, above the 10 percent mark.
        line = [x for x in report.text(runs).splitlines() if "tiny" in x][0]
        self.assertIn("15.0%!", line)

    def test_root_open_at_record_time(self):
        nodes = dict(NODES)
        nodes[()] = {"wall": 0}
        runs = load_set([write(self.dir.name, "o.json", nodes, 10, 5)])
        _, run = report.rows(runs)
        self.assertEqual(run["wall"], 7830 + 990)

    def test_folded_and_json(self):
        self.assertEqual(report.folded(self.runs).splitlines(), [
            "run 985", "run;solve 2930", "run;solve;it 4900", "run;par 990",
            "run;par;task 3460"])
        doc = json.loads(report.as_json(self.runs))
        self.assertEqual(doc["nodes"][2]["measures"], {"iterations": 10})

    def test_schema_is_checked(self):
        path = write(self.dir.name, "s.json", NODES)
        with open(path) as f:
            doc = json.load(f)
        doc["schema"] = 3
        with open(path, "w") as f:
            json.dump(doc, f)
        with self.assertRaises(ValueError):
            load_set([path])


def edit(path, change):
    """Applies `change` to the JSON document of a record file."""
    with open(path) as f:
        doc = json.load(f)
    change(doc)
    with open(path, "w") as f:
        json.dump(doc, f)
    return path


def solve_in(module):
    """A record whose scope `solve` comes from `module`."""
    def change(doc):
        doc["sites"][0]["module"] = module
    return change


class Records(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def record(self, file, nodes, change):
        return edit(write(self.dir.name, file, nodes), change)

    def test_siblings_match_by_module_and_name(self):
        nodes = {(): {"wall": 100}, ("solve",): {"calls": 1, "wall": 60}}
        a = self.record("a.json", nodes, solve_in("lib::direct"))
        b = self.record("b.json", nodes, solve_in("lib::iterative"))
        c = self.record("c.json", nodes, solve_in("lib::direct"))
        rows, _ = report.rows(load_set([a, b, c]))
        solves = [r for r in rows if r["path"] == ["solve"]]
        self.assertEqual(sorted(r["path"][0].module for r in solves),
                         ["lib::direct", "lib::iterative"])
        self.assertEqual(sorted(r["calls"] for r in solves), [1, 2])

    def test_sites_of_one_module_and_name_are_one_node(self):
        def two_sites(doc):
            doc["sites"].append(dict(doc["sites"][0], id=7, line=9))
            doc["nodes"].append(dict(doc["nodes"][1], id=2, site=7))
        nodes = {(): {"wall": 100}, ("solve",): {"calls": 1, "wall": 30}}
        rows, _ = report.rows(load_set([self.record("s.json", nodes,
                                                    two_sites)]))
        self.assertEqual([(r["path"], r["calls"]) for r in rows],
                         [([], 0), (["solve"], 2)])

    def test_warnings_after_the_tree(self):
        def warn(dropped):
            def change(doc):
                doc["warnings"] = [
                    {"node": 1, "level": "warn", "module": "m", "file": "f",
                     "line": 3, "time": 5, "text": "pivot small",
                     "count": 2},
                    {"node": 0, "level": "error", "module": "m", "file": "f",
                     "line": 4, "time": 6, "text": "no convergence",
                     "count": 1}]
                doc["warnings_dropped"] = dropped
            return change
        nodes = {(): {"wall": 100}, ("solve", "it"): {"calls": 1, "wall": 60},
                 ("solve",): {"calls": 1, "wall": 80}}
        runs = load_set([self.record("w%d.json" % k, nodes, warn(k))
                         for k in range(3)])
        lines = report.text(runs).splitlines()
        at = lines.index("warnings, summed:")
        self.assertGreater(at, max(i for i, x in enumerate(lines)
                                   if x.startswith("    it")))
        rows = [x.split(None, 3) for x in lines[at + 2:]]
        self.assertEqual(rows, [["6", "solve", "warn", "pivot small"],
                                ["3", "(run)", "error", "no convergence"],
                                ["3", "not", "kept,", "beyond the record's bound"]])
        doc = json.loads(report.as_json(runs))
        self.assertEqual(doc["warnings"][0], {"path": ["solve"], "level": "warn",
                                              "text": "pivot small", "count": 6})
        self.assertEqual(doc["warnings_dropped"], 3)

    def test_no_warnings_no_section(self):
        nodes = {(): {"wall": 100}}
        runs = load_set([write(self.dir.name, "n.json", nodes)])
        self.assertNotIn("warnings", report.text(runs))



# A section `sec` on the calling thread whose tasks ran on workers inside the
# tasks of an outer section `par`: `sec` has wall time only, its tasks busy
# time only. inner 10 ns, outer 5 ns.
WORKERS = {
    (): {"wall": 10000},
    ("par",): {"calls": 1, "wall": 3000, "busy": 40000},
    ("par", "outer"): {"calls": 4, "busy": 38000},
    ("sec",): {"calls": 1, "wall": 2000, "width": 8, "sections": 2},
    ("sec", "task"): {"calls": 100, "busy": 30000, "nnz": 600},
}

# By hand: a time measure a node has none of is not corrected.
# par:   wall 3000 - 10 = 2990, self 2990; busy 40000 - 4*15 = 39940
# outer: busy 38000 - 4*10 = 37960, self 37960
# sec:   wall 2000 - 10 = 1990, self 1990; busy 0 (no interval to correct)
# task:  busy 30000 - 100*10 = 29000, self 29000
# run:   wall 10000 - 2*15 = 9970, self 9970 - 2990 - 1990 = 4990
#        busy = busy of par and sec = 39940
WORKERS_EXPECTED = {
    (): (9970, 0, 4990),
    ("par",): (2990, 39940, 2990),
    ("par", "outer"): (0, 37960, 37960),
    ("sec",): (1990, 0, 1990),
    ("sec", "task"): (0, 29000, 29000),
}


class Workers(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.runs = load_set([write(self.dir.name, "w.json", WORKERS, 10, 5)])

    def tearDown(self):
        self.dir.cleanup()

    def test_a_measure_without_time_is_not_corrected(self):
        rows, run = report.rows(self.runs)
        got = {tuple(r["path"]): r for r in rows}
        for p, (wall, busy, own) in WORKERS_EXPECTED.items():
            self.assertEqual((got[p]["wall"], got[p]["busy"], got[p]["self"]),
                             (wall, busy, own), msg=p)
        self.assertEqual((run["wall"], run["busy"]), (9970, 39940))
        self.assertAlmostEqual(got[("sec", "task")]["share"], 29000 / 39940)

    def test_an_open_root_has_no_self_time(self):
        nodes = dict(WORKERS)
        nodes[()] = {"wall": 0}
        runs = load_set([write(self.dir.name, "o.json", nodes, 10, 5)])
        rows, _ = report.rows(runs)
        self.assertEqual((rows[0]["wall"], rows[0]["self"]), (0, 0))

    def test_ns_per_unit_only_where_the_time_is_the_nodes_own(self):
        rows, _ = report.rows(self.runs)
        got = {tuple(r["path"]): r["ratios"] for r in rows}
        # sec waits for tasks timed in busy: no ns/width, no ns/sections
        self.assertEqual(got[("sec",)], {})
        self.assertEqual(got[("sec", "task")], {"ns/nnz": 29000 / 600})

    def test_asked_ratios(self):
        asked = [report.asked_ratio("width/sections"),
                 report.asked_ratio("busy/nnz")]
        rows, _ = report.rows(self.runs, asked)
        got = {tuple(r["path"]): r["ratios"] for r in rows}
        self.assertEqual(got[("sec",)], {"width/sections": 4})
        self.assertEqual(got[("sec", "task")], {"ns/nnz": 29000 / 600,
                                                "busy/nnz": 29000 / 600})
        line = [x for x in report.text(self.runs, asked).splitlines()
                if x.startswith("  sec")][0]
        self.assertIn("width/sections 4", line)
        with self.assertRaises(ValueError):
            report.asked_ratio("width")


if __name__ == "__main__":
    unittest.main()
