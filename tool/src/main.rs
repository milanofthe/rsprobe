//! Command line: rsprobe {report,compare,ab,scale,critical} ...

use rsprobe_tool::constants::{CRITICAL_DEPTH, GATE};
use rsprobe_tool::{ab, compare, critical, load_set, report, scale, Result};

const USAGE: &str = "usage: rsprobe COMMAND ...

Reads rsprobe run records; a set of records is a record file or a directory of them.

  report RECORD... [--folded | --threads [NODE]] [--sum] [--ratio A/B]...
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
    /// Parses `argv` against the options in `spec`: `--name` a flag,
    /// `--name=` with a value, `--name?` with an optional value (a following
    /// word not starting with '-'). After `--`, and with `rest` from the first
    /// positional on, everything is positional.
    fn parse(argv: &[String], spec: &[&str], rest: bool) -> Result<Args> {
        let mut a = Args {
            pos: Vec::new(),
            opts: Vec::new(),
        };
        let mut it = argv.iter().peekable();
        while let Some(x) = it.next() {
            if x == "--" || (rest && !x.starts_with("--")) {
                a.pos.extend(
                    (x != "--")
                        .then(|| x.clone())
                        .into_iter()
                        .chain(it.by_ref().cloned()),
                );
            } else if !x.starts_with("--") {
                a.pos.push(x.clone());
            } else {
                let (k, v) = x
                    .split_once('=')
                    .map_or((x.as_str(), None), |(k, v)| (k, Some(v.to_string())));
                let has = |suffix: &str| spec.contains(&format!("{k}{suffix}").as_str());
                let next =
                    |it: &mut std::iter::Peekable<std::slice::Iter<String>>, any: bool| match it
                        .peek()
                    {
                        Some(n) if any || !n.starts_with('-') => it.next().cloned(),
                        _ => None,
                    };
                let v = match () {
                    _ if has("") && v.is_none() => String::new(),
                    _ if has("=") => v
                        .or_else(|| next(&mut it, true))
                        .ok_or(format!("argument {k}: expected one argument"))?,
                    _ if has("?") => v.or_else(|| next(&mut it, false)).unwrap_or_default(),
                    _ => return Err(format!("unrecognized arguments: {x}")),
                };
                a.opts.push((k.to_string(), v));
            }
        }
        Ok(a)
    }

    fn all(&self, k: &str) -> Vec<&str> {
        self.opts
            .iter()
            .filter(|o| o.0 == k)
            .map(|o| o.1.as_str())
            .collect()
    }

    fn one(&self, k: &str) -> Option<&str> {
        self.all(k).pop()
    }

    /// The option's value as an integer, `default` without it.
    fn int(&self, k: &str, default: Option<usize>) -> Result<usize> {
        match self.one(k) {
            Some(v) => v
                .parse()
                .map_err(|_| format!("argument {k}: invalid int value: '{v}'")),
            None => default.ok_or(format!("the following arguments are required: {k}")),
        }
    }

    /// At most one of the options.
    fn exclusive(&self, ks: &[&str]) -> Result<()> {
        match ks
            .iter()
            .filter(|k| self.one(k).is_some())
            .collect::<Vec<_>>()[..]
        {
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
    let help = argv
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "-h" || a == "--help");
    let Some((cmd, argv)) = argv.split_first().filter(|_| !help) else {
        println!("{USAGE}");
        return Ok(if help { 0 } else { 2 });
    };
    let out = match cmd.as_str() {
        "report" => {
            let a = Args::parse(
                argv,
                &["--folded", "--sum", "--ratio=", "--threads?"],
                false,
            )?;
            a.exclusive(&["--folded", "--threads"])?;
            let asked: Vec<_> = a
                .all("--ratio")
                .into_iter()
                .map(report::asked_ratio)
                .collect::<Result<_>>()?;
            let (runs, sum) = (a.records()?, a.one("--sum").is_some());
            match (a.one("--folded"), a.one("--threads")) {
                (Some(_), _) => report::folded(&runs),
                (_, Some(t)) => report::threads(&runs, Some(t).filter(|t| !t.is_empty()))?,
                _ => report::text(&runs, &asked, sum),
            }
        }
        "scale" => {
            let a = Args::parse(argv, &["--threads", "--by=", "--by-measure="], false)?;
            a.exclusive(&["--by", "--by-measure"])?;
            let runs = a.records()?;
            match (
                a.one("--by"),
                a.one("--by-measure"),
                a.one("--threads").is_some(),
            ) {
                (_, Some(_), true) => return Err("--threads takes --by".into()),
                (_, Some(m), _) => scale::text_measure(&runs, m),
                (Some(by), _, true) => scale::text_threads(&runs, by)?,
                (Some(by), _, _) => scale::text(&runs, by)?,
                _ => return Err("one of the arguments --by --by-measure is required".into()),
            }
        }
        "critical" => {
            let a = Args::parse(argv, &["--node=", "--depth="], false)?;
            let [trace] = &a.pos[..] else {
                return Err("expected one trace".into());
            };
            critical::text(
                &critical::load(trace)?,
                a.one("--node"),
                a.int("--depth", Some(CRITICAL_DEPTH))?,
            )?
        }
        "compare" | "ab" => return gate(cmd == "ab", argv),
        c => return Err(format!("argument command: invalid choice: '{c}'\n{USAGE}")),
    };
    println!("{out}");
    Ok(0)
}

/// `compare`, or `ab` and the comparison of its sets; 1 where the gate fails.
fn gate(ab: bool, argv: &[String]) -> Result<i32> {
    let (sets, a) = match ab {
        false => {
            let a = Args::parse(argv, &["--aa=", "--gate="], false)?;
            let [x, y] = &a.pos[..] else {
                return Err("expected the sets A and B".into());
            };
            (
                vec![(
                    String::new(),
                    (x.into(), y.into(), a.one("--aa").map(Into::into)),
                )],
                a,
            )
        }
        true => {
            let a = Args::parse(
                argv,
                &["--aa", "--rounds=", "--out=", "--case=", "--gate="],
                true,
            )?;
            let cases: Vec<_> = a
                .all("--case")
                .into_iter()
                .map(ab::case)
                .collect::<Result<_>>()?;
            (
                ab::run(
                    a.int("--rounds", None)?,
                    &a.pos,
                    a.one("--aa").is_some(),
                    a.one("--out"),
                    &cases,
                )?,
                a,
            )
        }
    };
    let gate: Vec<&str> = GATE.iter().copied().chain(a.all("--gate")).collect();
    let mut cs = Vec::new();
    for (n, (x, y, aa)) in sets {
        let aa = aa.map(|aa| load_set(&[aa])).transpose()?;
        cs.push((
            n,
            compare::compare(&load_set(&[x])?, &load_set(&[y])?, aa.as_deref(), &gate),
        ));
    }
    match ab && !a.all("--case").is_empty() {
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
