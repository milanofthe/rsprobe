"""Command line: python -m rsprobe {report,compare,ab,scale} ..."""

import argparse
import sys

from . import ab, compare, report, scale
from .records import load_set


def parser():
    ap = argparse.ArgumentParser(prog="rsprobe",
                                 description="Reads rsprobe run records.")
    sub = ap.add_subparsers(dest="command", required=True)
    r = sub.add_parser("report", help="the tree with corrected times; "
                       "over several records the medians")
    r.add_argument("records", nargs="+", help="record files or directories")
    form = r.add_mutually_exclusive_group()
    form.add_argument("--folded", action="store_true",
                      help="folded stacks of self time (flame graph input)")
    form.add_argument("--json", action="store_true")
    form.add_argument("--threads", nargs="?", const="", metavar="NODE",
                      help="each thread's values, at NODE (a/b) or at every "
                      "node; records taken with Context::record_threads()")
    r.add_argument("--sum", action="store_true",
                   help="the sums over the records instead of the medians")
    r.add_argument("--ratio", action="append", default=[],
                   type=report.asked_ratio, metavar="A/B",
                   help="measure A over measure B per node, e.g. "
                   "width/sections; repeatable")
    c = sub.add_parser("compare", help="two sets of records; exit 1 when a "
                       "node fails the gate")
    c.add_argument("a", help="set A: a record file or a directory")
    c.add_argument("b", help="set B")
    c.add_argument("--aa", help="a second set of A's build, for the noise band")
    a = sub.add_parser("ab", help="run two commands alternately, then compare")
    a.add_argument("--rounds", type=int, required=True)
    a.add_argument("--aa", action="store_true",
                   help="run A a second time per round, for the noise band")
    a.add_argument("--out", help="directory for the records (default: a new "
                   "temporary one)")
    a.add_argument("--case", action="append", default=[], type=ab.case,
                   metavar="NAME=ARGS", help="a configuration: ARGS appended "
                   "to both commands; repeatable, one table across them")
    a.add_argument("cmd", nargs=argparse.REMAINDER,
                   help="-- CMD_A :: CMD_B")
    s = sub.add_parser("scale", help="records over a run parameter: a fit "
                       "per node, or speedup over a thread count")
    s.add_argument("records", nargs="+", help="record files or directories")
    s.add_argument("--by", required=True, help="the parameter, from the "
                   "records' params")
    s.add_argument("--threads", action="store_true",
                   help="the parameter is a thread count: speedup, "
                   "efficiency and serial fraction instead of a fit")
    return ap


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        if args.command == "report":
            runs = load_set(args.records)
            if args.folded:
                print(report.folded(runs))
            elif args.threads is not None:
                print(report.threads(runs, args.threads or None))
            else:
                form = report.as_json if args.json else report.text
                print(form(runs, args.ratio, args.sum))
            return 0
        if args.command == "scale":
            form = scale.text_threads if args.threads else scale.text
            print(form(load_set(args.records), args.by))
            return 0
        if args.command == "ab":
            cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
            sets = ab.run(args.rounds, cmd, args.aa, args.out, args.case)
        else:
            sets = {"": (args.a, args.b, args.aa)}
        cs = [(n, compare.compare(load_set([a]), load_set([b]),
                                  load_set([aa]) if aa else None))
              for n, (a, b, aa) in sets.items()]
        print(compare.table(cs) if args.command == "ab" and args.case
              else compare.text(cs[0][1]))
        return 1 if any(c["gate"] for _, c in cs) else 0
    except (OSError, ValueError, KeyError) as e:
        print("rsprobe: %s" % e, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
