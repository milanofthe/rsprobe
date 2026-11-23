# rsprobe

Logging and profiling for numerical solvers: one small library that every layer of a solver stack instruments itself with, and one record per run that says where the time, the iterations and the memory went.

A solve is instrumented once, in the libraries it runs through, and read at the level a question needs:

- **off**: the instrumentation is compiled out;
- **summary**: a tree of timed scopes per call path, with the counts a solver is judged by (iterations, evaluations, factorizations, rejected steps) at its nodes, switched at run time at one atomic load per scope;
- **detail**: minimum and maximum per scope, and the memory allocated inside it;
- **trace**: every scope instance as an event in the Chrome trace format, which Perfetto, speedscope and the Firefox Profiler open as they are.

What it holds to:

- No required dependency. It builds for wasm32 without an import; the host supplies the clock.
- No allocation on the measured path and no background thread.
- A context per problem instead of process globals. A thread pool hands the parent scope to its tasks, so work on a worker is reported under the scope that started it, with wall time and summed worker time apart.
- Logging through the same context: levels, filters per module, sinks for a terminal, a Python caller and a wasm host, collected warnings, progress.
- One structured record per run (scope tree, counts, memory, machine, threads, revision), and a tool that compares two sets of runs: the exact differences of the deterministic counts first, then the ratio of the medians over interleaved rounds with its confidence interval, against a threshold taken from an A/A run.

## License

MIT
