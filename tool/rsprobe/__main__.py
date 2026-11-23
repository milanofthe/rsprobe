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
                       "several records are summed")
    r.add_argument("records", nargs="+", help="record files or directories")
    form = r.add_mutually_exclusive_group()
    form.add_argument("--folded", action="store_true",
                      help="folded stacks of self time (flame graph input)")
    form.add_argument("--json", action="store_true")
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
            form = (report.folded if args.folded else
                    report.as_json if args.json else report.text)
            print(form(runs))
            return 0
        if args.command == "scale":
            form = scale.text_threads if args.threads else scale.text
            print(form(load_set(args.records), args.by))
            return 0
        if args.command == "ab":
            cmd = args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd
            args.a, args.b, args.aa = ab.run(args.rounds, cmd, args.aa, args.out)
        c = compare.compare(load_set([args.a]), load_set([args.b]),
                            load_set([args.aa]) if args.aa else None)
        print(compare.text(c))
        return 1 if c["gate"] else 0
    except (OSError, ValueError, KeyError) as e:
        print("rsprobe: %s" % e, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
