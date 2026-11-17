# Business CI

Two independent workflows report different results:

- **Business correctness** builds all targets, runs the ordinary Rust and Node
  tests, checks formatting and runs Clippy. A green result certifies these app
  checks; explicitly gated hosted/provider tests remain separate operator runs.
- **Business dependency security** runs the unchanged strict cargo-audit command.
  It fails on unsound, yanked or unmaintained dependencies. Its red result stays
  visible independently of correctness; no advisory ignore or allowed failure
  is introduced.

Both run for relevant pull requests and main pushes and can be dispatched
manually. Their concurrency groups are independent, so one cannot cancel the
other. Existing job IDs are retained.

The remaining dependency findings and ownership are recorded in
[the dependency audit](unmaintained-time-dependencies.md). Separating the
workflows does not resolve those findings or certify a clean dependency graph.
