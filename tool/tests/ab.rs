//! `ab` over several configurations, with this test's binary as the command:
//! `ab fake FACTOR N` writes a record with a root of N us times FACTOR and an
//! exact measure `n` = N to RSPROBE_RECORD.

mod common;

use rsprobe_tool::ab::{case, schedule};
use std::process::Command;

fn fake(args: &[String]) {
    let (f, n): (f64, u64) = (args[0].parse().unwrap(), args[1].parse().unwrap());
    let r = common::Hand::default().record(&[
        (&[], &[("wall", (1000.0 * n as f64 * f).round() as u64)]),
        (&["w"], &[("calls", 1), ("n", n)]),
    ]);
    std::fs::write(std::env::var("RSPROBE_RECORD").unwrap(), r.to_string()).unwrap();
}

fn one_table_across_cases() {
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ab-cases");
    _ = std::fs::remove_dir_all(&out);
    let me = std::env::current_exe().unwrap();
    let me = me.to_str().unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_rsprobe"))
        .args([
            "ab",
            "--rounds",
            "3",
            "--out",
            out.to_str().unwrap(),
            "--case",
            "small=10",
            "--case",
            "large=100",
        ])
        .args(["--", me, "fake", "1", "::", me, "fake", "2"])
        .output()
        .unwrap();
    let text = String::from_utf8(run.stdout).unwrap();
    assert_eq!(run.status.code(), Some(1), "{text}");
    assert!(text.contains("round 3/3 large b"));
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().position(|l| l.starts_with("case")).unwrap();
    let rows: Vec<Vec<&str>> = lines[at + 1..lines.len() - 1]
        .iter()
        .map(|l| l.split_whitespace().collect())
        .collect();
    // B takes twice A's time at every size: run wall in ms, B/A 2.
    assert_eq!(rows[0][..4], ["small", "0.010", "0.020", "2.000"]);
    assert_eq!(rows[1][..4], ["large", "0.100", "0.200", "2.000"]);
    for r in &rows {
        assert_eq!(r[6..], ["slower", "0", "fails", "on", "(run)", "wall"]);
    }
    assert_eq!(std::fs::read_dir(out.join("large/b")).unwrap().count(), 3);
    std::fs::remove_dir_all(&out).unwrap();
}

fn order_rotates() {
    let arms = ["a", "b", "aa"];
    let s = schedule(12, &arms, 1);
    assert_eq!(s, schedule(12, &arms, 1));
    for order in &s {
        let mut o = order.clone();
        o.sort();
        assert_eq!(o, ["a", "aa", "b"]);
    }
    // each block of three rounds puts every arm once in every position
    for block in s.chunks(3) {
        for i in 0..3 {
            let mut at: Vec<&str> = block.iter().map(|o| o[i]).collect();
            at.sort();
            assert_eq!(at, ["a", "aa", "b"]);
        }
    }
    assert!(s.iter().any(|o| *o != s[0]));
    assert_ne!(s, schedule(12, &arms, 2));
}

fn case_syntax() {
    assert_eq!(
        case("n=1000 --threads 4").unwrap(),
        (
            "n".into(),
            vec!["1000".into(), "--threads".into(), "4".into()]
        )
    );
    assert_eq!(
        case(r#"x=a 'b c' "d \"e\"" f\ g"#).unwrap().1,
        ["a", "b c", "d \"e\"", "f g"]
    );
    assert!(case("no-equals").is_err());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("fake") {
        return fake(&args[1..]);
    }
    for (name, test) in [
        ("one_table_across_cases", one_table_across_cases as fn()),
        ("order_rotates", order_rotates),
        ("case_syntax", case_syntax),
    ] {
        test();
        println!("test {name} ... ok");
    }
}
