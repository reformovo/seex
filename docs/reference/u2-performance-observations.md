# U2 performance observations

These are non-blocking observations from the first scoped U2 optimization
comparisons. Both followed the fixed v3 A-B-B-A protocol and returned
Inconclusive. Neither was rerun or expanded, and neither updates the rolling
baseline. The recorded measurements contribute to closing U2 without being an
automated acceptance condition.

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
metric nevertheless changed direction between A-B and B-A, so the comparison
classification is Inconclusive.

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
more than the 2% cross-process limit, so the comparison is Inconclusive. An
earlier invocation accidentally used a binary without `test-support` and ran
no workload; it is discarded, and the artifact above is authoritative.

Historical U0 Viewer results used different fixtures and workloads. They are
not comparable evidence for the compact comparison's target. The earlier U2
single-View seven-pair result remains historical only, and the interrupted
dual-View attempt produced no result file.

## Bounded reducer fallback

A single manual profile of the accepted narrow SQL reported approximately
155,070,992 bytes of DuckDB peak buffer memory. It scanned 1,000,000 Step values
to determine real neighbors, then applied last-write-wins and four bucket
windows to 100,002 rows. The profile artifact is
`/tmp/seex-u2-narrow-profile.json`, SHA-256
`d4f4f86ecba0d14f62eccdb6a2f0c16fe220041dfa1468a4b9d9b0d39a7408b3`.

The bounded candidate kept DuckDB responsible for neighbor selection,
last-write-wins, and one Step order, then retained first/last/min/max per
Step-axis bucket in Rust. Its source-query profile reported 74,145,360 bytes
of peak buffer memory, but transferred all 100,002 effective rows. That profile
is `/tmp/seex-u2-bounded-axis-source-profile.json`, SHA-256
`164b5dde72ca741879d7174161548b4e5e372d690fb5b030510bee91d97fab79`.

The fixed Query A-B-B-A captures are stored at
`/tmp/seex-perf/u2-bounded-axis-candidate/query-optimization-result.json`,
SHA-256
`c82eb0dc6ea51b786f90e3448e9a0630c8f8e5a56b3aa682ae4e8db111febf6e`.
The artifact was initially classified with a transient comparator that also
required protected metrics to have matching directions. Re-evaluating the
same captures under corrected comparator revision `8351c48`, without running
another workload, returns Pass: narrow improves 40.20% and 39.92% in the two
orders, with a 40.06% combined improvement; full changes by approximately
-0.004% and +0.018%, with a 0.0068% combined regression.

The compact Viewer comparison is stored at
`/tmp/seex-perf/u2-bounded-axis-candidate/viewer-rss-result.json`, SHA-256
`6413d5a153f0edb7db3b977d729dd3bb58c32aff689afc2fe134c20ef5c1e11e`.
Baseline peak captures were 479,281,152 and 473,071,616 bytes; candidate peaks
were 443,432,960 and 389,447,680 bytes. The combined observation improves about
12.5%, below the declared 25% target, and candidate variation exceeds 2%, so
the classification is Inconclusive. It was not rerun. The uncommitted bounded
reducer was reverted and the rolling baseline remains `9edd1cd`.

`parquet_metadata` reports nine row groups with disjoint Step ranges covering
0 through 999,999. The bounded scan received the dynamic filter
`step>=449999 AND step<=550000`; the current neighbor-bound scan remained the
only full-series Step scan. This is evidence that Step statistics and
dynamic pruning are already available, not evidence for a physical-ordering or
row-group-setting candidate. No schema or partition change was attempted.
