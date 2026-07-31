# Unified Performance Validation

## U0 baseline identity

The permanent U0 executable baseline is the clean detached worktree at
`ce0bbf51122070b8a23d17332eeb3667d5fa0ecf`. Original and rolling initially
refer to the same revision. The older Viewer-only schema-v1 record remains in
`viewer-performance-baselines.json` as release history and is not a deciding
U1 baseline.

Raw samples and process RSS series are outside the repository under
`/tmp/seex-perf/u0/`. Their SHA-256 values and stable conclusions are recorded
in `performance-baselines.json`. The source worktree was clean; unrelated user
documentation edits in the primary worktree were excluded by measuring from a
detached worktree and independent release target directory.

## Reporting baseline

The Rust engine and Python/PyO3 workloads cover explicit single-metric,
compatibility implicit single-metric, compatibility eight-metric Mapping,
queue admission, drain/persistence, finalization, and external process RSS.
Compatibility modes use only the shipped explicit-step API; U3 and U4 replace
the adapter beneath the same workload names.

Every correctness check passed: no queue-full failure, zero pending reports
after drain, exactly 1,000 persisted points per durability repeat, and a
successful terminal flush. Python explicit and implicit admission were
reliable across seven processes. Python Mapping relative MAD was 2.0219%, so it
is recorded but non-deciding. Short queue-admission and shutdown phases and the
variable Python finalization phase are also non-deciding.

RSS is an informational original measurement rather than an optimization
verdict. The warm marker precedes lazy DuckDB/DuckLake initialization, so final
RSS legitimately exceeds the Viewer steady-state allowance. Future migration
candidates compare the same named phases rather than treating process startup
as a leak.

Representative commands:

```bash
python scripts/performance_gate.py capture-v2 --runs 1 \
  --output /tmp/seex-perf/u0/reporting-rust-clean.json -- \
  env CARGO_TARGET_DIR=/tmp/seex-perf/targets/u0-original \
  cargo test --release -p seex-core --test reporting_performance \
  -- --ignored --nocapture

python scripts/performance_gate.py capture-v2 --runs 7 \
  --output /tmp/seex-perf/u0/reporting-python-explicit-clean.json -- \
  python scripts/bench_log_throughput.py --mode explicit_single
```

Query, Viewer, ordinary-release instrumentation isolation, and dual-baseline
self-comparison remain open. U1 must not begin until those sections are closed.
