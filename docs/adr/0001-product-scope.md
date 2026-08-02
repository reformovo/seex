# ADR 0001: V1 Product Scope

## Status
Accepted.

## Context
The current architecture draft mixes v1 implementation, future Cloud delivery,
and AI Native vision. That makes the project look more mature than it is and
turns speculative features into false requirements.

## Decision
PulseOn v1 targets a minimal native loop for individual trainers who use AI
tools: create/select project, start run, log numeric metrics, query local data,
return chart-ready metric series data, and compare runs. PulseOn v1 does not
provide built-in plotting or plotting dependencies.

AI Native remains the product direction, but v1 only proves that DuckLake can
support the metrics ingestion and local query workload. Agent workflows,
hypotheses, reports, MCP, and Cloud execution are not v1 requirements.

## Consequences
- Keep the v1 API small and local-first.
- Keep project metadata minimal.
- Do not implement workspace, organization, Cloud skeletons, or agent tables in
  v1 code.
- Keep future Cloud and AI Native constraints in separate vision documents.
