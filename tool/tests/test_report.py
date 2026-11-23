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
    ("par",): {"calls": 1, "wall": 1000, "busy": 3600},
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
        self.runs = load_set([write(self.dir.name, "r.json", NODES, 10, 5)])

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
        self.assertEqual(got[("par",)], {"busy/wall": 3540 / 990})

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
        doc["schema"] = 2
        with open(path, "w") as f:
            json.dump(doc, f)
        with self.assertRaises(ValueError):
            load_set([path])


if __name__ == "__main__":
    unittest.main()
