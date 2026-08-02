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

## Rust Run SDK qualification

- Revision: `e4955fe99385c294ac446d53342d8ce5cd90aaad` on macOS arm64.
  The release test binary ran each admission mode in a fresh isolated process;
  each mode used five logical Runs and produced ten internally calibrated
  samples of at least 25 ms. Durability used 1,000 points per Run, three Runs
  per sample, and ten samples.
- Final benchmark command: `SEEX_REPORTING_ADMISSION_ISOLATED=1
  target/release/deps/reporting_performance-2d80308af20be878 <mode> --ignored
  --exact --nocapture`; durability omitted the environment variable. The raw
  artifact is `/tmp/seex-u3-perf-20260802/final-benchmark.txt`, SHA-256
  `92ac5a6a6f3991a48bf7ffa3d96dca4533265e2c77363e64fc4bf0cdba9c0dc1`.
- Explicit single admission measured 4,004,632 point calls/s p50 and 4,512,625
  p95, above the 100,000 calls/s observation target. Implicit single admission
  measured 4,270,065 calls/s p50 and 4,695,544 p95; its five-Run median was
  106.6% of explicit, above the 90% target. Eight-key Mapping admission measured
  8,665,848 points/s p50 and 9,530,848 p95.
- Drain persistence measured 143.640 ms p50 and 161.693 ms p95; finalization
  measured 39.189 ms p50 and 46.795 ms p95. All 30 durability Runs drained and
  finalized, persisting 30,000 admitted points without a queue or terminal
  error.
- Records A-B-B-A command: `python scripts/performance_gate.py run --manifest
  /tmp/seex-u3-perf-20260802/reporting-records.json --output
  /tmp/seex-u3-perf-20260802/reporting-records-result.json`. With no eligible
  public Run SDK rolling baseline after the migration comparison, both roles
  used the qualification revision to measure initial repeatability. The result
  was Inconclusive because explicit admission and finalization exceeded the 2%
  noise limit. SHA-256:
  `9053be3252356a1ce0fa304e5aa2fab966e31162cc54d7a98086f0b21158fe1a`.
- Reporting RSS A-B-B-A used the same revision with manifest `domain` set to
  `reporting`. Peak RSS was 264,798,208 bytes p50 and 273,891,328 p95; warm RSS
  was 191,987,712 bytes p50 and final RSS was 228,483,072 bytes p50. The result
  was Inconclusive because cross-process peak RSS exceeded the 2% noise limit.
  SHA-256:
  `1d221376cf6916abe672a0b06d2ce5b3b70ddb0eba6a7c3333e3a98b86b4d567`.
- Both classifications are non-blocking observations. Neither was Pass, so
  `docs/performance-baselines.json` was not advanced.
