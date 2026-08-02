---
status: accepted
---

# Use lightweight, target-directed performance validation

Seex separates deterministic Acceptance, measurement-only Benchmark,
rolling-baseline Performance comparison, and manual Profile work. A comparison
runs one bounded A-B-B-A sequence with ten internally calibrated samples per
timing or throughput capture, stops after 120 seconds, and returns Inconclusive
rather than adding processes when noise or order effects prevent a decision.

Acceptance is the only blocking verification type. Performance comparisons
classify observations as Pass, No-change, Regression, or Inconclusive, but do
not automatically reject commits or milestones. The rolling baseline remains
the comparison reference and advances only on Pass. Original revisions and old
stress results remain historical trend evidence because repeatedly rebuilding
and executing large matrices consumed substantially more CPU and time without
improving decisions. Correctness stays in ordinary Acceptance tests, while
Benchmarks, Performance comparisons, and Profiles never block.
