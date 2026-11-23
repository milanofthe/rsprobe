# rsprobe

Logging and profiling for numerical solvers: one small library that every layer of a solver stack instruments itself with, and one record per run that says where the time, the iterations and the memory went.

A solve is instrumented once, in the libraries it runs through. Everything it records hangs on one tree of scopes:

- a **scope** names a phase; the scopes entered inside it are its children;
- a **measure** is an additive quantity at the current scope: wall time, worker time, bytes allocated, and whatever the solver adds (iterations, factorizations, nonzeros);
- an **event** is a message at the current scope.

Every call site carries a level and the context a threshold, the same one for messages and scopes: compiled out when the feature is absent, one atomic load when a site is below the threshold, and finer scopes together with more messages as the threshold rises.

What it holds to:

- No required dependency. It builds for wasm32 without an import; the host supplies the clock.
- No allocation on the measured path and no background thread.
- The cost of a scope is calibrated per run and stored with it, so reported times have it subtracted and its share is known.
- A context per problem instead of process globals. A thread pool hands the current scope to its tasks, so work on a worker is reported under the scope that started it, with wall time and summed worker time apart.
- Logging through the same context: filters per module, sinks for a terminal, a Python caller and a wasm host, collected warnings, progress.
- One structured record per run, and a tool that reads it: a report with the ratios a solver is tuned by (time per nonzero, per op, per iteration), a comparison of two sets of runs (exact counts first, then timings with a confidence interval against an A/A threshold), and a scaling analysis that fits a model per scope over problem size or thread count and names the scope that departs.

## Use

Libraries depend on `rsprobe` without features; the binary, test harness or binding that wants a record turns on the feature `on`. Without it every macro expands to nothing.

```rust
let ctx = rsprobe::Context::new();
ctx.param("n", n);
let _on = ctx.enter();
{
    let _s = rsprobe::scope!("factor");
    rsprobe::add!("nnz", nnz);          // an exact measure: a count
    rsprobe::time!(DEBUG, "wait", ns);  // a noisy one: a time measured by the caller
    let _d = rsprobe::scope!(DEBUG, "pivot");
}
println!("{}", ctx.record()); // one JSON document
```

With the feature `alloc` the binary installs the counting allocator, which adds `bytes`, `allocs` and `freed` to every scope; without the feature it only forwards:

```rust
#[global_allocator]
static A: rsprobe::Alloc = rsprobe::Alloc::new();
```

A thread pool takes the current node where a section starts and enters it in each task, so the task's scopes hang under that node and its time counts as `busy` next to the caller's `wall`:

```rust
let h = rsprobe::current();
pool.run(|| {
    let _g = h.enter();
    let _s = rsprobe::scope!("task");
});
```

Messages are events at the current node, at the level of their macro; the text is formatted only when the site is active. A context is built with a threshold, filters per module and its sinks; scopes and messages of a module switch together. Events at WARN and above go into the record once per site and text, with a count.

```rust
let ctx = rsprobe::Context::builder()
    .threshold(rsprobe::Level::INFO)
    .filter("mylib::numeric", rsprobe::Level::DEBUG)
    .sink(rsprobe::Stderr::default())
    .build();
let _on = ctx.enter();
rsprobe::debug!("pivot growth {:.2e}", g); // "debug factor: pivot growth 1.00e3"
rsprobe::progress!(step, steps);           // one line, rewritten on a terminal
```

A host that must not be called from worker threads (a Python interpreter, a wasm host) attaches a `Buffer` and drains it on its own thread:

```rust
let buffer = rsprobe::Buffer::default();
let ctx = rsprobe::Context::builder().sink(buffer.clone()).build();
// ... the solve, on any threads ...
buffer.drain(|event, path| host_log(event.level.name(), path, &event.kind.to_string()));
```

With the feature `log`, `rsprobe::Logger` turns records of the `log` facade into events at the current node, and the sink `rsprobe::LogSink` passes events on to `log`.

A context built with `.stream()` also keeps every scope instance, event and `add!` value with its time; `ctx.trace()` writes them as Chrome trace event JSON, which Perfetto, speedscope and the Firefox Profiler open:

```rust
let ctx = rsprobe::Context::builder().stream().build();
// ... the solve ...
std::fs::write("solve.trace.json", ctx.trace())?;
```

## Tool

`tool/` reads records: a Python package, standard library only, Python 3.9+. Run it as `python -m rsprobe <command>` from `tool/`, or install it with `pip install ./tool`. A set of records is a record file or a directory of them; several records of a report are summed.

`report RECORD...`: the tree with the scope cost subtracted (`X' = X - calls inner - (scope instances below) (inner + outer)`, from the record's calibration), self time, the share of the run, the estimated scope cost per node (`!` above 10 percent), and per node the time per unit of each exact measure at scopes whose children ran in their domain, each noisy one (`time!`) in ms and as a share of the node's time, and busy over wall. `--ratio A/B` adds measure A over measure B per node (`width/sections`, a predicted time over `wall`); repeatable. The warnings the records collected follow the tree, with their count and node. Sibling scopes are matched by module and name. `--folded` writes folded stacks for a flame graph, `--json` the rows.

```
run: wall 4.653 ms, busy 17.557 ms, scope cost 0.00% of the thread time
node                            calls    wall ms    busy ms    self ms   share  scope  measures
(run)                               0      4.653      0.000      0.004  100.0%   0.0%
  assemble                          1      0.005      0.000      0.005    0.1%   0.5%  nonzeros=4000, ns/nonzeros 1.212
  solve                             1      0.018      0.000      0.000    0.4%   1.6%
    iteration                       5      0.018      0.000      0.018    0.4%   0.7%  iterations=5, ns/iterations 3615
  pairs                             1      4.569     17.557      4.569   98.2%   0.0%  busy/wall 3.843
    task                            4      0.000     17.552     17.552  100.0%   0.0%  differences=16000000, ns/differences 1.097
```

`compare A B [--aa A2]`: exact measures first, every difference between the sets and within a set; then per node and noisy measure (`wall`, `busy` and those added with `time!`) the ratio of the medians with a 95 percent bootstrap interval, against a noise band from the A/A sets when given. Exit code 1 when the `wall` or `busy` of a node of at least 5 percent of the run is slower beyond the band.

```
exact measures, A -> B:
  pairs/task                       calls        4 -> 2
noisy measures, median over records (ms), B/A with its 95% bootstrap interval:
  node                                            A          B     B/A          interval  share  verdict
  sort                             wall       0.042      0.045   1.066    [0.855, 1.252]   1.4%  within noise
  pairs                            wall       3.026      5.501   1.818    [1.678, 2.069]  98.0%  slower
gate: fails on (run) wall, pairs wall
```

`ab --rounds N [--aa] -- CMD_A :: CMD_B`: runs the commands alternately, each writing its record to the path in `RSPROBE_RECORD` (`examples/workload.rs` shows the program side), then compares; `--aa` runs A a second time per round for the noise band. `--case NAME=ARGS`, repeated, appends ARGS to both commands per configuration and prints one table across them: the run's wall time, its verdict, the exact differences and the gate.

```
round 10/10 a  0.005 s
round 10/10 b  0.008 s
round 10/10 aa 0.005 s
records in /tmp/rsprobe-ab-x1y2
```

`scale RECORD... --by PARAM`: records over a numeric run parameter, rounds of one value reduced to their median. Per node a least-squares fit of `c n^p log^q(n)` (q in 0, 1) in log space to the time, the calls and each exact measure, with the residual; the scopes whose local exponent at the largest n departs by more than 0.1 from that of the node with the largest self time are named. Fewer than three values are refused. With `--threads` the parameter is a thread count: speedup, efficiency, Karp-Flatt serial fraction and waiting share per node.

```
15 records over n = 1000 2000 4000 8000 16000: 5 points per fit, c n^p log^q(n) by least squares in log space, residual the rms of ln y.
  node                             measure               c model             residual
  sort                             wall               2.43 n^0.94 log n         0.245
  solve/iteration                  wall               1.15 n^0.88 log n         0.005
  pairs/task                       busy               0.18 n^1.92 log n         0.016
  pairs/task                       differences           1 n^2.00               0.000
dominant: pairs/task, local exponent 2.02 at n = 16000 (largest self time); departing by more than 0.1: assemble (1.10), sort (1.05), solve (0.98), solve/iteration (0.99), pairs (1.85)

  node                             threads    wall ms  speedup efficiency    serial   wait
  pairs                                  4     13.698     3.30      82.6%     0.070   6.2%
  pairs                                  8      9.906     4.57      57.1%     0.107  22.6%
```

## License

MIT
