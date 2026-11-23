"""critical on a trace built by hand with a known critical path."""

import json
import os
import tempfile
import unittest

from rsprobe import critical

# A phase on thread 0 over 0..100 us whose tasks run on two workers: thread 1
# runs the long chain 5..40 and 42..90 (with a kernel 50..80 inside the
# second), thread 2 the short one 6..30 and 31..60. A second top-level scope
# follows the phase.
SPANS = [  # name, node, parent, tid, ts, dur (us)
    ("root", 0, None, 0, 0, 120),
    ("phase", 1, 0, 0, 0, 100),
    ("task", 2, 1, 1, 5, 35),
    ("task", 2, 1, 1, 42, 48),
    ("kernel", 3, 2, 1, 50, 30),
    ("task", 2, 1, 2, 6, 24),
    ("task", 2, 1, 2, 31, 29),
    ("after", 4, 0, 0, 105, 10),
]


def trace(directory):
    events = [{"pid": 1, "tid": t, "ph": "M", "name": "thread_name",
               "args": {"name": "thread %d" % t}} for t in (0, 1, 2)]
    events += [{"pid": 1, "tid": t, "ph": "X", "name": n, "ts": ts, "dur": d,
                "args": {"node": node, "parent": parent}}
               for n, node, parent, t, ts, d in SPANS]
    path = os.path.join(directory, "trace.json")
    with open(path, "w") as f:
        json.dump({"displayTimeUnit": "ns", "traceEvents": events}, f)
    return critical.load(path)


class Critical(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.spans = trace(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def test_chain_follows_the_long_worker(self):
        c = critical.critical(self.spans, "phase", depth=1)[0]
        us = [(p and p[-1], tid, start // 1000, d // 1000)
              for _, p, tid, start, d, _ in c["rows"]]
        self.assertEqual(us, [(None, None, 0, 5), ("task", 1, 5, 35),
                              (None, None, 40, 2), ("task", 1, 42, 48),
                              (None, None, 90, 10)])
        self.assertAlmostEqual(c["rows"][3][5], 0.48)
        self.assertEqual(c["totals"], [(("phase", "task"), 83000),
                                       (("phase",), 17000)])

    def test_depth_follows_into_the_elements(self):
        c = critical.critical(self.spans, "phase", depth=2)[0]
        inner = [(p and p[-1], start // 1000, d // 1000)
                 for level, p, _, start, d, _ in c["rows"] if level == 2]
        self.assertEqual(inner, [(None, 42, 8),
                                 ("kernel", 50, 30), (None, 80, 10)])
        totals = dict(c["totals"])
        self.assertEqual(totals[("phase", "task", "kernel")], 30000)
        self.assertEqual(totals[("phase", "task")], 53000)
        self.assertEqual(sum(totals.values()), 100000)

    def test_default_is_each_top_level_scope(self):
        got = [c["path"] for c in critical.critical(self.spans)]
        self.assertEqual(got, [("phase",), ("after",)])
        out = critical.text(self.spans, "phase")
        self.assertIn("phase/task", out)
        with self.assertRaises(ValueError):
            critical.critical(self.spans, "phase/none")


if __name__ == "__main__":
    unittest.main()
