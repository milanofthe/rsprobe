"""report over several records: medians with their range, exact measures
once or flagged, max measures as levels, --sum, and --threads."""

import json
import tempfile
import unittest

from rsprobe import report
from rsprobe.records import load_set

from . import write

# Three rounds of one run: wall varies, nnz is exact and equal, iterations
# differ in one round, peak is a level. No scope cost.
ROUNDS = [
    {(): {"wall": 1000}, ("solve",): {"calls": 1, "wall": w, "nnz": 50,
                                      "iterations": it, "peak": pk}}
    for w, it, pk in ((400, 7, 300), (600, 7, 900), (500, 8, 600))]


class Medians(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.runs = load_set([write(self.dir.name, "r%d.json" % k, n,
                                    maxima=("peak",))
                              for k, n in enumerate(ROUNDS)])

    def tearDown(self):
        self.dir.cleanup()

    def solve(self, summed):
        rows, _ = report.rows(self.runs, summed=summed)
        return [r for r in rows if r["path"] == ["solve"]][0]

    def test_medians_with_the_range(self):
        r = self.solve(False)
        self.assertEqual((r["wall"], r["calls"]), (500, 1))
        self.assertEqual(r["measures"], {"nnz": 50, "iterations": 7,
                                         "peak": 900})
        self.assertEqual(r["spread"], {"wall": (400, 600), "self": (400, 600),
                                       "iterations": (7, 8)})
        self.assertAlmostEqual(r["share"], 0.5)
        # a level has no time per unit
        self.assertEqual(r["ratios"], {"ns/nnz": 10, "ns/iterations": 500 / 7})

    def test_text(self):
        out = report.text(self.runs)
        self.assertIn("3 records, median over records.", out)
        line = [x for x in out.splitlines() if x.startswith("  solve")][0]
        self.assertIn("wall [0.000..0.001]", line)
        self.assertIn("nnz=50,", line)
        self.assertIn("iterations=7..8 differs", line)
        self.assertIn("peak max 900", line)
        self.assertNotIn("ns/peak", line)
        self.assertIn("50.0%", line)

    def test_sum_restores_the_sums(self):
        r = self.solve(True)
        self.assertEqual((r["wall"], r["calls"]), (1500, 3))
        self.assertEqual(r["measures"], {"nnz": 150, "iterations": 22,
                                         "peak": 900})
        self.assertIn("3 records, summed.", report.text(self.runs, summed=True))
        doc = json.loads(report.as_json(self.runs))
        self.assertEqual(doc["nodes"][1]["spread"]["iterations"], [7, 8])


def with_threads(path):
    """Adds per-thread values to a record of WORKERS: the calling thread at
    the root and `par`, two workers at `par/task` with unequal busy time."""
    with open(path) as f:
        doc = json.load(f)
    ids = {doc["sites"][n["site"]]["name"] if n["site"] is not None else "":
           n["id"] for n in doc["nodes"]}
    doc["per_thread"] = [
        {"index": 0, "handle": False, "nodes": [
            {"id": ids[""], "values": {"wall": 5000}},
            {"id": ids["par"], "values": {"calls": 1, "wall": 4000}}]},
        {"index": 1, "handle": True, "nodes": [
            {"id": ids["par"], "values": {"busy": 3000}},
            {"id": ids["task"], "values": {"calls": 2, "busy": 2900,
                                           "allocs": 4, "peak": 640}}]},
        {"index": 2, "handle": True, "nodes": [
            {"id": ids["par"], "values": {"busy": 1000}},
            {"id": ids["task"], "values": {"calls": 1, "busy": 900,
                                           "allocs": 2, "peak": 320}}]}]
    with open(path, "w") as f:
        json.dump(doc, f)
    return path


WORKERS = {(): {"wall": 5000}, ("par",): {"calls": 1, "wall": 4000,
                                         "busy": 4000},
           ("par", "task"): {"calls": 3, "busy": 3800, "allocs": 6,
                             "peak": 640}}


class Threads(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = with_threads(write(self.dir.name, "t.json", WORKERS,
                                       maxima=("peak",)))

    def tearDown(self):
        self.dir.cleanup()

    def test_a_node(self):
        lines = report.threads(load_set([self.path]), "par/task").splitlines()
        self.assertTrue(lines[0].endswith("3 threads, recorded values (scope "
                                          "cost not subtracted)"))
        rows = [x.split() for x in lines[2:]]
        self.assertEqual(rows, [
            ["task", "1", "handle", "2", "0.000", "0.003", "1.526",
             "allocs=4,", "peak", "max", "640"],
            ["2", "handle", "1", "0.000", "0.001", "0.474", "allocs=2,",
             "peak", "max", "320"]])

    def test_the_tree(self):
        out = report.threads(load_set([self.path]))
        self.assertEqual([x.split()[:3] for x in out.splitlines()[2:]], [
            ["(run)", "0", "context"], ["par", "0", "context"],
            ["1", "handle", "0"], ["2", "handle", "0"],
            ["task", "1", "handle"], ["2", "handle", "1"]])

    def test_errors(self):
        plain = write(self.dir.name, "p.json", WORKERS)
        with self.assertRaises(ValueError):
            report.threads(load_set([plain]))
        with self.assertRaises(ValueError):
            report.threads(load_set([self.path]), "nothing")


if __name__ == "__main__":
    unittest.main()
