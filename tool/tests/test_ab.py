"""`ab` over several configurations, with commands that write records built
by hand."""

import os
import sys
import tempfile
import unittest

from rsprobe.ab import case, schedule

from .test_roundtrip import run

TOOL = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# A command: `python3 FAKE FACTOR N` writes a record with a root of N us
# times FACTOR and an exact measure `n` = N to RSPROBE_RECORD.
FAKE = """import os, sys
sys.path.insert(0, %r)
from tests import write
f, n = float(sys.argv[1]), int(sys.argv[2])
path = os.environ["RSPROBE_RECORD"]
write(os.path.dirname(path), os.path.basename(path),
      {(): {"wall": round(1000 * n * f)}, ("w",): {"calls": 1, "n": n}})
""" % TOOL


class Cases(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.fake = os.path.join(self.dir.name, "fake.py")
        with open(self.fake, "w") as f:
            f.write(FAKE)

    def tearDown(self):
        self.dir.cleanup()

    def test_one_table_across_cases(self):
        out = os.path.join(self.dir.name, "ab")
        code, text = run("ab", "--rounds", "3", "--out", out,
                         "--case", "small=10", "--case", "large=100", "--",
                         sys.executable, self.fake, "1", "::",
                         sys.executable, self.fake, "2")
        self.assertEqual(code, 1)
        self.assertIn("round 3/3 large b", text)
        lines = text.splitlines()
        at = [i for i, x in enumerate(lines) if x.startswith("case")][0]
        rows = [x.split() for x in lines[at + 1:-1]]
        # B takes twice A's time at every size: run wall in ms, B/A 2.
        self.assertEqual(rows[0][:4], ["small", "0.010", "0.020", "2.000"])
        self.assertEqual(rows[1][:4], ["large", "0.100", "0.200", "2.000"])
        self.assertEqual([r[6:] for r in rows],
                         [["slower", "0", "fails", "on", "(run)", "wall"]] * 2)
        self.assertEqual(len(os.listdir(os.path.join(out, "large", "b"))), 3)

    def test_order_rotates(self):
        arms = ["a", "b", "aa"]
        s = schedule(12, arms)
        self.assertEqual(s, schedule(12, arms))
        for order in s:
            self.assertEqual(sorted(order), sorted(arms))
        # each block of three rounds puts every arm once in every position
        for k in range(0, 12, 3):
            for i in range(3):
                self.assertEqual(sorted(o[i] for o in s[k:k + 3]), sorted(arms))
        self.assertGreater(len({tuple(o) for o in s}), 1)
        self.assertNotEqual(s, schedule(12, arms, seed=2))

    def test_case_syntax(self):
        self.assertEqual(case("n=1000 --threads 4"),
                         ("n", ["1000", "--threads", "4"]))
        with self.assertRaises(ValueError):
            case("no-equals")


if __name__ == "__main__":
    unittest.main()
