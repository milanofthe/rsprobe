"""Records written by the crate (examples/workload.rs) through the commands:
the check that the tool reads the schema the crate writes."""

import contextlib
import io
import os
import subprocess
import tempfile
import unittest

from rsprobe.__main__ import main
from rsprobe import scale
from rsprobe.compare import compare
from rsprobe.records import load_set
from rsprobe.report import rows

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BIN = os.path.join(ROOT, "target", "debug", "examples", "workload")


def setUpModule():
    subprocess.run(["cargo", "build", "--quiet", "--example", "workload",
                    "--features", "on"], cwd=ROOT, check=True)


def run(*argv):
    """main(argv) and its stdout."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = main(list(argv))
    return code, out.getvalue()


class RoundTrip(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.TemporaryDirectory()
        cls.sets = {}
        for n, t in ((1000, 2), (1500, 2), (500, 2), (1000, 1)):
            d = os.path.join(cls.dir.name, "%d-%d" % (n, t))
            os.makedirs(d)
            for k in range(3):
                subprocess.run([BIN, str(n), str(t)], check=True, env=dict(
                    os.environ, RSPROBE_RECORD=os.path.join(d, "%d.json" % k)))
            cls.sets[n, t] = d

    @classmethod
    def tearDownClass(cls):
        cls.dir.cleanup()

    def test_report(self):
        rs, run_ = rows(load_set([self.sets[1000, 2]]))
        got = {"/".join(r["path"]): r for r in rs}
        self.assertEqual(list(got), ["", "assemble", "sort", "solve",
                                     "solve/iteration", "pairs", "pairs/task"])
        self.assertEqual(got["solve/iteration"]["calls"], 15)
        self.assertEqual(got["assemble"]["measures"], {"nonzeros": 3000})
        self.assertEqual(got["pairs/task"]["domain"], "busy")
        self.assertGreater(got["pairs"]["ratios"]["busy/wall"], 0)
        self.assertGreater(run_["wall"], 0)
        code, out = run("report", self.sets[1000, 2])
        self.assertEqual(code, 0)
        self.assertIn("ns/iterations", out)
        code, out = run("report", "--folded", self.sets[1000, 2])
        self.assertTrue(out.startswith("run"))

    def test_compare(self):
        c = compare(load_set([self.sets[1000, 2]]), load_set([self.sets[1500, 2]]))
        self.assertEqual(c["exact"], [
            (("assemble",), "nonzeros", 1000, 1500),
            (("pairs", "task"), "differences", 1000 ** 2, 1500 ** 2)])
        self.assertEqual(c["nondeterministic"], [])
        self.assertEqual(c["structure"], [])
        # the caller's wait, a time the program adds: compared with an
        # interval, never as an exact difference
        self.assertIn((("pairs",), "join"), [r[:2] for r in c["noisy"]])
        code, out = run("compare", self.sets[1000, 2], self.sets[1500, 2])
        self.assertIn(code, (0, 1))
        self.assertIn("nonzeros     1000 -> 1500", out)

    def test_scale(self):
        sizes = [self.sets[n, 2] for n in (500, 1000, 1500)]
        r = scale.fits(load_set(sizes), "n")
        self.assertEqual(r["n"], [500, 1000, 1500])
        got = {(p, m): e for p, m, c, e, q, res in r["rows"]}
        self.assertAlmostEqual(got[("pairs", "task"), "differences"], 2)
        self.assertAlmostEqual(got[("assemble",), "nonzeros"], 1)
        code, out = run("scale", "--by", "n", *sizes)
        self.assertEqual(code, 0)
        self.assertIn("3 points per fit", out)
        threads = [self.sets[1000, 1], self.sets[1000, 2]]
        code, out = run("scale", "--by", "threads", "--threads", *threads)
        self.assertEqual(code, 0)
        self.assertIn("pairs", out)

    def test_ab(self):
        out_dir = os.path.join(self.dir.name, "ab")
        code, out = run("ab", "--rounds", "3", "--aa", "--out", out_dir, "--",
                        BIN, "1000", "2", "::", BIN, "1000", "1")
        self.assertIn(code, (0, 1))
        self.assertIn("round 3/3 aa", out)
        self.assertIn("pairs/task                       calls        2 -> 1", out)
        self.assertEqual(len(os.listdir(os.path.join(out_dir, "b"))), 3)


if __name__ == "__main__":
    unittest.main()
