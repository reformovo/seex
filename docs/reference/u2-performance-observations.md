# U2 performance observations

These are non-deciding observations from the first scoped U2 optimization
gates. Both gates followed the fixed v3 A-B-B-A protocol and returned
Inconclusive. Per the performance contract, neither was rerun or expanded, and
neither updates the rolling baseline.

## Narrow Step Reader

- Rolling baseline: `9edd1cd`
- Candidate: `da3a469`
- Fixture: `reader-1x1x1m-v3`, DuckDB Step axis, narrow primary and full
  protected
- Artifact: `/tmp/seex-perf/u2-narrow-da3a469/query-optimization-result.json`
- Artifact SHA-256:
  `7492c0ba1eb9156e8f6827bfa84af56b96544c6305db810e4b9b4b864558eebb`
- Baseline harness patch SHA-256:
  `29ad4e07fd0fe0709e7da5a22216a7a5fe25e710518ac25a1b3b61ca67c0ab9c`

Narrow improved in both order pairs, with a combined improvement of about
28.93%. Full changed by about 1.18%, and the largest full ordered-pair
regression was about 3.02%, within the declared preservation limits. The full
metric nevertheless changed direction between A-B and B-A, so the gate verdict
is Inconclusive.

## Compact dual-View RSS

- Baseline: `20a51dc`
- Candidate build revision: `6cfa378`
- Fixture: `viewer-4x2x100k-dual-v3`
- Artifact:
  `/tmp/seex-perf/u2-final-131c254/viewer-rss-optimization-result.json`
- Artifact SHA-256:
  `878957d687d2416ca44d9cc2990cc4499fab606527cc6fe9081bed31ff2d0a67`
- Baseline peak captures: 470,089,728 and 476,151,808 bytes
- Candidate peak captures: 409,337,856 and 440,336,384 bytes

The combined median peak improved by about 10.2%. Peak and warm RSS varied by
more than the 2% cross-process limit, so the gate verdict is Inconclusive. An
earlier invocation accidentally used a binary without `test-support` and ran
no workload; it is discarded, and the artifact above is authoritative.

Historical U0 Viewer results used different fixtures and workloads. They are
not comparable evidence for the compact gate's target. The earlier U2
single-View seven-pair result remains historical only, and the interrupted
dual-View attempt produced no result file.
