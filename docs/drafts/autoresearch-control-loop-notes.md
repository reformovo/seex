# Autoresearch Control Loop Notes

> Status: exploratory notes, not an accepted architecture decision or roadmap commitment.

## Case Study

Seex supported one hardware-local MLX autoresearch session on Apple Silicon:
one baseline and 23 Candidate Runs, with nine kept and 14 discarded. The
accepted sequence reduced validation bits per byte from `1.879073` to
`1.460362` and peak memory from `8.42 GiB` to `4.28 GiB`.

Seex retained per-step curves, allowed concurrent SQLite-backed queries,
preserved evidence for reverted candidates, and exposed deterministic table
and versioned JSON output.

The agent still had to encode Git and hypothesis data in the Run name, parse
output, maintain a Git-tracked TSV ledger, compare, decide, and mutate Git.

## Observed Gaps

### Identity, lineage, and decisions

The Run model describes one execution and its lifecycle, not its accepted
parent, source commit, parameter diff, objective, or research decision. A
normally completed discarded candidate is correctly still a `finished` Run.

Execution status and research decision are different concepts:

```text
execution status = running | finished | failed
research decision = baseline | keep | discard | inconclusive
```

The decision must not be added as another `RunStatus`. A Run can finish
successfully and still be discarded.

### Comparison semantics

`metrics compare` exposes per-Run summaries, but the loop still needs to derive
the incumbent, objective direction, delta, memory trade-off, and recommended
decision. Curve comparison is also ambiguous when Runs have different step
counts. In the case study, changing batch size moved the terminal step from
roughly 240 to roughly 984 under the same wall-time budget.

Seex 0.1.x comparison uses raw step and elapsed wall time. Cumulative-token
and normalized-budget axes are intentionally deferred.

The curve viewer direction in `docs/drafts/gpui-curve-viewer-spike.md` should
own interactive rendering. Autoresearch support should define comparison and
alignment semantics that a CLI, SDK consumer, or viewer can share.

### Rigor and variable budgets

A strict lower-is-better policy accepted the final change from `1.463308` to
`1.460362`, a delta of `0.002946`. The evidence is sufficient for a mechanical
single-Run policy, but not necessarily for a statistical claim. Fixed wall
time also produces different token and step totals as throughput changes.

Seex currently retains the facts needed to investigate these effects, but
does not group repeated configurations, estimate variance, or distinguish a
clear improvement from an inconclusive near tie.

### Diagnostics

Seex stores are created under `.seex`; pre-Seex directories are neither
discovered nor migrated during initialization.

## Working Language

`Run` remains the canonical Seex product term. This draft uses these
autoresearch-specific phrases only as working language:

- **Candidate Run**: a Run evaluating one focused code or parameter change;
- **Incumbent Run**: the accepted Run against which a candidate is compared;
- **Research decision**: the post-execution keep, discard, baseline, or
  inconclusive classification; and
- **Research driver**: the optional controller that edits code, invokes Git,
  starts Runs, and applies a comparison policy.

These terms are not proposed schema names yet. They should not be added to the
glossary until Seex accepts the corresponding product boundary.

## Working Position

Seex Core should remain the source of durable evidence and domain queries.
An optional research driver should own mutations and loop control:

```text
research driver: edit, check, commit, start Run, enforce budget, apply Git action
        |
        v
Seex Core
  lifecycle, metrics, context, lineage, comparisons, diagnostics, JSON evidence
```

Seex Core should not edit source files, commit or revert Git history, or put
decision logic on the metric-reporting hot path. Research context belongs in
catalog application state, not the metric-point Parquet compatibility schema.
Adding durable context or decisions would require an explicit future schema
and product-boundary decision.

## Candidate Capability Surface

### Structured Run context

A future Run creation surface could accept structured context for:

- source identity: Git commit, branch, dirty state, and parent commit;
- lineage plus parameters and the focused diff;
- objective metric, direction, and execution/evaluation budgets; and
- hardware, Seex, MLX, data, and tokenizer fingerprints.

The storage shape is intentionally unresolved. Generic Run metadata, typed
catalog tables, and an external Git-tracked research ledger have different
query and compatibility trade-offs.

### Agent-friendly comparison

An optional autoresearch command surface could build on the existing read
contract:

```bash
seex autoresearch compare <candidate-run> --against <incumbent-run>
seex autoresearch decide <candidate-run> --policy strict-improvement
seex autoresearch leaderboard --metric eval/val_bpb --direction minimize
seex autoresearch best --metric eval/val_bpb --direction minimize
```

Machine output should use the existing versioned JSON envelope and include the
candidate and incumbent Run IDs, objective, absolute and relative delta,
secondary metrics, evidence completeness, confidence state, reasons, and a
recommended action. A recommended Git action is advice only; the command must
not mutate the repository.

### Repetition and significance

Comparison policies should be explicit. A first version may support strict
improvement without claiming significance. A later policy may group repeated
Runs by source and parameter fingerprint, compute uncertainty, and return
`inconclusive` when a near tie requires paired candidate and incumbent reruns.

### Budget and watchdog support

The driver should be able to enforce a maximum wall time, detect missing
heartbeats, and stop on non-finite loss, sustained throughput collapse, or
clear divergence. Seex may expose evidence or stop advice, but the driver
owns process termination. Every stop needs a structured reason retained with
the Run evidence.

### Store diagnostics

A diagnostic surface should be safe and read-only by default:

```bash
seex doctor
seex doctor --verbose
```

It should identify configured and detected backends, conflicting catalog
artifacts, schema or release compatibility, sanitized paths, and a concrete
recovery command. It must remain read-only and never rewrite a store during
initialization.

## Possible Delivery Phases

1. Add read-only comparison reports, objective-aware ranking, and store
   diagnostics using the existing Run and metric read surface.
2. Decide through an ADR whether generic Run context, lineage, and durable
   research decisions belong in Seex catalog application state.
3. Add an optional research driver with strict comparison, budget enforcement,
   Git-action advice, and Git-friendly ledger export.
4. Add repeated-Run grouping, uncertainty-aware decisions, stop advice, and
   search memory only after deterministic policies are validated.
5. Feed shared alignment semantics into the separate curve viewer rather than
   adding plotting dependencies to the Python/Rust SDK.

## Validation Gates

- Reproduce the case study without parsing Run names or manually deriving delta.
- Preserve `RunStatus` lifecycle meaning and the existing Parquet metric schema.
- Keep metric-reporting admission latency and finalization semantics unchanged.
- Compare by step and elapsed time while preserving endpoints.
- Emit deterministic, versioned JSON suitable for an agent to consume without
  scraping human-readable output.
- Return `inconclusive` when a rigor policy lacks enough evidence.
- Diagnose mixed catalogs read-only without modifying them.
- Keep all source and Git mutations outside Seex Core.

## Open Questions

- Is research decision state general or owned by the optional driver?
- Is context generic metadata, typed catalog state, or a Git-tracked ledger?
- Which objective and secondary-metric policies are general enough for Core?
- What repeat/confidence default is honest for fixed-time local training?
- Is stop advice SDK state, a query result, or entirely a driver concern?
- Which comparison semantics belong in the headless read surface versus the
  future curve viewer?
