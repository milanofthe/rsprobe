//! Tree structure, threads and handles, the threshold and the record.
#![cfg(feature = "on")]

use rsprobe::{add, current, scope, Context, Param};

fn leaf() {
    let _s = scope!("leaf");
    add!("items", 2);
}

#[test]
fn nested_and_repeated_scopes() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        for _ in 0..3 {
            let _a = scope!("a");
            leaf();
            leaf();
        }
        let _b = scope!("b");
        leaf();
        add!("nested", {
            leaf();
            1
        });
    }
    let r = ctx.record();
    let calls = |p: &[&str]| r.nodes[r.find(p).unwrap() as usize].get("calls");
    assert_eq!(r.nodes.len(), 5);
    assert_eq!(calls(&["a"]), 3);
    assert_eq!(calls(&["a", "leaf"]), 6);
    assert_eq!(calls(&["b"]), 1);
    assert_eq!(calls(&["b", "leaf"]), 2);
    assert_eq!(r.nodes[r.find(&["b"]).unwrap() as usize].get("nested"), 1);
    assert_eq!(
        r.nodes[r.find(&["a", "leaf"]).unwrap() as usize].get("items"),
        12
    );
    assert_eq!(r.nodes[0].get("calls"), 0);
    assert_eq!(r.threads, 1);
}

#[test]
fn threads_and_handles_sum_under_their_node() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _p = scope!("par");
        let h = current();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(move || {
                    let _g = h.enter();
                    for _ in 0..10 {
                        leaf();
                    }
                });
            }
        });
        // Inline on the entering thread, as a pool runs a task on its caller.
        let _g = h.enter();
        leaf();
    }
    let r = ctx.record();
    let leaf = &r.nodes[r.find(&["par", "leaf"]).unwrap() as usize];
    assert_eq!(leaf.get("calls"), 41);
    assert_eq!(leaf.get("items"), 82);
    assert_eq!(leaf.get("wall"), 0);
    assert_eq!(r.nodes.len(), 3);
    assert_eq!(r.threads, 5);
}

#[test]
fn below_threshold_records_nothing() {
    let ctx = Context::new();
    {
        let _on = ctx.enter();
        let _s = scope!(DEBUG, "fine");
        add!(TRACE, "steps", 1);
    }
    let r = ctx.record();
    assert_eq!(r.nodes.len(), 1);
    assert_eq!(r.measures.len(), 3);
}

#[test]
fn no_context_records_nothing() {
    let _s = scope!("orphan");
    add!("orphan", 1);
    let _g = current().enter();
    let _t = scope!("orphan");
}

#[test]
fn record_parses_and_round_trips() {
    let ctx = Context::new();
    ctx.param("n", 1000);
    ctx.param("tol", 1e-9);
    ctx.param("case", "a \"quoted\"\nname");
    {
        let _on = ctx.enter();
        let _a = scope!("a");
        add!("big", u64::MAX / 3);
        leaf();
    }
    let r = ctx.record();
    let v: serde_json::Value = serde_json::from_str(&r.to_string()).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["params"]["n"].as_f64(), Some(1000.0));
    assert_eq!(v["params"]["tol"].as_f64(), Some(1e-9));
    assert_eq!(v["params"]["case"], "a \"quoted\"\nname");
    assert_eq!(
        v["calibration"]["inner_ns"].as_f64(),
        Some(r.calibration.inner_ns)
    );
    assert_eq!(
        v["calibration"]["outer_ns"].as_f64(),
        Some(r.calibration.outer_ns)
    );
    assert_eq!(v["threads"], 1);
    assert_eq!(v["measures"]["calls"], "exact");
    assert_eq!(v["measures"]["wall"], "noisy");
    assert_eq!(v["measures"]["items"], "exact");
    let nodes = v["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), r.nodes.len());
    for (i, n) in r.nodes.iter().enumerate() {
        assert_eq!(nodes[i]["parent"].as_u64(), n.parent.map(u64::from));
        assert_eq!(nodes[i]["site"].as_u64(), n.site.map(u64::from));
        for (m, x) in &n.values {
            assert_eq!(nodes[i]["values"][m].as_u64(), Some(*x));
        }
    }
    let sites = v["sites"].as_array().unwrap();
    assert_eq!(sites.len(), 2);
    assert!(sites
        .iter()
        .any(|s| s["name"] == "leaf" && s["level"] == "info"));
    assert_eq!(r.params[2].1, Param::Str("a \"quoted\"\nname".into()));
}

#[test]
fn calibration_is_timed() {
    let c = Context::new().record().calibration;
    assert!(c.inner_ns + c.outer_ns > 0.0, "{c:?}");
}

#[test]
fn a_scope_outliving_its_context_records_nothing() {
    let a = Context::new();
    let on = a.enter();
    let stale = scope!("stale");
    drop(on);
    drop(a);
    let b = Context::new();
    let _on = b.enter();
    drop(stale);
    let r = b.record();
    assert_eq!(r.nodes.len(), 1);
    assert_eq!(r.nodes[0].get("calls"), 0);
}

#[test]
fn guards_dropped_out_of_order_leave_no_context_current() {
    let ctx = Context::new();
    let on = ctx.enter();
    let g = current().enter();
    drop(on);
    drop(g);
    // The node `g` restores belongs to a guard already dropped.
    let s = scope!("after");
    add!("after", 1);
    rsprobe::info!("after");
    let _h = current().enter();
    drop(s);
    let r = ctx.record();
    assert_eq!(r.nodes.len(), 1);
    assert!(r.find(&["after"]).is_none());
}
