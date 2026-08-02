# U3 Performance Observations

## Mechanical engine migration

- Candidate: `d51e6ffe1a8add300539bb7b4a9202faf6a914b1`; baseline:
  `3e5bb35be73989aaf4c7cb8ce778a696b12c53c9`.
- Fixture: Rust reporting durability, 1,000 reports per Run and ten raw drain
  and finalization samples per capture.
- Command: `python scripts/performance_gate.py run --manifest
  /tmp/seex-u3-perf-20260802/engine-migration.json --output
  /tmp/seex-u3-perf-20260802/engine-migration-durability-result.json`.
- The first combined admission/durability attempt timed out before its first
  capture under the 120-second budget. Its artifact SHA-256 is
  `dba0c14f9eee10adcbfe858dfa291cce20b74b00ce1a56ca1cee05aced1c899a`.
- The narrowed A-B-B-A completed all four processes. The strict record parser
  could not see `rust.drain_persistence` because the Rust test harness placed
  the first record after its `test ...` prefix, so the result is Inconclusive
  with no baseline update. Its artifact SHA-256 is
  `1884bc77826848d0385c57911b1530ffb7f3f5381b0b6b0ef7152b0c3b3adbb3`.
