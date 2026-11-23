"""compare on synthetic sets with a known shift and known noise."""

import random
import tempfile
import unittest

from rsprobe.compare import compare, text
from rsprobe.records import load_set

from . import write

ROUNDS = 10


def records(directory, prefix, shift, noise, seed, iterations=10, extra=False):
    """ROUNDS records: a node `work` of 0.9 ms and a `rest` of 0.1 ms, the
    work's time times (1 + shift) and every time times a factor drawn from
    a normal distribution of the given relative noise; in `work` a noisy
    measure `wait` of a tenth of its time."""
    rng = random.Random(seed)
    paths = []
    for k in range(ROUNDS):
        work = round(9e5 * (1 + shift) * rng.gauss(1, noise))
        rest = round(1e5 * rng.gauss(1, noise))
        nodes = {(): {"wall": work + rest},
                 ("work",): {"calls": 1, "wall": work, "iterations": iterations,
                             "wait": work // 10},
                 ("rest",): {"calls": 1, "wall": rest}}
        if extra:
            nodes[("new",)] = {"calls": 1, "wall": 1}
        paths.append(write(directory, "%s%02d.json" % (prefix, k), nodes,
                           noisy=("wait",)))
    return load_set(paths)


class Compare(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.d = self.dir.name

    def tearDown(self):
        self.dir.cleanup()

    def verdict(self, c, path, x="wall"):
        return [r[7] for r in c["noisy"] if r[0] == path and r[1] == x][0]

    def test_ten_percent_at_one_percent_noise_is_flagged(self):
        c = compare(records(self.d, "a", 0, 0.01, 1),
                    records(self.d, "b", 0.10, 0.01, 2))
        self.assertEqual(self.verdict(c, ("work",)), "slower")
        self.assertIn((("work",), "wall"), c["gate"])
        self.assertEqual(self.verdict(c, ("rest",)), "within noise")
        self.assertEqual(c["exact"], [])

    def test_a_noisy_measure_of_the_program_is_compared_with_an_interval(self):
        # it differs from round to round: no exact difference, a verdict
        c = compare(records(self.d, "a", 0, 0.01, 1),
                    records(self.d, "b", 0.10, 0.01, 2))
        self.assertEqual(c["nondeterministic"], [])
        self.assertEqual(self.verdict(c, ("work",), "wait"), "slower")
        self.assertNotIn((("work",), "wait"), c["gate"])
        c = compare(records(self.d, "a", 0, 0.05, 3),
                    records(self.d, "b", 0.01, 0.05, 4))
        self.assertEqual(self.verdict(c, ("work",), "wait"), "within noise")

    def test_ten_percent_faster_does_not_fail_the_gate(self):
        c = compare(records(self.d, "a", 0, 0.01, 1),
                    records(self.d, "b", -0.10, 0.01, 2))
        self.assertEqual(self.verdict(c, ("work",)), "faster")
        self.assertEqual(c["gate"], [])

    def test_one_percent_at_five_percent_noise_is_not(self):
        c = compare(records(self.d, "a", 0, 0.05, 3),
                    records(self.d, "b", 0.01, 0.05, 4))
        self.assertEqual(self.verdict(c, ("work",)), "within noise")
        self.assertEqual(c["gate"], [])

    def test_aa_band(self):
        a = records(self.d, "a", 0, 0.01, 1)
        aa = records(self.d, "aa", 0, 0.01, 5)
        c = compare(a, records(self.d, "b", 0.10, 0.01, 2), aa)
        self.assertEqual(self.verdict(c, ("work",)), "slower")
        self.assertIn("A/A", text(c))
        # A 3 percent shift at 1 percent noise sits beyond a band of A/A
        # sets at 1 percent noise, and inside one of sets at 5 percent.
        b3 = records(self.d, "b3", 0.03, 0.01, 6)
        self.assertEqual(self.verdict(compare(a, b3, aa), ("work",)), "slower")
        noisy = (records(self.d, "n", 0, 0.05, 7), records(self.d, "n2", 0, 0.05, 8))
        c = compare(noisy[0], records(self.d, "b4", 0.03, 0.01, 6), noisy[1])
        self.assertEqual(self.verdict(c, ("work",)), "within noise")

    def test_exact_difference_is_always_reported(self):
        c = compare(records(self.d, "a", 0, 0.05, 3),
                    records(self.d, "b", 0, 0.05, 4, iterations=11))
        self.assertEqual(c["exact"], [(("work",), "iterations", 10, 11)])
        self.assertIn("iterations   10 -> 11", text(c))

    def test_difference_within_a_set(self):
        a = records(self.d, "a", 0, 0.01, 1)
        a[3]["nodes"][("work",)]["iterations"] = 12
        c = compare(a, records(self.d, "b", 0, 0.01, 2))
        self.assertEqual(c["nondeterministic"],
                         [(("work",), "iterations", "A", [10, 12])])
        self.assertEqual(c["exact"], [])

    def test_structure_change(self):
        c = compare(records(self.d, "a", 0, 0.01, 1),
                    records(self.d, "b", 0, 0.01, 2, extra=True))
        self.assertEqual(c["structure"], [(("new",), "B")])

    def test_too_few_rounds(self):
        a = records(self.d, "a", 0, 0.01, 1)[:2]
        c = compare(a, records(self.d, "b", 0.5, 0.01, 2)[:2])
        self.assertEqual(self.verdict(c, ("work",)), "too few rounds")
        self.assertEqual(c["gate"], [])


if __name__ == "__main__":
    unittest.main()
