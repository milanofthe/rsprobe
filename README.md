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
    rsprobe::max!("slots", slots);      // a level: the node keeps the largest value
    let _d = rsprobe::scope!(DEBUG, "pivot");
}
println!("{}", ctx.record()); // one JSON document
```

Measures of `add!` and `time!` are summed over calls, threads and records; those of `max!` are merged by maximum, so a level (slots, code size, a peak) does not add up over nested or repeated scopes. The record tags each measure `exact`, `noisy` or `max`.

With the feature `alloc` the binary installs the counting allocator, which adds `bytes`, `allocs` and `freed` to every scope, and `peak`: per thread, the largest live bytes (allocated minus freed) after an allocation inside the scope, minus the live bytes at its entry, so a scope's peak includes those of the scopes inside it; a node keeps the maximum over calls and threads. Without the feature it only forwards:

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

`ctx.record()` merges the threads; `ctx.record_threads()` adds `per_thread`, per thread its index, whether it entered through the context or a handle, and its values by node: the allocations of the calling thread in a scope apart from those of the pool, the busy time of each worker.

A worker keeps the context it entered through a handle until it enters another one or ends, so the next task of the same context writes nothing shared with other threads; a dropped context's tree is freed when the last worker holding it moves on. `rsprobe::active!(DEBUG)` says whether a site at that level would record on this thread, so a caller reads a clock for a `time!` value only then.

Messages are events at the current node, at the level of their macro; the text is formatted only when the site is active and a sink, the stream or the record takes the event. A context is built with a threshold, filters per module and its sinks; scopes and messages of a module switch together. Events at WARN and above go into the record once per site and text, with a count.

```rust
let ctx = rsprobe::Context::builder()
    .threshold(rsprobe::Level::INFO)
    .filter("mylib::numeric", rsprobe::Level::DEBUG)
    .sink(rsprobe::Stderr::default())
    .build();
let _on = ctx.enter();
rsprobe::debug!("pivot growth {:.2e}", g);
rsprobe::progress!(step, steps);
```

Progress events of a call site pass at most once a second (`Builder::progress_interval`), and the closing one (`done == total`) where the run reported before: a loop calls `progress!` every pass, work shorter than the interval reports nothing.

`Stderr` writes one line per event: seconds since the context was built, level, path of the node, text. On a terminal a line is set in the style of its level (ERROR red, WARN yellow, DEBUG and TRACE dim; not with `NO_COLOR` set) and progress is one line rewritten in place.

```
   0.000036 info  transient: 4 steps, 2000 unknowns
   0.000051 debug transient/step/newton/iteration: residual 1.0e-4
   0.000131 warn  transient/step/newton/factor: small pivot 3.2e-14 in column 417
   0.000140 info  transient/step: 3/4
```

The threshold says what is recorded, a sink's level what it is shown: `.sink_at(Level::WARN, Stderr::default())` under a threshold of INFO keeps the scopes, so a warning is shown with its path, without the INFO messages.

Formatting and the sinks run on the emitting thread. Their time is the noisy measure `emit` of the event's node and is taken out of `wall` and `busy` of the scopes open on that thread, so a profile does not change with the amount of output; the trace keeps the spans as the clock gave them. Of the events at WARN and above with the same site and text, the sinks get the first, the tenth, the hundredth and so on, with the count appended (`small pivot (10 times)`); the record has every count.

A host that must not be called from worker threads (a Python interpreter, a wasm host) attaches a `Buffer` and drains it on its own thread:

```rust
let buffer = rsprobe::Buffer::default();
let ctx = rsprobe::Context::builder().sink(buffer.clone()).build();
// ... the solve, on any threads ...
buffer.drain(|event, path| host_log(&event.line(&path).to_string())); // the line of `Stderr`
```

With the feature `log`, `rsprobe::Logger` turns records of the `log` facade into events at the current node, and the sink `rsprobe::LogSink` passes events on to `log`.

A context built with `.stream()` also keeps every scope instance, event and `add!` value with its time; `ctx.trace()` writes them as Chrome trace event JSON, which Perfetto, speedscope and the Firefox Profiler open; each span carries its node and the node's parent:

```rust
let ctx = rsprobe::Context::builder().stream().build();
// ... the solve ...
std::fs::write("solve.trace.json", ctx.trace())?;
```

## Tool

`tool/` reads records and traces: the crate `rsprobe-tool`, without dependencies besides `rsprobe`. Run it as `cargo run -p rsprobe-tool -- <command>`, or install the binary `rsprobe` with `cargo install --path tool`. A set of records is a record file or a directory of them.

The commands are library functions on `rsprobe::Record`, so a program or a test reads its own record in process, without a file:

```rust
let runs = [rsprobe_tool::Run::new("solve", ctx.record())];
println!("{}", rsprobe_tool::report::text(&runs, &[], false));
let before = rsprobe_tool::load_set(&["before/"])?;
let c = rsprobe_tool::compare::compare(&before, &runs, None, &["wall"]);
```

`report RECORD...`: the tree with the scope cost subtracted (`X' = X - calls inner - (scope instances below) (inner + outer)`, from the record's calibration), self time, the share of the run, the estimated scope cost per node (`!` above 10 percent), and per node the time per unit of each exact measure at scopes whose children ran in their domain, each noisy one (`time!`) in ms and as a share of the node's time, each `max!` measure as a level, and busy over wall. Over several records each noisy value is their median with the range `[min..max]`, an exact one is shown once where the records agree and as its range where not, a `max!` one is the largest; `--sum` shows the sums instead. `--ratio A/B` adds measure A over measure B per node (`width/sections`, a predicted time over `wall`); repeatable. The warnings the records collected follow the tree, with their count and node. Sibling scopes are matched by module and name. `--folded` writes folded stacks for a flame graph. `--threads [NODE]` lists, for records taken with `record_threads()`, each thread's values at a node (`pairs/task`) or at every node, with its busy time over the mean of the threads at that node:

```
1.json: 4 threads, recorded values (scope cost not subtracted)
node                         thread via        calls    wall ms    busy ms busy/mean  measures
    task                          1 handle         1      0.000      9.591     0.992  differences=1334000
                                  2 handle         1      0.000      9.662     0.999  differences=1332000
                                  3 handle         1      0.000      9.749     1.008  differences=1334000
```

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

`compare A B [--aa A2]`: exact measures first, every difference between the sets and within a set; then per node and noisy measure (`wall`, `busy` and those added with `time!`) the ratio of the medians with a 95 percent bootstrap interval, against a noise band from the A/A sets when given. Exit code 1 when the `wall` of a node of at least 5 percent of the run is slower beyond the band; `busy` and the `time!` measures get a verdict but do not fail the gate (`busy` also grows where a worker waits), `--gate MEASURE` adds one, repeatable. The footer states the rule.

```
exact measures, A -> B:
  pairs/task                       calls        4 -> 2
noisy measures, median over records (ms), B/A with its 95% bootstrap interval:
  node                                            A          B     B/A          interval  share  verdict
  sort                             wall       0.042      0.045   1.066    [0.855, 1.252]   1.4%  within noise
  pairs                            wall       3.026      5.501   1.818    [1.678, 2.069]  98.0%  slower
gate: fails on (run) wall, pairs wall
gate rule: fails where a node of at least 5% of the run is slower beyond the band in wall; other measures get a verdict only, --gate MEASURE adds one
```

`ab --rounds N [--aa] -- CMD_A :: CMD_B`: runs the commands alternately, each writing its record to the path in `RSPROBE_RECORD` (`examples/workload.rs` shows the program side), then compares; `--aa` runs A a second time per round for the noise band. The order of the arms rotates per round, in blocks that put each arm once in each position, drawn with a fixed seed, and is printed per round, so a drift over the run or a warm cache does not fall on one arm. `--gate` as for `compare`. `--case NAME=ARGS`, repeated, appends ARGS to both commands per configuration and prints one table across them: the run's wall time, its verdict, the exact differences and the gate.

```
round 3/3 order a b aa
round 3/3 a  0.005 s
round 3/3 b  0.008 s
round 3/3 aa 0.005 s
records in /tmp/rsprobe-ab-x1y2
```

`scale RECORD... --by PARAM`: records over a numeric run parameter, rounds of one value reduced to their median. Per node a least-squares fit of `c n^p log^q(n)` (q in 0, 1) in log space to the time, the calls and each exact measure, with the residual; the scopes whose local exponent at the largest n departs by more than 0.1 from that of the node with the largest self time are named. Fewer than three values are refused. With `--threads` the parameter is a thread count: speedup, efficiency, Karp-Flatt serial fraction and waiting share per node.

`scale RECORD... --by-measure NAME`: for runs that differ in kind (other circuits, other matrices), per node the same fit of its time and its other measures against its own measure NAME (ops, nonzeros), one point per record that has NAME at the node; nodes without it or with fewer than three values are listed as skipped. A node whose time has a residual above 0.25 is named: there NAME is not the unit of its work.

```
10 records, per node against its own differences: c x^p log^q(x) by least squares in log space over the records that have it, residual the rms of ln y.
  node                             measure      points          c model             residual
  pairs/task                       busy             10       1.24 x^0.93 log x         0.052
skipped: without differences (run), assemble, sort, solve, solve/iteration, pairs; fewer than 3 values none
time not following differences, residual above 0.25: none
```

`critical TRACE [--node PATH] [--depth N]`: the chain of spans that bounds a scope instance's elapsed time, from a trace (`ctx.trace()` of a context built with `.stream()`), for a phase whose children run on a pool. For the longest instance of PATH (default: of each top-level scope), from its end back: the span of a child scope inside it, on any thread, that ends last at or before the current time joins the chain, and the walk continues from its start; time between chain elements is a gap (waiting or unrecorded). Each element with spans inside it is followed the same way, to depth N (default 2). Per element its start, duration, thread and share of the enclosing element's time, then per node the time on the chain, a gap counted to the element it lies in. Child spans are matched to the instance by node and time.

```
pairs: the instance on thread 0 at 0.827 ms, elapsed 9.652 ms; its chain to depth 2, share of the enclosing element's time:
  element                               start ms   time ms  share thread
  (gap)                                    0.000     0.182   1.9%
  task                                     0.182     9.448  97.9%      4
  (gap)                                    9.630     0.022   0.2%
  per node over the chain (a gap counts to the element it lies in):
  pairs/task                                         9.448  97.9%
  pairs                                              0.204   2.1%
```

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
