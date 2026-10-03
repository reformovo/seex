# P2 directory discovery evidence

Format v3 adds indexed Project and Run lookup, a global metric key dictionary,
and one directory record for each actual `(Run, metric key)` series. First
append creates a pending record; successful append publishes it. Writer reopen
checks only pending records against facts, publishing rows with facts and
removing rows without facts. Readers do not run repair. Run pages are ordered
by `(created_at, run_id)` descending and capped at 100; global key prefix and
per-Run key pages are also capped at 100. Reader metadata and diagnostic caches
have fixed entry limits.

## Correctness

Rust tests cover v2 rejection, pending repair, duplicate series avoidance,
missing Run references, equal-timestamp cursor pages, key prefix pages, and
Reader bounds. The full workspace `cargo test` suite and Viewer multi-source
pipeline passed after the directory changes. The Viewer loads the first 100
Runs per Project and resolves selected Runs outside that page by identity.
Virtualized navigation through later pages remains in P6.

## Catalog scale probe

The diagnostic catalog was created as a native v3 store, then populated through
SQLite with synthetic **directory-only** rows. It does not replace the P7
Native fact workload. It contains 1,000,001 Runs, 1,000,001 global metric keys,
and 10,000,001 sequence records in a 1.60 GB catalog. Direct SQLite query plans
used `seex_projects_id`, `seex_runs_id`, `seex_runs_project_cursor`, and the
primary-key indexes for exact key and series lookup. Prefix search used the
global key index as a bounded range scan. Ten direct SQLite 100-Run page samples
were 153–168 ms, with 25.6 MB process peak RSS. Catalog generation and these
diagnostic samples took 204 seconds, so they are not a bounded performance
comparison.

The product Reader's 100-Run page probe took 142–149 ms in ten hot samples on
the million-scale catalog, with 108 MB peak RSS. A 200-Run, 200-key, 2,000-series
catalog used 63 MB peak RSS. The 45 MB process increase while the catalog grew
from 0.5 MB to 1.60 GB is evidence that discovery does not copy the full
directory into Reader memory. The probe is
`cargo run -p seex --example p2_catalog_probe -- <store> p2`.

The affected write boundary was compared once using P1 revision `6b951b1`
and the v3 candidate in a bounded A-B-B-A run with ten samples of at least
25 ms per capture. The formal verdict was **inconclusive** because selected
metrics exceeded the noise limit. Baseline/candidate/candidate/baseline
drain medians were 167/176/154/158 ms; finalization medians were
614/706/629/564 ms. No throughput recovery is claimed.

Raw files remain outside the repository at `/tmp/seex-p2-catalog-plan.txt`,
`/tmp/seex-p2-million-directory.json`, `/tmp/seex-p2-reader-page-final.json`,
`/tmp/seex-p2-reader-page-rss.txt`, and
`/tmp/seex-p2-directory-abba-bounded.json`.
