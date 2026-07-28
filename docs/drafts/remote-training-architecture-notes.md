# Remote Training Architecture Notes

> Status: exploratory notes, not an accepted architecture decision or roadmap
> commitment.

## Scenario

Trainers commonly rent compute from a GPU platform only for the duration of a
training job. The training machine is therefore an ephemeral execution
environment rather than the durable home of a Seex project.

The current native mode assumes that the process reporting metrics can also
open the project's local catalog. An S3-compatible `data_path` makes Parquet
metric data remote, but it does not move the catalog database, run lifecycle,
aggregate indexes, DuckLake metadata, or run-writer lock off the training
machine. Releasing that machine can therefore release essential project state
with it.

## Proposed Direction

Separate the ephemeral training node from a stable Seex control service:

```text
ephemeral training node
  Seex SDK
  bounded queue / optional local spool
        |
        | batched HTTPS reports
        v
stable Seex service
  authentication, idempotency, run lifecycle, queries
  DuckDB/DuckLake writer
        |
        +-- catalog: persistent local volume (initially DuckDB or SQLite)
        +-- metric_points: S3-compatible Parquet data path
```

The training node should report domain data to the service. It should not
directly coordinate a shared DuckLake catalog. A service-owned writer avoids
putting shared transactions, distributed locking, catalog credentials, and
DuckLake snapshot coordination on every rented machine.

The initial service can remain a single instance with a persistent catalog
volume and reuse the current native engine. PostgreSQL catalog support,
distributed leases, and multi-instance availability are later concerns, not
requirements for proving the remote-training workflow.

This direction preserves the current product boundary in which catalog state
owns control-plane and query-index data while Parquet remains the durable open
boundary for metric facts. It changes where the native engine runs: the stable
service, rather than the ephemeral training node, owns it.

## Reporting Performance

Remote reporting need not put network latency on the training hot path. A
`run.log(...)` call should enqueue locally and return; a background uploader
should serialize and send batches over a persistent connection:

```text
training thread -> bounded local queue -> batch uploader -> service -> ACK
```

A network request per metric point would perform poorly. Batches of roughly
256 to 2,048 points, with a maximum wait of roughly 50 to 200 milliseconds,
are a reasonable starting range to benchmark. HTTP keep-alive or HTTP/2 should
amortize connection and TLS costs.

For an illustrative encoded size of 100 to 200 bytes per point:

- 100 points/second is approximately 10 to 20 KB/second.
- 1,000 points/second is approximately 0.1 to 0.2 MB/second.
- 10,000 points/second is approximately 1 to 2 MB/second.

The sustained uploader and service throughput must exceed the workload's
average report production rate. Bursts may be absorbed by the bounded queue.
Remote round-trip time primarily affects visibility delay and finalization,
not individual training steps, until an outage or sustained throughput deficit
fills the queue.

Remote-mode verification should measure separately:

- `run.log(...)` admission throughput and p99 latency;
- sustained upload and service persistence throughput;
- report backlog under bursts;
- persisted metric visibility delay;
- final drain and run-finalization time; and
- behavior during disconnection, retry, and queue exhaustion.

## Reliability Semantics

The remote protocol needs explicit states rather than treating a successful
local enqueue as remote durability:

- A queued report is still held by the training client.
- An acknowledged report has been durably accepted by the service.
- Run finalization succeeds only after all reports admitted before the close
  barrier have been acknowledged.

Each batch needs an idempotency identity, such as stable report identifiers or
a client session plus batch sequence. The client must retain unacknowledged
batches and safely retry them without changing effective metric results.

An optional local spool can survive a process restart or a temporary network
failure, but storage on an ephemeral instance does not survive destruction of
that instance. The true safety boundary is therefore service acknowledgment.
Frequent asynchronous upload can keep the potential loss window small without
blocking each training step.

There is an unavoidable trade-off: guaranteeing that an arbitrary immediate
machine termination cannot lose even the latest point requires synchronous
remote durability before training continues. That mode would add network and
storage latency to the hot path. The expected default should instead be
asynchronous batched metrics, while checkpoints and other large critical
artifacts use a separate explicit durability workflow.

During a prolonged outage, the client must not silently discard reports. Once
its bounded queue or spool is full, it needs an explicit policy such as raising
a queue-full error or applying configured backpressure.

## Possible Delivery Phases

1. Add a single-instance Seex service that owns the existing native engine,
   a persistent catalog volume, and an S3-compatible data path.
2. Add a remote client mode for project/run lifecycle, batched reporting,
   finalization, diagnostics, and queries while retaining local native mode.
3. Add idempotent retries, remote drain semantics, bounded offline buffering,
   and preemption-oriented recovery tests.
4. Add authentication, quotas, credential management, and operational backup
   for the service-owned catalog.
5. Consider a PostgreSQL DuckLake catalog, service replicas, and distributed
   run-writer leases only when scale or availability requires them.

## Open Questions

- Must the first version support live observation from another machine, or is
  post-run synchronization sufficient?
- What maximum metric-loss window is acceptable when a rental platform kills
  an instance without a shutdown notice?
- Is the stable service self-hosted by one trainer, provided as a managed
  Seex service, or both?
- Does one remote service initially represent one Seex project root, one
  user, or a future workspace containing multiple projects?
- Are model checkpoints and artifacts in scope, or does this design cover only
  the existing numeric metric model?
- Should a service ACK mean a successful DuckLake append, or should a future
  durable ingestion log become an earlier acceptance boundary?

The answers affect the domain language and may justify a future ADR. Until
they are resolved, terms such as "training node" and "Seex control service"
remain provisional and are not added to the project glossary.
