"""`ab`: two commands run alternately, each writing one record per round."""

import os
import shlex
import subprocess
import tempfile
import time


def case(text):
    """`NAME=ARGS` as (NAME, [ARGS split as a shell does])."""
    name, eq, args = text.partition("=")
    if not name or not eq:
        raise ValueError("expected --case NAME=ARGS, got %r" % text)
    return name, shlex.split(args)


def run(rounds, argv, aa, out=None, cases=()):
    """Runs `CMD_A :: CMD_B` (argv split at '::') alternately for the given
    rounds, A, B and with aa A again, each with RSPROBE_RECORD set to its
    record's path under out (a new temporary directory by default), and
    prints a line per finished run. With cases, (NAME, ARGS) pairs, a round
    runs each case in turn, its ARGS appended to both commands and its
    records under out/NAME. Returns {case: (a, b, aa)}, the set directories
    (aa None without aa), with the one case "" without cases."""
    if "::" not in argv:
        raise ValueError("expected CMD_A :: CMD_B")
    i = argv.index("::")
    arms = [("a", argv[:i]), ("b", argv[i + 1:])] + [("aa", argv[:i])] * aa
    if not all(cmd for _, cmd in arms):
        raise ValueError("expected CMD_A :: CMD_B")
    cases = list(cases) or [("", [])]
    if len({c for c, _ in cases}) < len(cases):
        raise ValueError("case names repeat")
    out = out or tempfile.mkdtemp(prefix="rsprobe-ab-")
    dirs = {(c, arm): os.path.join(out, c, arm) for c, _ in cases
            for arm, _ in arms}
    for d in dirs.values():
        os.makedirs(d, exist_ok=True)
    for k in range(rounds):
        for c, args in cases:
            for arm, cmd in arms:
                path = os.path.join(dirs[c, arm], "%04d.json" % k)
                t = time.perf_counter()
                subprocess.run(cmd + args, check=True,
                               env=dict(os.environ, RSPROBE_RECORD=path))
                if not os.path.exists(path):
                    raise ValueError("%s wrote no record to RSPROBE_RECORD"
                                     % " ".join(cmd + args))
                print("round %d/%d %s%-2s %.3f s" % (
                    k + 1, rounds, c and c + " ", arm,
                    time.perf_counter() - t), flush=True)
    print("records in %s" % out, flush=True)
    return {c: (dirs[c, "a"], dirs[c, "b"], dirs.get((c, "aa")))
            for c, _ in cases}
