//! report: the correction formula, shares and ratios on records built by
//! hand; medians with their range, exact measures once or flagged, max
//! measures as levels, --sum, --threads and the warnings.

mod common;

use common::{constants::ROUND_OFF, run, Hand};
use rsprobe::{Event, Kind, Level, Node, Param, Site, ThreadNodes};
use rsprobe_tool::records::Path;
use rsprobe_tool::report::{self, asked_ratio, Row};
use rsprobe_tool::{parse, Run};

fn names(p: &Path) -> Vec<&str> {
    p.iter().map(|s| s.0).collect()
}

fn row<'a>(rows: &'a [Row], p: &[&str]) -> &'a Row {
    rows.iter().find(|r| names(&r.path) == p).unwrap()
}

fn ratios(r: &Row) -> Vec<(&str, f64)> {
    r.ratios.iter().map(|(k, x)| (k.as_str(), *x)).collect()
}

fn line<'a>(text: &'a str, start: &str) -> &'a str {
    text.lines().find(|l| l.starts_with(start)).unwrap()
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < ROUND_OFF * b.abs().max(1.0), "{a} != {b}");
}

const INNER: f64 = 10.0;
const OUTER: f64 = 5.0;
const FULL: f64 = INNER + OUTER;

const NODES: common::Tree = &[
    (&[], &[("wall", 10000)]),
    (&["solve"], &[("calls", 2), ("wall", 8000)]),
    (
        &["solve", "it"],
        &[("calls", 10), ("wall", 5000), ("iterations", 10)],
    ),
    (
        &["par"],
        &[
            ("calls", 1),
            ("wall", 1000),
            ("pool", 3600),
            ("tasks", 4),
            ("wait", 198),
        ],
    ),
    (
        &["par", "task"],
        &[
            ("calls", 4),
            ("worker_calls", 4),
            ("busy", 3500),
            ("nnz", 700),
        ],
    ),
];

fn calibrated(noisy: Vec<&'static str>) -> Hand {
    Hand {
        inner: INNER,
        outer: OUTER,
        noisy,
        ..Hand::default()
    }
}

fn nodes() -> Vec<Run> {
    vec![calibrated(vec!["wait"]).run("r.json", NODES)]
}

#[test]
fn correction_and_shares() {
    let it = 5000.0 - 10.0 * INNER;
    let solve = 8000.0 - 2.0 * INNER - 10.0 * FULL;
    let task = 3500.0 - 4.0 * INNER;
    let par_wall = 1000.0 - INNER;
    let par_pool = 3600.0 - 4.0 * FULL;
    let run_wall = 10000.0 - (2.0 + 10.0 + 1.0) * FULL;
    let t = run_wall + par_pool;
    let expected: [(&[&str], [f64; 4]); 5] = [
        (
            &[],
            [run_wall, 0.0, run_wall - solve - par_wall, run_wall / t],
        ),
        (&["solve"], [solve, 0.0, solve - it, solve / t]),
        (&["solve", "it"], [it, 0.0, it, it / t]),
        (&["par"], [par_wall, 0.0, par_wall, par_wall / t]),
        (&["par", "task"], [0.0, task, task, task / t]),
    ];
    let (rows, run) = report::rows(&nodes(), &[], true);
    for (p, [wall, busy, own, share]) in expected {
        let r = row(&rows, p);
        assert_eq!(
            [r.v.get("wall"), r.v.get("busy"), r.v.get("self")],
            [wall, busy, own],
            "{p:?}"
        );
        near(r.share, share);
    }
    assert_eq!(row(&rows, &["par"]).v.get("pool"), par_pool);
    assert_eq!(
        [run.get("wall"), run.get("pool"), run.get("thread")],
        [run_wall, par_pool, t]
    );
    let scopes = 2.0 + 10.0 + 1.0 + 4.0;
    near(
        run.get("cost") / run.get("raw"),
        scopes * FULL / (10000.0 + 3600.0),
    );
    near(
        row(&rows, &["solve", "it"]).cost_share,
        10.0 * INNER / 5000.0,
    );
    let order: Vec<Vec<&str>> = rows.iter().map(|r| names(&r.path)).collect();
    assert_eq!(order, expected.map(|e| e.0.to_vec()));
}

#[test]
fn ratios_per_unit_and_share() {
    let runs = nodes();
    let (rows, _) = report::rows(&runs, &[], true);
    assert_eq!(
        ratios(row(&rows, &["solve", "it"])),
        [("ns/iterations", 490.0)]
    );
    assert_eq!(
        ratios(row(&rows, &["par", "task"])),
        [("ns/nnz", 3460.0 / 700.0)]
    );
    assert_eq!(
        ratios(row(&rows, &["par"])),
        [("wait/time", 198.0 / 990.0), ("pool/wall", 3540.0 / 990.0)],
        "a noisy measure is a time and gets its share of the time of the node"
    );
    let text = report::text(&runs, &[], false);
    assert!(line(&text, "  par").contains("wait 198 ns (20.0 %)"));
    assert!(line(&text, "    it").ends_with("10 iterations, 490 ns each"));
    assert!(line(&text, "node").contains("  wall      busy      pool  "));
    assert!(line(&text, "run:").contains("thread time 13.3 us"));
}

#[test]
fn records_are_summed() {
    let runs: Vec<Run> = (0..3).flat_map(|_| nodes()).collect();
    let (rows, run) = report::rows(&runs, &[], true);
    assert_eq!(run.get("wall"), 3.0 * 9805.0);
    assert_eq!(rows[2].v.get("calls"), 30.0);
    near(rows[2].v.get("self"), 3.0 * 4900.0);
    near(rows[2].share, 4900.0 / 13345.0);
}

#[test]
fn mark_above_the_stated_share() {
    let runs = [Hand {
        inner: 3.0,
        outer: 2.0,
        ..Hand::default()
    }
    .run(
        "t.json",
        &[
            (&[], &[("wall", 1000)]),
            (&["tiny"], &[("calls", 10), ("wall", 200)]),
        ],
    )];
    assert!(
        line(&report::text(&runs, &[], false), "  tiny").starts_with("  tiny ! "),
        "a scope cost of 10 * 3 ns of 200 ns lies above the mark"
    );
}

#[test]
fn root_open_at_record_time() {
    let mut tree = NODES.to_vec();
    tree[0] = (&[], &[("wall", 0)]);
    let (_, run) = report::rows(&[calibrated(vec![]).run("o.json", &tree)], &[], true);
    assert_eq!(run.get("wall"), 7830.0 + 990.0);
}

#[test]
fn folded_and_rows() {
    let runs = nodes();
    let folded = report::folded(&runs);
    assert_eq!(
        folded.lines().collect::<Vec<_>>(),
        [
            "run 985",
            "run;solve 2930",
            "run;solve;it 4900",
            "run;par 990",
            "run;par;task 3460"
        ]
    );
    let (rows, _) = report::rows(&runs, &[], false);
    assert_eq!(
        rows[2].measures().copied().collect::<Vec<_>>(),
        [("iterations", 10.0)]
    );
}

#[test]
fn schema_is_checked_and_the_reader_reads_what_the_crate_writes() {
    let mut r = calibrated(vec!["wait"]).record(NODES);
    r.params = vec![
        ("n".into(), Param::Num(1000.0)),
        ("tol".into(), Param::Num(1e-9)),
        ("case".into(), "a\"\u{e9}".into()),
    ];
    r.per_thread = vec![ThreadNodes {
        index: 0,
        handle: false,
        nodes: r.nodes.clone(),
    }];
    let event = Event {
        node: 1,
        level: Level::WARN,
        module: "m",
        file: "f",
        line: 3,
        time: 5,
        kind: Kind::Message("x".into()),
    };
    r.warnings = vec![(event, 2)];
    let json = r.to_string();
    assert_eq!(parse(&json).unwrap().to_string(), json);
    assert!(parse(&json.replace("\"schema\":3", "\"schema\":4")).is_err());
    assert!(parse(&json.replace("\"schema\":3", "\"schema\":1")).is_err());
}

#[test]
fn schema_2_is_read_and_named() {
    let mut handle_spans_in_busy = NODES.to_vec();
    handle_spans_in_busy[3] = (&["par"], &[("calls", 1), ("wall", 1000), ("busy", 3600)]);
    handle_spans_in_busy[4] = (&["par", "task"], &[("calls", 4), ("busy", 3500)]);
    let json = common::run(&handle_spans_in_busy).record.to_string();
    let run = rsprobe_tool::read("old.json", &json.replace("\"schema\":3", "\"schema\":2"));
    let run = run.unwrap();
    assert_eq!(run.schema, 2);
    let at = |p: &str| run.record.nodes[run.record.find(&[p]).unwrap() as usize].clone();
    assert_eq!(at("par").get("worker_calls"), 0);
    let task = run.record.find(&["par", "task"]).unwrap() as usize;
    assert_eq!(run.record.nodes[task].get("worker_calls"), 4);
    let text = report::text(&[run], &[], false);
    assert!(text.lines().next().unwrap().contains("Schema 2 in 1 of 1"));
}

fn solve_in(module: &'static str) -> Run {
    let mut r = common::Hand::default().record(&[
        (&[], &[("wall", 100)]),
        (&["solve"], &[("calls", 1), ("wall", 60)]),
    ]);
    r.sites[0].1 = Box::leak(Box::new(Site::new("solve", module, "f", 1, Level::INFO)));
    Run::new("s.json", r)
}

#[test]
fn siblings_match_by_module_and_name() {
    let runs = [
        solve_in("lib::direct"),
        solve_in("lib::iterative"),
        solve_in("lib::direct"),
    ];
    let (rows, _) = report::rows(&runs, &[], true);
    let mut solves: Vec<(&str, f64)> = rows
        .iter()
        .filter(|r| names(&r.path) == ["solve"])
        .map(|r| (r.path[0].1, r.v.get("calls")))
        .collect();
    solves.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(solves, [("lib::direct", 2.0), ("lib::iterative", 1.0)]);
}

#[test]
fn sites_of_one_module_and_name_are_one_node() {
    let mut r = common::Hand::default().record(&[
        (&[], &[("wall", 100)]),
        (&["solve"], &[("calls", 1), ("wall", 30)]),
    ]);
    r.sites.push((
        7,
        Box::leak(Box::new(Site::new("solve", "m", "f", 9, Level::INFO))),
    ));
    r.nodes.push(Node {
        site: Some(7),
        ..r.nodes[1].clone()
    });
    let (rows, _) = report::rows(&[Run::new("s.json", r)], &[], true);
    let got: Vec<(Vec<&str>, f64)> = rows
        .iter()
        .map(|r| (names(&r.path), r.v.get("calls")))
        .collect();
    assert_eq!(got, [(vec![], 0.0), (vec!["solve"], 2.0)]);
}

#[test]
fn warnings_after_the_tree() {
    let tree: common::Tree = &[
        (&[], &[("wall", 100)]),
        (&["solve", "it"], &[("calls", 1), ("wall", 60)]),
        (&["solve"], &[("calls", 1), ("wall", 80)]),
    ];
    let runs: Vec<Run> = (0..3)
        .map(|k| {
            let mut r = Hand::default().record(tree);
            let e = |node, level, line, text: &str| Event {
                node,
                level,
                module: "m",
                file: "f",
                line,
                time: 5,
                kind: Kind::Message(text.into()),
            };
            r.warnings = vec![
                (e(1, Level::WARN, 3, "pivot small"), 2),
                (e(0, Level::ERROR, 4, "no convergence"), 1),
            ];
            r.warnings_dropped = k;
            Run::new(format!("w{k}.json"), r)
        })
        .collect();
    let text = report::text(&runs, &[], false);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "warnings, summed:")
        .unwrap();
    assert!(at > lines.iter().rposition(|l| l.starts_with("    it")).unwrap());
    let split = |l: &'static str| {
        let mut rest = l.trim_start();
        let mut out: Vec<&str> = (0..3)
            .map(|_| {
                let (w, r) = rest.split_once(' ').unwrap();
                rest = r.trim_start();
                w
            })
            .collect();
        out.push(rest);
        out
    };
    let text: &'static str = text.leak();
    let rows: Vec<Vec<&str>> = text.lines().skip(at + 2).map(split).collect();
    let expected = [
        ["6", "solve", "warn", "pivot small"],
        ["3", "(run)", "error", "no convergence"],
        ["3", "not", "kept,", "beyond the record's bound"],
    ];
    assert_eq!(rows, expected);
    let (w, dropped) = report::warnings(&runs);
    assert_eq!(
        (w[0].0[0].0, w[0].1, w[0].2.as_str(), w[0].3),
        ("solve", "warn", "pivot small", 6)
    );
    assert_eq!(dropped, 3);
}

#[test]
fn the_warning_column_fits_the_longest_node() {
    let tree: common::Tree = &[
        (&[], &[("wall", 100)]),
        (&["transient"], &[("calls", 1)]),
        (&["transient", "step"], &[("calls", 1)]),
        (&["transient", "step", "newton"], &[("calls", 1)]),
        (
            &["transient", "step", "newton", "iteration"],
            &[("calls", 1)],
        ),
        (
            &["transient", "step", "newton", "iteration", "factor"],
            &[("calls", 1)],
        ),
    ];
    let mut r = Hand::default().record(tree);
    let e = Event {
        node: 5,
        level: Level::WARN,
        module: "m",
        file: "f",
        line: 3,
        time: 5,
        kind: Kind::Message("small pivot".into()),
    };
    r.warnings = vec![(e.clone(), 1), (Event { node: 0, ..e }, 1)];
    let text = report::text(&[Run::new("w.json", r)], &[], false);
    let rows: Vec<&str> = text
        .lines()
        .skip_while(|l| *l != "warnings:")
        .skip(1)
        .collect();
    assert_eq!(rows[0].find("level"), rows[1].find("warn"));
    assert_eq!(rows[1].find("warn"), rows[2].find("warn"));
    assert!(rows[1].ends_with("transient/step/newton/iteration/factor warn   small pivot"));
}

#[test]
fn no_warnings_no_section() {
    assert!(!report::text(&[run(&[(&[], &[("wall", 100)])])], &[], false).contains("warnings"));
}

const WORKERS: common::Tree = &[
    (&[], &[("wall", 10000)]),
    (
        &["par"],
        &[("calls", 1), ("wall", 3000), ("pool", 40000), ("tasks", 4)],
    ),
    (
        &["par", "outer"],
        &[("calls", 4), ("worker_calls", 4), ("busy", 38000)],
    ),
    (
        &["par", "outer", "inner"],
        &[("calls", 8), ("worker_calls", 8), ("busy", 20000)],
    ),
    (
        &["sec"],
        &[
            ("calls", 1),
            ("wall", 2000),
            ("pool", 31000),
            ("tasks", 100),
            ("width", 8),
            ("sections", 2),
        ],
    ),
    (
        &["sec", "task"],
        &[
            ("calls", 100),
            ("worker_calls", 100),
            ("busy", 30000),
            ("nnz", 600),
        ],
    ),
];

fn workers() -> Vec<Run> {
    vec![calibrated(vec![]).run("w.json", WORKERS)]
}

#[test]
fn correction_per_kind_of_thread() {
    let inner = 20000.0 - 8.0 * INNER;
    let outer = 38000.0 - 4.0 * INNER - 8.0 * FULL;
    let par_wall = 3000.0 - INNER;
    let par_pool = 40000.0 - (4.0 + 8.0) * FULL;
    let task = 30000.0 - 100.0 * INNER;
    let sec_wall = 2000.0 - INNER;
    let sec_pool = 31000.0 - 100.0 * FULL;
    let run_wall = 10000.0 - 2.0 * FULL;
    let thread = run_wall + par_pool + sec_pool;
    let expected: [(&[&str], [f64; 4]); 6] = [
        (&[], [run_wall, 0.0, 0.0, run_wall - par_wall - sec_wall]),
        (&["par"], [par_wall, 0.0, par_pool, par_wall]),
        (&["par", "outer"], [0.0, outer, 0.0, outer - inner]),
        (&["par", "outer", "inner"], [0.0, inner, 0.0, inner]),
        (&["sec"], [sec_wall, 0.0, sec_pool, sec_wall]),
        (&["sec", "task"], [0.0, task, 0.0, task]),
    ];
    let (rows, run) = report::rows(&workers(), &[], true);
    for (p, v) in expected {
        let r = row(&rows, p);
        let got = ["wall", "busy", "pool", "self"].map(|m| r.v.get(m));
        assert_eq!(got, v, "{p:?}");
        assert!(!r.mixed);
    }
    assert_eq!(
        [run.get("wall"), run.get("pool"), run.get("thread")],
        [run_wall, par_pool + sec_pool, thread]
    );
    near(row(&rows, &["sec", "task"]).share, task / thread);
}

#[test]
fn a_node_with_tasks_and_worker_instances_is_marked() {
    let tree: common::Tree = &[
        (&[], &[("wall", 1000)]),
        (
            &["par"],
            &[("calls", 1), ("wall", 900), ("pool", 1600), ("tasks", 2)],
        ),
        (
            &["par", "block"],
            &[
                ("calls", 2),
                ("worker_calls", 2),
                ("busy", 1500),
                ("pool", 600),
                ("tasks", 3),
            ],
        ),
        (
            &["par", "block", "leaf"],
            &[("calls", 6), ("worker_calls", 6), ("busy", 1200)],
        ),
    ];
    let runs = [calibrated(vec![]).run("m.json", tree)];
    let (rows, _) = report::rows(&runs, &[], true);
    let block = row(&rows, &["par", "block"]);
    assert!(
        block.mixed && !row(&rows, &["par"]).mixed,
        "block has tasks and worker instances of its own"
    );
    let block_busy = 1500.0 - 2.0 * INNER;
    let got = ["busy", "pool", "self"].map(|m| block.v.get(m));
    assert_eq!(got, [block_busy, 600.0 - 6.0 * FULL, block_busy]);
    assert_eq!(
        row(&rows, &["par"]).v.get("pool"),
        1600.0 - 2.0 * FULL,
        "the leaf lies below the tasks of block, not in the pool of par"
    );
    let text = report::text(&runs, &[], false);
    assert!(line(&text, "    block").starts_with("    block ? "));
    assert!(!line(&text, "  par").contains('?'));
}

#[test]
fn an_open_root_has_no_self_time() {
    let mut tree = WORKERS.to_vec();
    tree[0] = (&[], &[("wall", 0)]);
    let (rows, _) = report::rows(&[calibrated(vec![]).run("o.json", &tree)], &[], true);
    assert_eq!((rows[0].v.get("wall"), rows[0].v.get("self")), (0.0, 0.0));
}

#[test]
fn ns_per_unit_only_where_the_time_is_the_nodes_own() {
    let (rows, _) = report::rows(&workers(), &[], true);
    assert_eq!(
        ratios(row(&rows, &["sec"])),
        [("pool/wall", 29500.0 / 1990.0)],
        "sec has tasks, so its work lies in their pool time and gets no ns per unit"
    );
    assert_eq!(
        ratios(row(&rows, &["sec", "task"])),
        [("ns/nnz", 29000.0 / 600.0)]
    );
}

#[test]
fn asked_ratios() {
    let asked = [
        asked_ratio("width/sections").unwrap(),
        asked_ratio("busy/nnz").unwrap(),
    ];
    let runs = workers();
    let (rows, _) = report::rows(&runs, &asked, true);
    assert_eq!(
        ratios(row(&rows, &["sec"])),
        [("pool/wall", 29500.0 / 1990.0), ("width/sections", 4.0)]
    );
    assert_eq!(
        ratios(row(&rows, &["sec", "task"])),
        [("ns/nnz", 29000.0 / 600.0), ("busy/nnz", 29000.0 / 600.0)]
    );
    assert!(line(&report::text(&runs, &asked, false), "  sec").contains("width/sections 4"));
    assert!(asked_ratio("width").is_err());
}

fn rounds() -> Vec<Run> {
    let hand = Hand {
        maxima: vec!["peak"],
        ..Hand::default()
    };
    let round = |k, w, it, pk| {
        let solve = [
            ("calls", 1),
            ("wall", w),
            ("nnz", 50),
            ("iterations", it),
            ("peak", pk),
        ];
        hand.run(
            &format!("r{k}.json"),
            &[(&[], &[("wall", 1000)]), (&["solve"], &solve)],
        )
    };
    vec![
        round(0, 400, 7, 300),
        round(1, 600, 7, 900),
        round(2, 500, 8, 600),
    ]
}

#[test]
fn medians_with_the_range() {
    let (rows, _) = report::rows(&rounds(), &[], false);
    let r = row(&rows, &["solve"]);
    assert_eq!((r.v.get("wall"), r.v.get("calls")), (500.0, 1.0));
    assert_eq!(
        r.measures().copied().collect::<Vec<_>>(),
        [("nnz", 50.0), ("iterations", 7.0), ("peak", 900.0)]
    );
    let mut spread = r.spread.clone();
    spread.sort_by_key(|s| s.0);
    assert_eq!(
        spread,
        [
            ("iterations", (7.0, 8.0)),
            ("self", (400.0, 600.0)),
            ("time", (400.0, 600.0)),
            ("wall", (400.0, 600.0))
        ]
    );
    near(r.share, 0.5);
    assert_eq!(
        ratios(r),
        [("ns/nnz", 10.0), ("ns/iterations", 500.0 / 7.0)],
        "a level has no time per unit"
    );
}

#[test]
fn median_text() {
    let out = report::text(&rounds(), &[], false);
    assert!(out.contains("3 records, median over records."));
    let l = line(&out, "  solve");
    for part in [
        "time 500 ns [400 ns..600 ns]",
        "50 nnz, 10.0 ns each",
        "7..8 iterations differs, 71.4 ns each",
        "peak max 900 B",
        "50.0 %",
    ] {
        assert!(l.contains(part), "{part} in {l}");
    }
    assert!(!l.contains("ns/peak"));
}

#[test]
fn sum_restores_the_sums() {
    let runs = rounds();
    let (rows, _) = report::rows(&runs, &[], true);
    let r = row(&rows, &["solve"]);
    assert_eq!((r.v.get("wall"), r.v.get("calls")), (1500.0, 3.0));
    assert_eq!(
        r.measures().copied().collect::<Vec<_>>(),
        [("nnz", 150.0), ("iterations", 22.0), ("peak", 900.0)]
    );
    assert!(report::text(&runs, &[], true).contains("3 records, summed."));
    let (rows, _) = report::rows(&runs, &[], false);
    let spread = rows[1].spread.iter().find(|s| s.0 == "iterations");
    assert_eq!(spread.map(|s| s.1), Some((7.0, 8.0)));
}

#[test]
fn max_is_merged_by_maximum() {
    let hand = Hand {
        maxima: vec!["peak"],
        ..Hand::default()
    };
    let rec = |file, peak| {
        hand.run(
            file,
            &[
                (&[], &[("wall", 100)]),
                (
                    &["solve"],
                    &[("calls", 1), ("wall", 50), ("peak", peak), ("nnz", 10)],
                ),
            ],
        )
    };
    let runs = [rec("a.json", 300), rec("b.json", 700)];
    assert_eq!(
        runs[0].kind("max").into_iter().collect::<Vec<_>>(),
        ["peak"]
    );
    let (nodes, _) = report::summary(&runs);
    let solve = nodes.get(&vec![("solve", "m")]).unwrap();
    assert_eq!((solve.get("peak"), solve.get("nnz")), (700.0, 20.0));
}

fn with_threads(threads: bool) -> Run {
    let task = [
        ("calls", 3),
        ("worker_calls", 3),
        ("busy", 3800),
        ("allocs", 6),
        ("peak", 640),
    ];
    let tree: common::Tree = &[
        (&[], &[("wall", 5000)]),
        (
            &["par"],
            &[("calls", 1), ("wall", 4000), ("pool", 4000), ("tasks", 3)],
        ),
        (&["par", "task"], &task),
    ];
    let mut r = Hand {
        maxima: vec!["peak"],
        ..Hand::default()
    }
    .record(tree);
    let thread = |index, handle, values: [&[(&'static str, u64)]; 3]| {
        let nodes = r
            .nodes
            .iter()
            .zip(values)
            .map(|(n, v)| Node {
                values: v.to_vec(),
                ..n.clone()
            })
            .collect();
        ThreadNodes {
            index,
            handle,
            nodes,
        }
    };
    let per_thread = vec![
        thread(
            0,
            false,
            [&[("wall", 5000)], &[("calls", 1), ("wall", 4000)], &[]],
        ),
        thread(
            1,
            true,
            [
                &[],
                &[("pool", 3000), ("tasks", 2)],
                &[
                    ("calls", 2),
                    ("worker_calls", 2),
                    ("busy", 2900),
                    ("allocs", 4),
                    ("peak", 640),
                ],
            ],
        ),
        thread(
            2,
            true,
            [
                &[],
                &[("pool", 1000), ("tasks", 1)],
                &[
                    ("calls", 1),
                    ("worker_calls", 1),
                    ("busy", 900),
                    ("allocs", 2),
                    ("peak", 320),
                ],
            ],
        ),
    ];
    if threads {
        r.per_thread = per_thread;
    }
    Run::new("t.json", r)
}

#[test]
fn threads_at_a_node() {
    let out = report::threads(&[with_threads(true)], Some("par/task")).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].ends_with("3 threads, recorded values (scope cost not subtracted)"));
    assert_eq!(
        lines[1..],
        [
            "node     thread via        calls      wall      busy      pool      load  measures",
            "    task      1 handle         2             2.90 us               1.526  4 allocs, peak max 640 B",
            "              2 handle         1              900 ns               0.474  2 allocs, peak max 320 B",
        ]
    );
}

#[test]
fn threads_of_the_tree() {
    let out = report::threads(&[with_threads(true)], None).unwrap();
    let rows: Vec<Vec<&str>> = out
        .lines()
        .skip(2)
        .map(|l| l.split_whitespace().take(2).collect())
        .collect();
    let expected = [
        ["(run)", "0"],
        ["par", "0"],
        ["1", "handle"],
        ["2", "handle"],
        ["task", "1"],
        ["2", "handle"],
    ];
    assert_eq!(rows, expected);
    assert_eq!(out.matches(" context ").count(), 2);
}

#[test]
fn threads_errors() {
    assert!(report::threads(&[with_threads(false)], None).is_err());
    assert!(report::threads(&[with_threads(true)], Some("nothing")).is_err());
}
