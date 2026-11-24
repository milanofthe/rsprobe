//! Command line: rsprobe {report,compare,ab,scale,critical} ...

use rsprobe_tool::constants::{CRITICAL_DEPTH, GATE};
use rsprobe_tool::{ab, compare, critical, load_set, report, scale, Result};

const USAGE: &str = "usage: rsprobe COMMAND ...

Reads rsprobe run records; a set of records is a record file or a directory of them.

  report RECORD... [--folded | --json | --threads [NODE]] [--sum] [--ratio A/B]...
      the tree with corrected times; over several records the medians
  compare A B [--aa A2] [--gate MEASURE]...
      two sets of records; exit 1 when a node fails the gate
  ab --rounds N [--aa] [--out DIR] [--case NAME=ARGS]... [--gate MEASURE]... -- CMD_A :: CMD_B
      run two commands alternately, then compare
  scale RECORD... (--by PARAM [--threads] | --by-measure NAME)
      records over a run parameter: a fit per node, or speedup over a thread count
  critical TRACE [--node PATH] [--depth N]
      the chain of spans that bounds a scope instance's elapsed time, from a trace";

/// Positional arguments and options in order, flags with an empty value.
struct Args {
    pos: Vec<String>,
    opts: Vec<(String, String)>,
}

impl Args {
    /// Parses `argv` against the flags, the options with a value and those
    /// with an optional value (`--name` alone or followed by a word not
    /// starting with '-'). After `--`, and with `rest` after the first
    /// positional, everything is positional.
    fn parse(
        argv: &[String],
        flags: &[&str],
        valued: &[&str],
        optional: &[&str],
        rest: bool,
    ) -> Result<Args> {
        let mut a = Args { pos: Vec::new(), opts: Vec::new() };
        let mut it = argv.iter().peekable();
        while let Some(x) = it.next() {
            if x == "--" || (rest && !x.starts_with("--")) {
                a.pos.extend((x != "--").then(|| x.clone()));
                a.pos.extend(it.cloned());
                break;
            }
            if !x.starts_with("--") {
                a.pos.push(x.clone());
                continue;
            }
            let (k, v) =
                x.split_once('=').map_or((x.as_str(), None), |(k, v)| (k, Some(v.to_string())));
            let v = match () {
                _ if flags.contains(&k) && v.is_none() => String::new(),
                _ if valued.contains(&k) => match v.or_else(|| it.next().cloned()) {
                    Some(v) => v,
                    None => return Err(format!("argument {k}: expected one argument")),
                },
                _ if optional.contains(&k) => v.unwrap_or_else(|| match it.peek() {
                    Some(n) if !n.starts_with('-') => it.next().cloned().unwrap_or_default(),
                    _ => String::new(),
                }),
                _ => return Err(format!("unrecognized arguments: {x}")),
            };
            a.opts.push((k.to_string(), v));
        }
        Ok(a)
    }

    fn all(&self, k: &str) -> Vec<&str> {
        self.opts.iter().filter(|o| o.0 == k).map(|o| o.1.as_str()).collect()
    }

    fn one(&self, k: &str) -> Option<&str> {
        self.all(k).pop()
    }

    fn has(&self, k: &str) -> bool {
        self.one(k).is_some()
    }

    /// At most one of the options.
    fn exclusive(&self, ks: &[&str]) -> Result<()> {
        match ks.iter().filter(|k| self.has(k)).collect::<Vec<_>>()[..] {
            [a, b, ..] => Err(format!("argument {b}: not allowed with argument {a}")),
            _ => Ok(()),
        }
    }

    fn records(&self) -> Result<Vec<rsprobe_tool::Run>> {
        match self.pos.is_empty() {
            true => Err("the following arguments are required: records".into()),
            false => load_set(&self.pos),
        }
    }
}

fn run(argv: &[String]) -> Result<i32> {
    let help = argv.iter().take_while(|a| *a != "--").any(|a| a == "-h" || a == "--help");
    let Some((cmd, argv)) = argv.split_first().filter(|_| !help) else {
        println!("{USAGE}");
        return Ok(if help { 0 } else { 2 });
    };
    let (sets, by_case) = match cmd.as_str() {
        "report" => {
            let a = Args::parse(
                argv,
                &["--folded", "--json", "--sum"],
                &["--ratio"],
                &["--threads"],
                false,
            )?;
            a.exclusive(&["--folded", "--json", "--threads"])?;
            let asked = a
                .all("--ratio")
                .into_iter()
                .map(report::asked_ratio)
                .collect::<Result<Vec<_>>>()?;
            let runs = a.records()?;
            let out = match (a.has("--folded"), a.one("--threads")) {
                (true, _) => report::folded(&runs),
                (_, Some(t)) => report::threads(&runs, Some(t).filter(|t| !t.is_empty()))?,
                _ if a.has("--json") => report::json(&runs, &asked, a.has("--sum")),
                _ => report::text(&runs, &asked, a.has("--sum")),
            };
            println!("{out}");
            return Ok(0);
        }
        "scale" => {
            let a = Args::parse(argv, &["--threads"], &["--by", "--by-measure"], &[], false)?;
            a.exclusive(&["--by", "--by-measure"])?;
            let runs = a.records()?;
            let out = match (a.one("--by"), a.one("--by-measure")) {
                (_, Some(_)) if a.has("--threads") => return Err("--threads takes --by".into()),
                (_, Some(m)) => scale::text_measure(&runs, m),
                (Some(by), _) if a.has("--threads") => scale::text_threads(&runs, by)?,
                (Some(by), _) => scale::text(&runs, by)?,
                _ => return Err("one of the arguments --by --by-measure is required".into()),
            };
            println!("{out}");
            return Ok(0);
        }
        "critical" => {
            let a = Args::parse(argv, &[], &["--node", "--depth"], &[], false)?;
            let [trace] = &a.pos[..] else {
                return Err("expected one trace".into());
            };
            let depth = match a.one("--depth") {
                Some(d) => {
                    d.parse().map_err(|_| format!("argument --depth: invalid int value: '{d}'"))?
                }
                None => CRITICAL_DEPTH,
            };
            println!("{}", critical::text(&critical::load(trace)?, a.one("--node"), depth)?);
            return Ok(0);
        }
        "compare" => {
            let a = Args::parse(argv, &[], &["--aa", "--gate"], &[], false)?;
            let [x, y] = &a.pos[..] else {
                return Err("expected the sets A and B".into());
            };
            let sets = (x.into(), y.into(), a.one("--aa").map(Into::into));
            (vec![(String::new(), sets)], a)
        }
        "ab" => {
            let a = Args::parse(
                argv,
                &["--aa"],
                &["--rounds", "--out", "--case", "--gate"],
                &[],
                true,
            )?;
            let rounds =
                a.one("--rounds").ok_or("the following arguments are required: --rounds")?;
            let rounds = rounds
                .parse()
                .map_err(|_| format!("argument --rounds: invalid int value: '{rounds}'"))?;
            let cases = a.all("--case").into_iter().map(ab::case).collect::<Result<Vec<_>>>()?;
            (ab::run(rounds, &a.pos, a.has("--aa"), a.one("--out"), &cases)?, a)
        }
        c => return Err(format!("argument command: invalid choice: '{c}'\n{USAGE}")),
    };
    let gate: Vec<&str> = GATE.iter().copied().chain(by_case.all("--gate")).collect();
    let mut cs = Vec::new();
    for (n, (a, b, aa)) in sets {
        let aa = aa.map(|aa| load_set(&[aa])).transpose()?;
        let c = compare::compare(&load_set(&[a])?, &load_set(&[b])?, aa.as_deref(), &gate);
        cs.push((n, c));
    }
    match cmd == "ab" && by_case.has("--case") {
        true => println!("{}", compare::table(&cs)),
        false => println!("{}", compare::text(&cs[0].1)),
    }
    Ok(cs.iter().any(|c| !c.1.gate.is_empty()) as i32)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&argv).unwrap_or_else(|e| {
        eprintln!("rsprobe: {e}");
        2
    });
    std::process::exit(code)
}
