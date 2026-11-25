//! `ab`: two commands run alternately, each writing one record per round.

use crate::constants::AB_SEED;
use crate::stats::Rng;
use crate::Result;
use rsprobe::units::time;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

/// The record directories of a case: A, B and A again for the noise band.
pub type Sets = (PathBuf, PathBuf, Option<PathBuf>);

/// `NAME=ARGS` as (NAME, ARGS split as a POSIX shell does).
pub fn case(text: &str) -> Result<(String, Vec<String>)> {
    match text.split_once('=') {
        Some((name, args)) if !name.is_empty() => Ok((name.into(), split(args)?)),
        _ => Err(format!("expected --case NAME=ARGS, got '{text}'")),
    }
}

/// Words as a POSIX shell splits them: blanks separate, single quotes keep
/// everything, double quotes keep all but `\"` and `\\`, a backslash
/// outside quotes keeps the next character.
pub fn split(text: &str) -> Result<Vec<String>> {
    let (mut out, mut word, mut open) = (Vec::new(), None::<String>, None::<char>);
    let mut cs = text.chars();
    while let Some(c) = cs.next() {
        match (open, c) {
            (None, c) if c.is_whitespace() => out.extend(word.take()),
            (None, '\'' | '"') => {
                open = Some(c);
                word.get_or_insert_with(String::new);
            }
            (Some(q), c) if c == q => open = None,
            (None, '\\') | (Some('"'), '\\') => {
                let w = word.get_or_insert_with(String::new);
                match cs.next() {
                    Some(n) if open.is_none() || n == '"' || n == '\\' => w.push(n),
                    Some(n) => w.extend(['\\', n]),
                    None => return Err("no escaped character".into()),
                }
            }
            (_, c) => word.get_or_insert_with(String::new).push(c),
        }
    }
    if open.is_some() {
        return Err("no closing quotation".into());
    }
    out.extend(word);
    Ok(out)
}

/// The order of the arms in each round: rotations of arms, in blocks of
/// len(arms) rounds that hold each rotation once, in an order drawn per
/// block with the seed. Over a whole block each arm runs once in each
/// position.
pub fn schedule<T: Clone>(rounds: usize, arms: &[T], seed: u64) -> Vec<Vec<T>> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    while out.len() < rounds && !arms.is_empty() {
        let mut shifts: Vec<usize> = (0..arms.len()).collect();
        rng.shuffle(&mut shifts);
        out.extend(
            shifts
                .into_iter()
                .map(|k| [&arms[k..], &arms[..k]].concat()),
        );
    }
    out.truncate(rounds);
    out
}

/// Runs `CMD_A :: CMD_B` (argv split at `::`) alternately for the given
/// rounds, A, B and with aa A again, in the order of [`schedule`] printed
/// per round, each with RSPROBE_RECORD set to its record's path under out
/// (a new temporary directory by default), and prints a line per finished
/// run. With cases, (NAME, ARGS) pairs, a round runs each case in turn, its
/// ARGS appended to both commands and its records under out/NAME. Returns
/// per case its sets, the one case "" without cases.
pub fn run(
    rounds: usize,
    argv: &[String],
    aa: bool,
    out: Option<&str>,
    cases: &[(String, Vec<String>)],
) -> Result<Vec<(String, Sets)>> {
    let usage = || "expected CMD_A :: CMD_B".to_string();
    let i = argv.iter().position(|a| a == "::").ok_or_else(usage)?;
    let mut arms = vec![("a", &argv[..i]), ("b", &argv[i + 1..])];
    if aa {
        arms.push(("aa", &argv[..i]));
    }
    if arms.iter().any(|a| a.1.is_empty()) {
        return Err(usage());
    }
    let one = [(String::new(), Vec::new())];
    let cases = if cases.is_empty() { &one[..] } else { cases };
    if (1..cases.len()).any(|k| cases[..k].iter().any(|c| c.0 == cases[k].0)) {
        return Err("case names repeat".into());
    }
    let out = match out {
        Some(o) => PathBuf::from(o),
        None => {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            std::env::temp_dir().join(format!(
                "rsprobe-ab-{}-{}",
                std::process::id(),
                t.as_nanos()
            ))
        }
    };
    let dir = |c: &str, arm: &str| out.join(c).join(arm);
    for (c, _) in cases {
        for (arm, _) in &arms {
            std::fs::create_dir_all(dir(c, arm))
                .map_err(|e| format!("{}: {e}", dir(c, arm).display()))?;
        }
    }
    for (k, order) in schedule(rounds, &arms, AB_SEED).iter().enumerate() {
        let names: Vec<&str> = order.iter().map(|a| a.0).collect();
        println!("round {}/{rounds} order {}", k + 1, names.join(" "));
        for (c, args) in cases {
            for (arm, cmd) in order {
                let path = dir(c, arm).join(format!("{k:04}.json"));
                let all: Vec<&String> = cmd.iter().chain(args).collect();
                let line = all.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
                let t = Instant::now();
                let status = Command::new(all[0])
                    .args(&all[1..])
                    .env(rsprobe::RECORD_ENV, &path)
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    Ok(s) => return Err(format!("{line}: {s}")),
                    Err(e) => return Err(format!("{line}: {e}")),
                }
                if !path.exists() {
                    return Err(format!("{line} wrote no record to {}", rsprobe::RECORD_ENV));
                }
                let case = if c.is_empty() {
                    String::new()
                } else {
                    format!("{c} ")
                };
                let ns = t.elapsed().as_nanos() as f64;
                println!("round {}/{rounds} {case}{arm:<2} {}", k + 1, time(ns));
            }
        }
    }
    println!("records in {}", out.display());
    let sets = cases.iter().map(|(c, _)| {
        (
            c.clone(),
            (dir(c, "a"), dir(c, "b"), aa.then(|| dir(c, "aa"))),
        )
    });
    Ok(sets.collect())
}
