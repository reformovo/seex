---
status: accepted
---

# Use lightweight, target-directed performance validation

Seex separates deterministic Acceptance, measurement-only Benchmark,
rolling-baseline Performance gate, and manual Profile work. A gate runs one
bounded A-B-B-A sequence with ten internally calibrated samples per timing or
throughput capture, stops after 120 seconds, and returns Inconclusive rather
than adding processes when noise or order effects prevent a decision.

Only the rolling baseline can block a candidate. Original revisions and old
stress results remain historical trend evidence because repeatedly rebuilding
and executing large matrices consumed substantially more CPU and time without
improving the decision. Preservation gates protect affected metrics; an
optimization must improve both order pairs and reach its declared primary
target while preserving protected metrics. Correctness stays in ordinary
Acceptance tests, and Profiles never block a commit or milestone.
