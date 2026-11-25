//! compare on synthetic sets with a known shift and known noise. The
//! bootstrap intervals are checked by their properties: reproducible, the
//! ratio of the medians inside, verdicts on clear cases.

mod common;

use common::{Gauss, Hand};
use rsprobe_tool::compare::{compare, text, Comparison};
use rsprobe_tool::constants::GATE;
use rsprobe_tool::Run;

const ROUNDS: usize = 10;

/// ROUNDS records: a node `work` of 0.9 ms and a `rest` of 0.1 ms, the
/// work's time times (1 + shift) and every time times a normal factor of the
/// given relative noise; in `work` a noisy measure `wait` of a tenth of its
/// time.
fn records(shift: f64, noise: f64, seed: u64, iterations: u64, extra: bool) -> Vec<Run> {
    let mut g = Gauss::new(seed);
    let hand = Hand {
        noisy: vec!["wait"],
        ..Hand::default()
    };
    (0..ROUNDS)
        .map(|k| {
            let work = (9e5 * (1.0 + shift) * g.draw(1.0, noise)).round() as u64;
            let rest = (1e5 * g.draw(1.0, noise)).round() as u64;
            let w = [
                ("calls", 1),
                ("wall", work),
                ("iterations", iterations),
                ("wait", work / 10),
            ];
            let (root, r) = ([("wall", work + rest)], [("calls", 1), ("wall", rest)]);
            let mut tree: Vec<common::Entry> = vec![(&[], &root), (&["work"], &w), (&["rest"], &r)];
            if extra {
                tree.push((&["new"], &[("calls", 1), ("wall", 1)]));
            }
            hand.run(&format!("{k:02}.json"), &tree)
        })
        .collect()
}

fn set(shift: f64, noise: f64, seed: u64) -> Vec<Run> {
    records(shift, noise, seed, 10, false)
}

fn verdict(c: &Comparison, node: &str, x: &str) -> &'static str {
    let n = c
        .noisy
        .iter()
        .find(|n| n.measure == x && n.path.iter().map(|s| s.0).eq(node.split('/')));
    n.unwrap().verdict
}

fn gated(c: &Comparison) -> Vec<(String, &str)> {
    c.gate
        .iter()
        .map(|(p, x)| (rsprobe_tool::fmt::path(p), *x))
        .collect()
}

#[test]
fn ten_percent_at_one_percent_noise_is_flagged() {
    let c = compare(&set(0.0, 0.01, 1), &set(0.10, 0.01, 2), None, &GATE);
    assert_eq!(verdict(&c, "work", "wall"), "slower");
    assert!(gated(&c).contains(&("work".into(), "wall")));
    assert_eq!(verdict(&c, "rest", "wall"), "within noise");
    assert!(c.exact.is_empty());
}

#[test]
fn intervals_are_reproducible_and_hold_the_ratio_of_medians() {
    let (a, b) = (set(0.0, 0.05, 3), set(0.02, 0.05, 4));
    let (c, d) = (compare(&a, &b, None, &GATE), compare(&a, &b, None, &GATE));
    assert_eq!(text(&c), text(&d));
    for n in &c.noisy {
        let (r, lo, hi) = n.ratio.unwrap();
        assert!(lo <= r && r <= hi, "{lo} {r} {hi}");
        assert!(hi / lo < 1.2, "{lo} {hi}");
    }
}

#[test]
fn a_noisy_measure_of_the_program_is_compared_with_an_interval() {
    // it differs from round to round: no exact difference, a verdict
    let c = compare(&set(0.0, 0.01, 1), &set(0.10, 0.01, 2), None, &GATE);
    assert!(c.nondeterministic.is_empty());
    assert_eq!(verdict(&c, "work", "wait"), "slower");
    assert!(!gated(&c).contains(&("work".into(), "wait")));
    let c = compare(&set(0.0, 0.05, 3), &set(0.01, 0.05, 4), None, &GATE);
    assert_eq!(verdict(&c, "work", "wait"), "within noise");
}

#[test]
fn ten_percent_faster_does_not_fail_the_gate() {
    let c = compare(&set(0.0, 0.01, 1), &set(-0.10, 0.01, 2), None, &GATE);
    assert_eq!(verdict(&c, "work", "wall"), "faster");
    assert!(c.gate.is_empty());
}

#[test]
fn one_percent_at_five_percent_noise_is_not() {
    let c = compare(&set(0.0, 0.05, 3), &set(0.01, 0.05, 4), None, &GATE);
    assert_eq!(verdict(&c, "work", "wall"), "within noise");
    assert!(c.gate.is_empty());
}

#[test]
fn aa_band() {
    let (a, aa) = (set(0.0, 0.01, 1), set(0.0, 0.01, 5));
    let c = compare(&a, &set(0.10, 0.01, 2), Some(&aa), &GATE);
    assert_eq!(verdict(&c, "work", "wall"), "slower");
    assert!(text(&c).contains("A/A"));
    // A 5 percent shift at 1 percent noise sits beyond a band of A/A sets at
    // 1 percent noise (about 2 percent wide), and inside one of sets at 5
    // percent (about 6 percent).
    let b5 = set(0.05, 0.01, 6);
    assert_eq!(
        verdict(&compare(&a, &b5, Some(&aa), &GATE), "work", "wall"),
        "slower"
    );
    let (n, n2) = (set(0.0, 0.05, 7), set(0.0, 0.05, 8));
    let c = compare(&n, &b5, Some(&n2), &GATE);
    assert_eq!(verdict(&c, "work", "wall"), "within noise");
}

#[test]
fn exact_difference_is_always_reported() {
    let c = compare(
        &set(0.0, 0.05, 3),
        &records(0.0, 0.05, 4, 11, false),
        None,
        &GATE,
    );
    let exact: Vec<_> = c
        .exact
        .iter()
        .map(|(p, m, a, b)| (rsprobe_tool::fmt::path(p), *m, *a, *b))
        .collect();
    assert_eq!(exact, [("work".to_string(), "iterations", 10.0, 11.0)]);
    assert!(text(&c).contains("iterations   10 -> 11"));
    // a difference the rounding of a count would hide is written exactly
    let a = records(0.0, 0.05, 3, 4_000_000, false);
    let c = compare(&a, &records(0.0, 0.05, 4, 4_000_001, false), None, &GATE);
    assert!(text(&c).contains("iterations   4000000 -> 4000001"));
    let c = compare(&a, &records(0.0, 0.05, 4, 5_000_000, false), None, &GATE);
    assert!(text(&c).contains("iterations   4.00 M -> 5.00 M"));
}

#[test]
fn difference_within_a_set() {
    let mut a = set(0.0, 0.01, 1);
    let mut r = a[3].record.clone();
    let work = r
        .nodes
        .iter_mut()
        .find(|n| n.values.iter().any(|v| v.0 == "iterations"))
        .unwrap();
    work.values
        .iter_mut()
        .find(|v| v.0 == "iterations")
        .unwrap()
        .1 = 12;
    a[3] = Run::new("03.json", r);
    let c = compare(&a, &set(0.0, 0.01, 2), None, &GATE);
    let nd: Vec<_> = c
        .nondeterministic
        .iter()
        .map(|(p, m, s, v)| (rsprobe_tool::fmt::path(p), *m, *s, v.clone()))
        .collect();
    assert_eq!(
        nd,
        [("work".to_string(), "iterations", "A", vec![10.0, 12.0])]
    );
    assert!(c.exact.is_empty());
    assert!(text(&c).contains("differs within A: 10 12"));
}

#[test]
fn structure_change() {
    let c = compare(
        &set(0.0, 0.01, 1),
        &records(0.0, 0.01, 2, 10, true),
        None,
        &GATE,
    );
    let s: Vec<_> = c
        .structure
        .iter()
        .map(|(p, s)| (rsprobe_tool::fmt::path(p), *s))
        .collect();
    assert_eq!(s, [("new".to_string(), "B")]);
}

#[test]
fn pool_alone_does_not_fail_the_default_gate() {
    // a section whose workers wait longer: pool 30 percent up, wall equal
    let pool = |busy: f64, seed| {
        let mut g = Gauss::new(seed);
        let mut draw = |x: f64| (x * g.draw(1.0, 0.01)).round() as u64;
        (0..ROUNDS)
            .map(|k| {
                let p = [("calls", 1), ("wall", draw(1e6)), ("pool", draw(busy))];
                common::Hand::default().run(
                    &format!("{k:02}.json"),
                    &[(&[], &[("wall", draw(1e6))]), (&["pool"], &p)],
                )
            })
            .collect::<Vec<_>>()
    };
    let (a, b) = (pool(4e6, 1), pool(5.2e6, 2));
    let c = compare(&a, &b, None, &GATE);
    assert_eq!(verdict(&c, "pool", "pool"), "slower");
    assert!(c.gate.is_empty());
    assert!(
        text(&c).contains("gate rule: fails where a node of at least 5% of the run is slower beyond the band in wall;")
    );
    let c = compare(&a, &b, None, &["wall", "pool"]);
    assert_eq!(gated(&c), [("pool".to_string(), "pool")]);
}

#[test]
fn too_few_rounds() {
    let c = compare(
        &set(0.0, 0.01, 1)[..2],
        &set(0.5, 0.01, 2)[..2],
        None,
        &GATE,
    );
    assert_eq!(verdict(&c, "work", "wall"), "too few rounds");
    assert!(c.gate.is_empty());
}
