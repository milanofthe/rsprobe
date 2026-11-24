//! Records and traces written by the crate (examples/workload.rs) through the
//! library and the binary: the check that the tool reads what the crate
//! writes.

use rsprobe_tool::compare::compare;
use rsprobe_tool::constants::GATE;
use rsprobe_tool::fmt::path;
use rsprobe_tool::{critical, load_set, report, scale};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The workload example, built once with the feature `on`.
fn workload() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let ok = Command::new(env!("CARGO"))
            .args([
                "build",
                "--quiet",
                "-p",
                "rsprobe",
                "--example",
                "workload",
                "--features",
                "on",
            ])
            .status()
            .unwrap();
        assert!(ok.success());
        let exe = std::env::current_exe().unwrap();
        exe.parent().unwrap().parent().unwrap().join("examples").join("workload")
    })
}

/// A directory of `k` records of the workload at size n and t threads,
/// written once per run of the tests.
fn set(n: u32, t: u32, k: u32) -> String {
    static DONE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("roundtrip-{n}-{t}-{k}"));
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    if !done.contains(&dir) {
        _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..k {
            let path = dir.join(format!("{i}.json"));
            let ok = Command::new(workload())
                .args([n.to_string(), t.to_string()])
                .env("RSPROBE_RECORD", path)
                .status();
            assert!(ok.unwrap().success());
        }
        done.push(dir.clone());
    }
    dir.to_str().unwrap().into()
}

/// The binary on the arguments: exit code and stdout.
fn tool(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rsprobe")).args(args).output().unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn report_of_the_workload() {
    let dir = set(1000, 2, 3);
    let (rows, run) = report::rows(&load_set(&[&dir]).unwrap(), &[], true);
    let paths: Vec<String> = rows.iter().map(|r| path(&r.path)).collect();
    assert_eq!(
        paths,
        ["(run)", "assemble", "sort", "solve", "solve/iteration", "pairs", "pairs/task"]
    );
    let at = |p: &str| &rows[paths.iter().position(|x| x == p).unwrap()];
    assert_eq!(at("solve/iteration").v.get("calls"), 15.0);
    assert_eq!(at("assemble").measures().copied().collect::<Vec<_>>(), [("nonzeros", 3000.0)]);
    assert_eq!(at("pairs/task").domain, "busy");
    assert!(at("pairs").ratios.iter().any(|(k, x)| k == "busy/wall" && *x > 0.0));
    assert!(run.get("wall") > 0.0);
    let (code, out) = tool(&["report", &dir]);
    assert!(code == 0 && out.contains("ns/iterations"));
    assert!(tool(&["report", "--folded", &dir]).1.starts_with("run"));
}

#[test]
fn compare_of_two_sizes() {
    let (a, b) = (set(1000, 2, 3), set(1500, 2, 3));
    let c = compare(&load_set(&[&a]).unwrap(), &load_set(&[&b]).unwrap(), None, &GATE);
    let exact: Vec<_> = c.exact.iter().map(|(p, m, x, y)| (path(p), *m, *x, *y)).collect();
    let expected = [
        ("assemble".to_string(), "nonzeros", 1000.0, 1500.0),
        ("pairs/task".to_string(), "differences", 1e6, 2.25e6),
    ];
    assert_eq!(exact, expected);
    assert!(c.nondeterministic.is_empty() && c.structure.is_empty());
    // the caller's wait, a time the program adds: compared with an interval,
    // never as an exact difference
    assert!(c.noisy.iter().any(|n| path(&n.path) == "pairs" && n.measure == "join"));
    let (code, out) = tool(&["compare", &a, &b]);
    assert!(code == 0 || code == 1);
    assert!(out.contains("nonzeros     1000 -> 1500"));
}

#[test]
fn scale_over_size_and_threads() {
    let sizes: Vec<String> = [500, 1000, 1500].map(|n| set(n, 2, 3)).to_vec();
    let f = scale::fits(&load_set(&sizes).unwrap(), "n").unwrap();
    assert_eq!(f.n, [500.0, 1000.0, 1500.0]);
    let exponent =
        |p: &str, m: &str| f.rows.iter().find(|r| path(&r.0) == p && r.1 == m).unwrap().3;
    assert!((exponent("pairs/task", "differences") - 2.0).abs() < 1e-9);
    assert!((exponent("assemble", "nonzeros") - 1.0).abs() < 1e-9);
    let mut args = vec!["scale", "--by", "n"];
    args.extend(sizes.iter().map(String::as_str));
    let (code, out) = tool(&args);
    assert!(code == 0 && out.contains("3 points per fit"));
    let (one, two) = (set(1000, 1, 3), set(1000, 2, 3));
    let (code, out) = tool(&["scale", "--by", "threads", "--threads", &one, &two]);
    assert!(code == 0 && out.contains("pairs"));
}

#[test]
fn critical_path_of_the_pairs() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("roundtrip-trace");
    std::fs::create_dir_all(&dir).unwrap();
    let trace = dir.join("trace.json");
    let ok = Command::new(workload())
        .args(["1000", "3"])
        .env("RSPROBE_TRACE", &trace)
        .env("RSPROBE_RECORD", dir.join("traced.json"))
        .status();
    assert!(ok.unwrap().success());
    let trace = trace.to_str().unwrap();
    let c = &critical::critical(&critical::load(trace).unwrap(), Some("pairs"), 2).unwrap()[0];
    assert_eq!(c.instance.tid, 0);
    let tasks: Vec<_> = c
        .rows
        .iter()
        .filter(|r| r.path.as_ref().is_some_and(|p| p.join("/") == "pairs/task"))
        .collect();
    // the tasks run concurrently: the chain holds the one ending last
    assert_eq!(tasks.len(), 1);
    assert_ne!(tasks[0].tid, Some(0));
    let elapsed = c.instance.end - c.instance.start;
    assert_eq!(c.rows.iter().filter(|r| r.level == 1).map(|r| r.dur).sum::<i64>(), elapsed);
    assert_eq!(c.totals.iter().map(|t| t.1).sum::<i64>(), elapsed);
    let (code, out) = tool(&["critical", trace]);
    assert_eq!(code, 0);
    let heads: Vec<&str> =
        out.lines().filter(|l| !l.starts_with(' ')).map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(heads, ["assemble", "sort", "solve", "pairs"]);
}

#[test]
fn ab_of_two_thread_counts() {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("roundtrip-ab");
    _ = std::fs::remove_dir_all(&out);
    let w = workload().to_str().unwrap();
    let (code, text) = tool(&[
        "ab",
        "--rounds",
        "3",
        "--aa",
        "--out",
        out.to_str().unwrap(),
        "--",
        w,
        "1000",
        "2",
        "::",
        w,
        "1000",
        "1",
    ]);
    assert!(code == 0 || code == 1, "{text}");
    assert!(text.contains("round 3/3 aa"));
    assert!(text.contains("pairs/task                       calls        2 -> 1"));
    assert_eq!(std::fs::read_dir(out.join("b")).unwrap().count(), 3);
}

#[test]
fn threads_and_max() {
    let dir = set(1000, 3, 3);
    let runs = load_set(&[&dir]).unwrap();
    assert_eq!(runs[0].kind("max").into_iter().collect::<Vec<_>>(), ["chunk"]);
    let ts = runs[0].per_thread();
    assert_eq!(ts.iter().map(|t| t.0.handle).collect::<Vec<_>>(), [false, true, true, true]);
    let task = vec![("pairs", "workload"), ("task", "workload")];
    let mut d: Vec<f64> =
        ts[1..].iter().map(|t| t.1.get(&task).unwrap().get("differences")).collect();
    d.sort_by(f64::total_cmp);
    assert_eq!(d, [332000.0, 334000.0, 334000.0]);
    let (code, out) = tool(&["report", "--threads", "pairs/task", &dir]);
    assert_eq!((code, out.matches(" handle ").count()), (0, 9));
    let (_, out) = tool(&["report", &dir]);
    for part in ["3 records, median over records.", "chunk max 334", "differences=1000000,"] {
        assert!(out.contains(part), "{part}");
    }
}
