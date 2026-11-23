"""`ab`: two commands run alternately, each writing one record per round."""

import os
import subprocess
import tempfile
import time


def run(rounds, argv, aa, out=None):
    """Runs `CMD_A :: CMD_B` (argv split at '::') alternately for the given
    rounds, A, B and with aa A again, each with RSPROBE_RECORD set to its
    record's path under out (a new temporary directory by default), and
    prints a line per finished run. Returns the set directories a, b and aa
    (None without aa)."""
    if "::" not in argv:
        raise ValueError("expected CMD_A :: CMD_B")
    i = argv.index("::")
    arms = [("a", argv[:i]), ("b", argv[i + 1:])] + [("aa", argv[:i])] * aa
    if not all(cmd for _, cmd in arms):
        raise ValueError("expected CMD_A :: CMD_B")
    out = out or tempfile.mkdtemp(prefix="rsprobe-ab-")
    dirs = {arm: os.path.join(out, arm) for arm, _ in arms}
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)
    for k in range(rounds):
        for arm, cmd in arms:
            path = os.path.join(dirs[arm], "%04d.json" % k)
            t = time.perf_counter()
            subprocess.run(cmd, check=True,
                           env=dict(os.environ, RSPROBE_RECORD=path))
            if not os.path.exists(path):
                raise ValueError("%s wrote no record to RSPROBE_RECORD"
                                 % " ".join(cmd))
            print("round %d/%d %-2s %.3f s" % (k + 1, rounds, arm,
                                               time.perf_counter() - t),
                  flush=True)
    print("records in %s" % out, flush=True)
    return dirs["a"], dirs["b"], dirs.get("aa")
