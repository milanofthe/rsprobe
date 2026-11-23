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

Libraries depend on `rsprobe` without features; the binary, test harness or binding that wants a record turns on the feature `on`. Without it `scope!` and `add!` expand to nothing.

```rust
let ctx = rsprobe::Context::new();
ctx.param("n", n);
let _on = ctx.enter();
{
    let _s = rsprobe::scope!("factor");
    rsprobe::add!("nnz", nnz);
    let _d = rsprobe::scope!(DEBUG, "pivot");
}
println!("{}", ctx.record()); // one JSON document
```

A thread pool takes the current node where a section starts and enters it in each task, so the task's scopes hang under that node and its time counts as `busy` next to the caller's `wall`:

```rust
let h = rsprobe::current();
pool.run(|| {
    let _g = h.enter();
    let _s = rsprobe::scope!("task");
});
```

## License

MIT
