"""scale on synthetic series with known exponents and serial fraction."""

import math
import random
import tempfile
import unittest

from rsprobe import scale
from rsprobe.records import load_set
from rsprobe.stats import local_exponent

from . import write

SIZES = (1000, 2000, 4000, 8000, 16000)


def series(directory, noise=0.0, sizes=SIZES):
    """Records over n: `lin` 100 n, `nlogn` 10 n ln n, `quad` n^2 / 10 and
    their sum at the root, each time times a factor of the given relative
    noise; `lin` adds n nonzeros."""
    rng = random.Random(3)
    paths = []
    for n in sizes:
        t = {"lin": 100 * n, "nlogn": 10 * n * math.log(n), "quad": n * n / 10}
        t = {k: round(v * rng.gauss(1, noise)) for k, v in t.items()}
        nodes = {(): {"wall": sum(t.values())}}
        for k, v in t.items():
            nodes[(k,)] = {"calls": 1, "wall": v}
        nodes[("lin",)]["nonzeros"] = n
        paths.append(write(directory, "%d.json" % n, nodes, params={"n": n}))
    return load_set(paths)


class Fit(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def fit(self, runs):
        return {(p, m): (c, e, q, res)
                for p, m, c, e, q, res in scale.fits(runs, "n")["rows"]}

    def test_known_exponents(self):
        f = self.fit(series(self.dir.name))
        for node, c, e, q in (("lin", 100, 1, 0), ("nlogn", 10, 1, 1),
                              ("quad", 0.1, 2, 0)):
            got = f[((node,), "wall")]
            self.assertEqual(got[2], q, node)
            self.assertAlmostEqual(got[1], e, delta=1e-3, msg=node)
            self.assertAlmostEqual(got[0] / c, 1, delta=1e-2, msg=node)
            self.assertLess(got[3], 1e-3, node)
        self.assertAlmostEqual(f[(("lin",), "nonzeros")][1], 1)
        self.assertAlmostEqual(f[(("lin",), "calls")][1], 0)

    def test_noisy_exponents(self):
        f = self.fit(series(self.dir.name, noise=0.02))
        c, e, q, res = f[(("quad",), "wall")]
        self.assertAlmostEqual(local_exponent(e, q, SIZES[-1]), 2, delta=0.05)
        self.assertGreater(res, 0)

    def test_departing_nodes_named(self):
        r = scale.fits(series(self.dir.name), "n")
        # quad, n^2 / 10, has the largest self time at n = 16000; the root
        # is not a scope and not listed.
        self.assertEqual(r["dominant"], ("quad",))
        self.assertAlmostEqual(r["run"], 2, delta=1e-3)
        self.assertEqual([p for p, _ in r["departs"]],
                         [("lin",), ("nlogn",)])
        self.assertIn("departing by more than 0.1: lin (1.00), nlogn (1.10)",
                      scale.text(series(self.dir.name), "n"))

    def test_fewer_than_three_points_refused(self):
        with self.assertRaises(ValueError):
            scale.fits(series(self.dir.name, sizes=SIZES[:2]), "n")

    def test_missing_parameter(self):
        with self.assertRaises(ValueError):
            scale.fits(series(self.dir.name), "threads")


class Threads(unittest.TestCase):
    def test_known_serial_fraction(self):
        # T(t) = T1 (f + (1 - f) / t) with f = 0.1; busy t (T(t) - f T1)
        # makes the waiting share 1 - (T - f T1) / T.
        t1, f = 1e9, 0.1
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for t in (1, 2, 4, 8):
                wall = t1 * (f + (1 - f) / t)
                busy = t * (wall - f * t1)
                nodes = {(): {"wall": wall},
                         ("par",): {"calls": 1, "wall": wall, "busy": busy}}
                paths.append(write(d, "%d.json" % t, nodes,
                                   params={"threads": t}))
            rows = scale.threads(load_set(paths), "threads")["rows"]
        got = {t: (s, e, kf, w) for p, t, _, s, e, kf, w in rows
               if p == ("par",)}
        for t, (s, e, kf, w) in got.items():
            self.assertAlmostEqual(s, 1 / (f + (1 - f) / t))
            self.assertAlmostEqual(e, s / t)
            self.assertAlmostEqual(kf, f)
            wall = t1 * (f + (1 - f) / t)
            self.assertAlmostEqual(w, f * t1 / wall)
        self.assertEqual(sorted(got), [2, 4, 8])


class ByMeasure(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.dir.cleanup()

    def test_time_against_the_nodes_own_measure(self):
        # Runs of different kinds: per record `factor` takes 3 ns per
        # nonzero at 2 percent noise, while `other` takes a time drawn
        # independently of its ops; `rest` has no measure. The run's size
        # (here n) does not order either.
        rng = random.Random(5)
        paths = []
        for k in range(12):
            nnz = rng.randrange(10 ** 4, 10 ** 6)
            ops = rng.randrange(10 ** 4, 10 ** 6)
            f = round(3 * nnz * rng.gauss(1, 0.02))
            o = round(rng.uniform(1e5, 1e7))
            paths.append(write(self.dir.name, "%02d.json" % k, {
                (): {"wall": f + o + 1000},
                ("factor",): {"calls": 1, "wall": f, "nonzeros": nnz},
                ("other",): {"calls": 1, "wall": o, "nonzeros": ops},
                ("rest",): {"calls": 1, "wall": 1000}},
                params={"n": rng.randrange(100)}))
        r = scale.by_measure(load_set(paths), "nonzeros")
        got = {(p, m): (k, c, e, q, res) for p, m, k, c, e, q, res in r["rows"]}
        k, c, e, q, res = got[("factor",), "wall"]
        self.assertEqual(k, 12)
        self.assertLess(abs(e + q / math.log(10 ** 5) - 1), 0.05)
        self.assertLess(res, 0.05)
        self.assertEqual([p for p, _ in r["poor"]], [("other",)])
        self.assertEqual(r["skipped"], [((), "absent"), (("rest",), "absent")])
        out = scale.text_measure(load_set(paths), "nonzeros")
        self.assertIn("time not following nonzeros, residual above 0.25: "
                      "other", out)

    def test_fewer_than_three_values_are_skipped(self):
        paths = [write(self.dir.name, "%d.json" % k, {
            (): {"wall": 10}, ("a",): {"calls": 1, "wall": 10, "ops": k + 1}})
            for k in range(2)]
        r = scale.by_measure(load_set(paths), "ops")
        self.assertEqual(r["rows"], [])
        self.assertIn((("a",), "2 values"), r["skipped"])


if __name__ == "__main__":
    unittest.main()
