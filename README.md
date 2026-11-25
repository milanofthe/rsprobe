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
- A context per problem instead of process globals. A thread pool hands the current scope to its tasks, so work on a worker is reported under the scope that started it, with the time on the calling thread and the time on workers apart.
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

A thread pool takes the current node where a section starts and enters it in each task, so the task's scopes hang under that node:

```rust
let h = rsprobe::current();
pool.run(|| {
    let _g = h.enter();
    let _s = rsprobe::scope!("task");
});
```

The built-in measures of a node, by the kind of thread they were taken on:

- `calls`: its scope instances on any thread;
- `wall`: the time of its instances on threads entered through `Context::enter`, and at the root the spans of the context's guards;
- `busy`: the time of its instances on threads entered through a handle (the workers);
- `worker_calls`: the part of `calls` on workers;
- `pool`: the spans of the handle guards entered at the node, the time workers spent in tasks under it;
- `tasks`: the number of those spans.

`wall + busy` is the thread time inside the node's own scope instances, without double counting over kinds of thread, and `(wall + busy) / calls` its time per call; at a section's node `pool` is what its workers spent and `pool / wall` the mean number of workers in its tasks. A handle entered where its context is current already (the thread that issued the work runs a task itself) moves the node only: no span, no task. The record (schema 3) writes `worker_calls`, `pool` and `tasks` at a node only where they are not zero.

`ctx.record()` merges the threads; `ctx.record_threads()` adds `per_thread`, per thread its index, whether it entered through the context or a handle, and its values by node: the allocations of the calling thread in a scope apart from those of the pool, the busy and pool time of each worker.

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

Progress events of a call site pass at most once a second (`Builder::progress_interval`), counted from the first event of a run, and the closing one (`done == total`) where the run reported before: a loop calls `progress!` every pass, and a run shorter than the interval reports nothing, however often it repeats. A text follows a semicolon and is formatted only when the event passes; a total of 0 stands for one that is not known: `progress!(k, 0; "residual {:.1e}", r)`.

`Stderr` writes one line per event: the time since the context was built, level, path of the node and, after two spaces, the text. On a terminal a line is set in the style of its level (ERROR red, WARN yellow, DEBUG and TRACE dim; not with `NO_COLOR` set) and progress is one line rewritten in place.

```
  36.0 us  info  transient  4 steps, 2,000 unknowns
  51.0 us  debug transient/step/newton/iteration  residual 1.0e-4
   131 us  warn  transient/step/newton/factor  small pivot 3.2e-14 in column 417
   140 us  info  transient/step  3/4
   1:00.3  info  transient  done
```

`rsprobe::units` writes numbers for a reader, the same in these lines, in the tool's tables and in a caller's own messages (`info!("{} unknowns", units::count(n as f64))`): `time` from ns in `ns`, `us`, `ms`, `s` and from a minute `m:ss.s` (from an hour `h:mm:ss`), `count` exact with a comma between groups of three digits below one million and in `M`, `G` above, `bytes` in `B`, `kB`, `MB`, `GB`, `share` as `23.2 %`. A number has three digits: three significant ones from 1 up and two decimals below 1, in the largest unit in which it is at least 1 (999.5 us is `1.00 ms`). Only what is shown is rounded: the record, the trace and the sinks' events keep integers in ns and raw counts.

The threshold says what is recorded, a sink's level what it is shown: `.sink_at(Level::WARN, Stderr::default())` under a threshold of INFO keeps the scopes, so a warning is shown with its path, without the INFO messages.

Formatting and the sinks run on the emitting thread. Their time is the noisy measure `emit` of the event's node and is taken out of `wall`, `busy` and `pool` of the spans open on that thread, so a profile does not change with the amount of output; the trace keeps the spans as the clock gave them. Of the events at WARN and above with the same site and text, a sink that writes for a reader (`Stderr`, the `log` bridge, the default of the `Sink` trait) gets the first, the tenth, the hundredth and so on, with the count appended (`small pivot (10 times)`); a `Buffer` gets every event, and the record has every count.

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

`report RECORD...`: the tree with `wall`, `busy` and `pool` (a column where any node has a value) with the scope cost subtracted, self time and the share of the run, each time in its unit and an empty cell for zero; `!` after the name of a node whose estimated scope cost is above 10 percent of its time; and per node its measures: `N workers` for `pool/wall`, `COUNT NAME, TIME each` for an exact measure with its time per unit at scopes without tasks, `NAME TIME (SHARE)` for a noisy one (`time!`) with its share of the node's time, `NAME max VALUE` for a `max!` one, a size for the allocator's `bytes`, `freed` and `peak`. The accounting, with `inner` and `full = inner + outer` from the record's calibration:

- a node's time is `wall + busy`; self time is that minus `wall` of its children and, at a node without tasks, minus `busy` of its children; its share is its time over the run's thread time, the root's `wall` plus `pool` of all nodes;
- `wall' = wall - (calls - worker_calls) inner - C full`, with C the instances of its descendants on threads entered through the context: on such a thread every instance below a node lies inside one of its instances;
- `busy' = busy - worker_calls inner - W full` and `pool' = pool - W full`, with W the worker instances of its descendants not separated from it by a node with tasks; W goes to `pool'` at a node with tasks (they ran under its handle guards) and to `busy'` elsewhere. A node with tasks and worker instances of its own is marked `?`: the worker instances below it are taken as run in its tasks;
- a recorded zero stays zero, and the handle guards' own cost is not calibrated.

A record of schema 2 has no split by kind of thread: its `busy` includes the spans of handle guards, so `wall + busy` counts a section twice; the tool reads it, takes `worker_calls` as `calls` where `wall` is 0, and says so in the report's first line. Over several records each noisy value is their median with the range `[min..max]`, an exact one is shown once where the records agree and as its range where not, a `max!` one is the largest; `--sum` shows the sums instead. `--ratio A/B` adds measure A over measure B per node (`width/sections`, a predicted time over `wall`); repeatable. The warnings the records collected follow the tree, with their count and node. Sibling scopes are matched by module and name. `--folded` writes folded stacks for a flame graph, weighted by self time. `--threads [NODE]` lists, for records taken with `record_threads()`, each thread's values at a node (`pairs`) or at every node, with its worker time (`busy + pool`) over the mean of the threads at that node:

```
1.json: 5 threads, recorded values (scope cost not subtracted)
node    thread via        calls      wall      busy      pool      load  measures
  pairs      0 context        1   1.54 ms                                chunk max 500, join 1.41 ms
             1 handle                                 1.34 ms     0.995
             2 handle                                 1.35 ms     1.005
             3 handle                                 1.35 ms     1.002
             4 handle                                 1.34 ms     0.997
```

```
run: wall 1.61 ms, thread time 6.99 ms, scope cost 0.0 %
node             calls      wall      busy      pool      self   share   measures
(run)                    1.61 ms                       5.97 us  23.0 %
  assemble           1   4.31 us                       4.31 us   0.1 %   2,000 nonzeros, 2.15 ns each
  sort               1   45.9 us                       45.9 us   0.7 %
  solve              1   16.0 us                        912 ns   0.2 %
    iteration        5   15.1 us                       15.1 us   0.2 %   5 iterations, 3.01 us each
  pairs              1   1.54 ms             5.39 ms   1.54 ms  22.0 %   3.51 workers, chunk max 500, join 1.41 ms (91.5 %)
    task             4             5.36 ms             5.36 ms  76.6 %   4.00 M differences, 1.34 ns each
```

`compare A B [--aa A2]`: exact measures first, every difference between the sets and within a set; then per node and noisy measure (`wall`, `busy`, `pool` and those added with `time!`) the ratio of the medians with a 95 percent bootstrap interval, against a noise band from the A/A sets when given. Exit code 1 when the `wall` of a node of at least 5 percent of the run's wall time is slower beyond the band; `busy`, `pool` and the `time!` measures get a verdict, with their share of the run's thread time, but do not fail the gate (they also grow where a worker waits), `--gate MEASURE` adds one, repeatable. The footer states the rule.

```
exact measures, A -> B:
  pairs/task                       calls        4 -> 2
noisy measures, median over records, B/A with its 95% bootstrap interval:
  node                                                A          B     B/A          interval   share  verdict
  sort                             wall         42.0 us    45.0 us   1.066    [0.855, 1.252]   1.4 %  within noise
  pairs                            wall         3.03 ms    5.50 ms   1.818    [1.678, 2.069]  98.0 %  slower
gate: fails on (run) wall, pairs wall
gate rule: fails where a node of at least 5% of the run is slower beyond the band in wall; other measures get a verdict only, --gate MEASURE adds one
```

`ab --rounds N [--aa] -- CMD_A :: CMD_B`: runs the commands alternately, each writing its record to the path in `RSPROBE_RECORD` (`examples/workload.rs` shows the program side), then compares; `--aa` runs A a second time per round for the noise band. The order of the arms rotates per round, in blocks that put each arm once in each position, drawn with a fixed seed, and is printed per round, so a drift over the run or a warm cache does not fall on one arm. `--gate` as for `compare`. `--case NAME=ARGS`, repeated, appends ARGS to both commands per configuration and prints one table across them: the run's wall time, its verdict, the exact differences and the gate.

```
round 3/3 order a b aa
round 3/3 a  5.13 ms
round 3/3 b  8.02 ms
round 3/3 aa 5.09 ms
records in /tmp/rsprobe-ab-x1y2
```

`scale RECORD... --by PARAM`: records over a numeric run parameter, rounds of one value reduced to their median. Per node a least-squares fit of `c n^p log^q(n)` (q in 0, 1) in log space to the time (`wall + busy`), the calls and each exact measure, with the residual; the scopes whose local exponent at the largest n departs by more than 0.1 from that of the node with the largest self time are named. Fewer than three values are refused. With `--threads` the parameter is a thread count: speedup, efficiency and Karp-Flatt serial fraction of `wall` per node, and at a node with tasks the waiting share `1 - pool / (threads wall)`.

`scale RECORD... --by-measure NAME`: for runs that differ in kind (other circuits, other matrices), per node the same fit of its time and its other measures against its own measure NAME (ops, nonzeros), one point per record that has NAME at the node; nodes without it or with fewer than three values are listed as skipped. A node whose time has a residual above 0.25 is named: there NAME is not the unit of its work.

```
10 records, per node against its own differences: c x^p log^q(x) by least squares in log space over the records that have it, residual the rms of ln y.
  node                             measure      points          c model             residual
  pairs/task                       time             10       1.24 x^0.93 log x         0.052
skipped: without differences (run), assemble, sort, solve, solve/iteration, pairs; fewer than 3 values none
time not following differences, residual above 0.25: none
```

`critical TRACE [--node PATH] [--depth N]`: the chain of spans that bounds a scope instance's elapsed time, from a trace (`ctx.trace()` of a context built with `.stream()`), for a phase whose children run on a pool. For the longest instance of PATH (default: of each top-level scope), from its end back: the span of a child scope inside it, on any thread, that ends last at or before the current time joins the chain, and the walk continues from its start; time between chain elements is a gap (waiting or unrecorded). Each element with spans inside it is followed the same way, to depth N (default 2). Per element its start, duration, thread and share of the enclosing element's time, then per node the time on the chain, a gap counted to the element it lies in. Child spans are matched to the instance by node and time.

```
pairs: the instance on thread 0 at 827 us, elapsed 9.65 ms; its chain to depth 2, share of the enclosing element's time:
  element                                  start      time   share thread
  (gap)                                     0 ns    182 us   1.9 %
  task                                    182 us   9.45 ms  97.9 %      4
  (gap)                                  9.63 ms   22.0 us   0.2 %
  per node over the chain (a gap counts to the element it lies in):
  pairs/task                                       9.45 ms  97.9 %
  pairs                                             204 us   2.1 %
```

```
15 records over n = 1000 2000 4000 8000 16000: 5 points per fit, c n^p log^q(n) by least squares in log space, residual the rms of ln y.
  node                             measure               c model             residual
  sort                             time               2.43 n^0.94 log n         0.245
  solve/iteration                  time               1.15 n^0.88 log n         0.005
  pairs/task                       time               0.18 n^1.92 log n         0.016
  pairs/task                       differences           1 n^2.00               0.000
dominant: pairs/task, local exponent 2.02 at n = 16000 (largest self time); departing by more than 0.1: assemble (1.10), sort (1.05), solve (0.98), solve/iteration (0.99), pairs (1.85)

  node                             threads       wall  speedup efficiency    serial    wait
  pairs                                  4    13.7 ms     3.30     82.6 %     0.070   6.2 %
  pairs                                  8    9.91 ms     4.57     57.1 %     0.107  22.6 %
```

## License

MIT
