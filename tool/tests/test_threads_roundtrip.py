"""A record of examples/workload.rs with each thread's values and a max
measure, through `report --threads` and the median view."""

import os
import subprocess
import tempfile
import unittest

from rsprobe.records import load_set

from .test_roundtrip import BIN, run, setUpModule  # noqa: F401


class ThreadsRoundTrip(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        for k in range(3):
            subprocess.run([BIN, "1000", "3"], check=True, env=dict(
                os.environ, RSPROBE_RECORD=os.path.join(self.dir.name,
                                                        "%d.json" % k)))

    def tearDown(self):
        self.dir.cleanup()

    def test_threads_and_max(self):
        runs = load_set([self.dir.name])
        self.assertEqual(runs[0]["max"], {"chunk"})
        ts = runs[0]["per_thread"]
        self.assertEqual([t["handle"] for t in ts], [False, True, True, True])
        task = ("pairs", "task")
        self.assertEqual(sorted(t["nodes"][task]["differences"]
                                for t in ts[1:]), [332000, 334000, 334000])
        code, out = run("report", "--threads", "pairs/task", self.dir.name)
        self.assertEqual(code, 0)
        self.assertEqual(out.count(" handle "), 9)
        code, out = run("report", self.dir.name)
        self.assertIn("3 records, median over records.", out)
        self.assertIn("chunk max 334", out)
        self.assertIn("differences=1000000,", out)


if __name__ == "__main__":
    unittest.main()
