//! report: the correction formula, shares and ratios on records built by
//! hand; medians with their range, exact measures once or flagged, max
//! measures as levels, --sum, --threads and the warnings.

mod common;

use common::{run, Hand};
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
    assert!((a - b).abs() < 1e-9 * b.abs().max(1.0), "{a} != {b}");
}

// inner 10 ns, outer 5 ns, so full 15 ns. A serial solve with iterations
// and a section `par` on the calling thread whose four tasks ran on workers.
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
        inner: 10.0,
        outer: 5.0,
        noisy,
        ..Hand::default()
    }
}

fn nodes() -> Vec<Run> {
    vec![calibrated(vec!["wait"]).run("r.json", NODES)]
}

#[test]
fn correction_and_shares() {
    // By hand, with c = calls - worker_calls:
    // it:    wall 5000 - 10*10         = 4900, self 4900
    // solve: wall 8000 - 2*10 - 10*15  = 7830, self 7830 - 4900 = 2930
    // task:  busy 3500 - 4*10          = 3460, self 3460
    // par:   wall 1000 - 1*10          = 990 (task on workers), self 990
    //        pool 3600 - 4*15          = 3540 (task under its handles)
    // run:   wall 10000 - (2+10+1)*15  = 9805, self 9805 - 7830 - 990 = 985
    // thread time 9805 + 3540 = 13345; cost 17*15 = 255 of 10000 + 3600.
    let t = 13345.0;
    let expected: [(&[&str], [f64; 4]); 5] = [
        (&[], [9805.0, 0.0, 985.0, 9805.0 / t]),
        (&["solve"], [7830.0, 0.0, 2930.0, 7830.0 / t]),
        (&["solve", "it"], [4900.0, 0.0, 4900.0, 4900.0 / t]),
        (&["par"], [990.0, 0.0, 990.0, 990.0 / t]),
        (&["par", "task"], [0.0, 3460.0, 3460.0, 3460.0 / t]),
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
    assert_eq!(row(&rows, &["par"]).v.get("pool"), 3540.0);
    assert_eq!(
        [run.get("wall"), run.get("pool"), run.get("thread")],
        [9805.0, 3540.0, t]
    );
    near(run.get("cost") / run.get("raw"), 255.0 / 13600.0);
    // Scope cost of it: 100 of 5000.
    near(row(&rows, &["solve", "it"]).cost_share, 0.02);
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
    // a noisy measure is a time: its share of the node's, not per unit
    assert_eq!(
        ratios(row(&rows, &["par"])),
        [("wait/time", 198.0 / 990.0), ("pool/wall", 3540.0 / 990.0)]
    );
    let text = report::text(&runs, &[], false);
    assert!(line(&text, "  par").contains("wait 0.000 ms"));
    assert!(line(&text, "node").contains("wall ms    busy ms    pool ms"));
    assert!(line(&text, "run:").contains("thread time 0.013 ms"));
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
    // Scope cost 10*3 of 200 = 15 percent, above the 10 percent mark.
    assert!(line(&report::text(&runs, &[], false), "  tiny").contains("15.0%!"));
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
    // as the crate wrote it before: the handle guards' spans in busy of par
    let mut old = NODES.to_vec();
    old[3] = (&["par"], &[("calls", 1), ("wall", 1000), ("busy", 3600)]);
    old[4] = (&["par", "task"], &[("calls", 4), ("busy", 3500)]);
    let json = common::run(&old).record.to_string();
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
    // The first three words and the rest of the line.
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

// Two sections on the calling thread: `par`, whose four tasks run `outer`
// with `inner` inside it on the worker, and `sec` with 100 tasks of one
// `task` each. inner 10 ns, outer 5 ns.
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
    // By hand; a time a node has none of is not corrected:
    // inner: busy 20000 - 8*10            = 19920, self 19920
    // outer: busy 38000 - 4*10 - 8*15     = 37840, self 37840 - 19920 = 17920
    // par:   wall 3000 - 10               = 2990, self 2990
    //        pool 40000 - (4+8)*15        = 39820
    // task:  busy 30000 - 100*10          = 29000, self 29000
    // sec:   wall 2000 - 10               = 1990, self 1990
    //        pool 31000 - 100*15          = 29500
    // run:   wall 10000 - 2*15            = 9970, self 9970 - 2990 - 1990 = 4990
    //        thread time 9970 + 39820 + 29500 = 79290
    let expected: [(&[&str], [f64; 4]); 6] = [
        (&[], [9970.0, 0.0, 0.0, 4990.0]),
        (&["par"], [2990.0, 0.0, 39820.0, 2990.0]),
        (&["par", "outer"], [0.0, 37840.0, 0.0, 17920.0]),
        (&["par", "outer", "inner"], [0.0, 19920.0, 0.0, 19920.0]),
        (&["sec"], [1990.0, 0.0, 29500.0, 1990.0]),
        (&["sec", "task"], [0.0, 29000.0, 0.0, 29000.0]),
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
        [9970.0, 69320.0, 79290.0]
    );
    near(row(&rows, &["sec", "task"]).share, 29000.0 / 79290.0);
}

#[test]
fn a_node_with_tasks_and_worker_instances_is_marked() {
    // `block` runs on workers inside the tasks of `par` and has tasks of its
    // own: its leaves are taken as run in its tasks.
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
    // block: busy 1500 - 2*10 = 1480, pool 600 - 6*15 = 510, self 1480
    // par:   pool 1600 - 2*15 = 1570 (leaf lies below block's tasks)
    let block = row(&rows, &["par", "block"]);
    assert!(block.mixed && !row(&rows, &["par"]).mixed);
    let got = ["busy", "pool", "self"].map(|m| block.v.get(m));
    assert_eq!(got, [1480.0, 510.0, 1480.0]);
    assert_eq!(row(&rows, &["par"]).v.get("pool"), 1570.0);
    let text = report::text(&runs, &[], false);
    assert!(line(&text, "    block").contains("% ?"));
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
    // sec has tasks, its work is in their pool time: no ns/width, no
    // ns/sections
    assert_eq!(
        ratios(row(&rows, &["sec"])),
        [("pool/wall", 29500.0 / 1990.0)]
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

// Three rounds of one run: wall varies, nnz is exact and equal, iterations
// differ in one round, peak is a level. No scope cost.
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
    // a level has no time per unit
    assert_eq!(
        ratios(r),
        [("ns/nnz", 10.0), ("ns/iterations", 500.0 / 7.0)]
    );
}

#[test]
fn median_text() {
    let out = report::text(&rounds(), &[], false);
    assert!(out.contains("3 records, median over records."));
    let l = line(&out, "  solve");
    for part in [
        "time [0.000..0.001]",
        "nnz=50,",
        "iterations=7..8 differs",
        "peak max 900",
        "50.0%",
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

/// A record of a section `par` whose tasks ran on two workers, with each
/// thread's values: the calling thread at the root and `par`, two workers at
/// `par/task` with unequal busy time.
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
    let rows: Vec<Vec<&str>> = lines[2..]
        .iter()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(
        rows,
        [
            vec![
                "task",
                "1",
                "handle",
                "2",
                "0.000",
                "0.003",
                "0.000",
                "1.526",
                "allocs=4,",
                "peak",
                "max",
                "640"
            ],
            vec![
                "2",
                "handle",
                "1",
                "0.000",
                "0.001",
                "0.000",
                "0.474",
                "allocs=2,",
                "peak",
                "max",
                "320"
            ]
        ]
    );
}

#[test]
fn threads_of_the_tree() {
    let out = report::threads(&[with_threads(true)], None).unwrap();
    let rows: Vec<Vec<&str>> = out
        .lines()
        .skip(2)
        .map(|l| l.split_whitespace().take(3).collect())
        .collect();
    let expected = [
        ["(run)", "0", "context"],
        ["par", "0", "context"],
        ["1", "handle", "0"],
        ["2", "handle", "0"],
        ["task", "1", "handle"],
        ["2", "handle", "1"],
    ];
    assert_eq!(rows, expected);
}

#[test]
fn threads_errors() {
    assert!(report::threads(&[with_threads(false)], None).is_err());
    assert!(report::threads(&[with_threads(true)], Some("nothing")).is_err());
}
