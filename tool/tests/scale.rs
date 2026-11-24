//! scale on synthetic series with known exponents and serial fraction.

mod common;

use common::{close, Gauss, Hand};
use rsprobe::Param;
use rsprobe_tool::fmt::path;
use rsprobe_tool::scale::{by_measure, fits, text, text_measure, threads};
use rsprobe_tool::stats::{local_exponent, Rng};
use rsprobe_tool::Run;

const SIZES: [u64; 5] = [1000, 2000, 4000, 8000, 16000];

/// Records over n: `lin` 100 n, `nlogn` 10 n ln n, `quad` n^2 / 10 and their
/// sum at the root, each time times a normal factor of the given relative
/// noise; `lin` adds n nonzeros.
fn series(noise: f64, sizes: &[u64]) -> Vec<Run> {
    let mut g = Gauss::new(3);
    sizes
        .iter()
        .map(|&n| {
            let x = n as f64;
            let t = [100.0 * x, 10.0 * x * x.ln(), x * x / 10.0]
                .map(|v| (v * g.draw(1.0, noise)).round() as u64);
            let hand = Hand {
                params: vec![("n", Param::Num(x))],
                ..Hand::default()
            };
            let (root, lin) = (
                [("wall", t.iter().sum())],
                [("calls", 1), ("wall", t[0]), ("nonzeros", n)],
            );
            let (nlogn, quad) = (
                [("calls", 1), ("wall", t[1])],
                [("calls", 1), ("wall", t[2])],
            );
            hand.run(
                &format!("{n}.json"),
                &[
                    (&[], &root),
                    (&["lin"], &lin),
                    (&["nlogn"], &nlogn),
                    (&["quad"], &quad),
                ],
            )
        })
        .collect()
}

fn fit(runs: &[Run], node: &str, m: &str) -> (f64, f64, i32, f64) {
    let f = fits(runs, "n").unwrap();
    let r = f
        .rows
        .into_iter()
        .find(|r| path(&r.0) == node && r.1 == m)
        .unwrap();
    (r.2, r.3, r.4, r.5)
}

#[test]
fn known_exponents() {
    let runs = series(0.0, &SIZES);
    for (node, c, e, q) in [
        ("lin", 100.0, 1.0, 0),
        ("nlogn", 10.0, 1.0, 1),
        ("quad", 0.1, 2.0, 0),
    ] {
        let got = fit(&runs, node, "wall");
        assert_eq!(got.2, q, "{node}");
        assert!(
            close(got.1, e, 1e-3) && close(got.0 / c, 1.0, 1e-2) && got.3 < 1e-3,
            "{node} {got:?}"
        );
    }
    assert!(close(fit(&runs, "lin", "nonzeros").1, 1.0, 1e-7));
    assert!(close(fit(&runs, "lin", "calls").1, 0.0, 1e-7));
}

#[test]
fn noisy_exponents() {
    let (_, e, q, res) = fit(&series(0.02, &SIZES), "quad", "wall");
    assert!(close(local_exponent(e, q, SIZES[4] as f64), 2.0, 0.05));
    assert!(res > 0.0);
}

#[test]
fn departing_nodes_named() {
    let runs = series(0.0, &SIZES);
    let f = fits(&runs, "n").unwrap();
    // quad, n^2 / 10, has the largest self time at n = 16000; the root is
    // not a scope and not listed.
    assert_eq!(f.dominant.as_ref().map(path), Some("quad".into()));
    assert!(close(f.run.unwrap(), 2.0, 1e-3));
    assert_eq!(
        f.departs.iter().map(|d| path(&d.0)).collect::<Vec<_>>(),
        ["lin", "nlogn"]
    );
    assert!(text(&runs, "n")
        .unwrap()
        .contains("departing by more than 0.1: lin (1.00), nlogn (1.10)"));
}

#[test]
fn fewer_than_three_points_refused() {
    assert!(fits(&series(0.0, &SIZES[..2]), "n").is_err());
}

#[test]
fn missing_parameter() {
    assert!(fits(&series(0.0, &SIZES), "threads").is_err());
}

#[test]
fn known_serial_fraction() {
    // T(t) = T1 (f + (1 - f) / t) with f = 0.1; busy t (T(t) - f T1) makes
    // the waiting share 1 - (T - f T1) / T.
    let (t1, f) = (1e9, 0.1);
    let wall = |t: f64| t1 * (f + (1.0 - f) / t);
    let runs: Vec<Run> = [1.0, 2.0, 4.0, 8.0]
        .iter()
        .map(|&t| {
            let (w, b) = (
                wall(t).round() as u64,
                (t * (wall(t) - f * t1)).round() as u64,
            );
            let hand = Hand {
                params: vec![("threads", Param::Num(t))],
                ..Hand::default()
            };
            hand.run(
                &format!("{t}.json"),
                &[
                    (&[], &[("wall", w)]),
                    (&["par"], &[("calls", 1), ("wall", w), ("busy", b)]),
                ],
            )
        })
        .collect();
    let rows = threads(&runs, "threads").unwrap().rows;
    let got: Vec<_> = rows.iter().filter(|r| path(&r.0) == "par").collect();
    assert_eq!(got.iter().map(|r| r.1).collect::<Vec<_>>(), [2.0, 4.0, 8.0]);
    for (_, t, _, s, e, kf, w) in got {
        assert!(close(*s, 1.0 / (f + (1.0 - f) / t), 1e-7));
        assert!(close(*e, s / t, 1e-7));
        assert!(close(*kf, f, 1e-7));
        assert!(close(w.unwrap(), f * t1 / wall(*t), 1e-7));
    }
}

#[test]
fn time_against_the_nodes_own_measure() {
    // Runs of different kinds: per record `factor` takes 3 ns per nonzero at
    // 2 percent noise, while `other` takes a time drawn independently of its
    // ops; `rest` has no measure. The run's size (here n) does not order
    // either.
    let mut g = Gauss::new(5);
    let mut rng = Rng::new(6);
    let runs: Vec<Run> = (0..12)
        .map(|k| {
            let mut draw = |lo: u64, hi: u64| lo + rng.below((hi - lo) as usize) as u64;
            let (nnz, ops, n) = (
                draw(10_000, 1_000_000),
                draw(10_000, 1_000_000),
                draw(0, 100),
            );
            let o = draw(100_000, 10_000_000);
            let f = (3.0 * nnz as f64 * g.draw(1.0, 0.02)).round() as u64;
            let hand = Hand {
                params: vec![("n", Param::Num(n as f64))],
                ..Hand::default()
            };
            let (factor, other) = (
                [("calls", 1), ("wall", f), ("nonzeros", nnz)],
                [("calls", 1), ("wall", o), ("nonzeros", ops)],
            );
            let tree: common::Tree = &[
                (&[], &[("wall", f + o + 1000)]),
                (&["factor"], &factor),
                (&["other"], &other),
                (&["rest"], &[("calls", 1), ("wall", 1000)]),
            ];
            hand.run(&format!("{k:02}.json"), tree)
        })
        .collect();
    let r = by_measure(&runs, "nonzeros");
    let (_, _, k, _, e, q, res) = r
        .rows
        .iter()
        .find(|r| path(&r.0) == "factor" && r.1 == "wall")
        .unwrap();
    assert_eq!(*k, 12);
    assert!((e + *q as f64 / 1e5f64.ln() - 1.0).abs() < 0.05);
    assert!(*res < 0.05);
    assert_eq!(
        r.poor.iter().map(|p| path(&p.0)).collect::<Vec<_>>(),
        ["other"]
    );
    let skipped: Vec<_> = r
        .skipped
        .iter()
        .map(|s| (path(&s.0), s.1.as_str()))
        .collect();
    assert_eq!(
        skipped,
        [
            ("(run)".to_string(), "absent"),
            ("rest".to_string(), "absent")
        ]
    );
    assert!(text_measure(&runs, "nonzeros")
        .contains("time not following nonzeros, residual above 0.25: other"));
}

#[test]
fn fewer_than_three_values_are_skipped() {
    let runs: Vec<Run> = (0..2)
        .map(|k| {
            common::run(&[
                (&[], &[("wall", 10)]),
                (&["a"], &[("calls", 1), ("wall", 10), ("ops", k + 1)]),
            ])
        })
        .collect();
    let r = by_measure(&runs, "ops");
    assert!(r.rows.is_empty());
    assert!(r
        .skipped
        .iter()
        .any(|s| path(&s.0) == "a" && s.1 == "2 values"));
}
