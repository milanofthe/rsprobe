"""Measures of kind max: merged by maximum within a record and over records."""

import tempfile
import unittest

from rsprobe.records import load_set
from rsprobe.report import summary

from . import write


class Kinds(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def test_max_is_merged_by_maximum(self):
        def record(file, peak):
            nodes = {(): {"wall": 100},
                     ("solve",): {"calls": 1, "wall": 50, "peak": peak,
                                  "nnz": 10}}
            return write(self.dir.name, file, nodes, maxima=("peak",))
        runs = load_set([record("a.json", 300), record("b.json", 700)])
        self.assertEqual(runs[0]["max"], {"peak"})
        nodes, _ = summary(runs)
        self.assertEqual(nodes[("solve",)]["peak"], 700)
        self.assertEqual(nodes[("solve",)]["nnz"], 20)


if __name__ == "__main__":
    unittest.main()
