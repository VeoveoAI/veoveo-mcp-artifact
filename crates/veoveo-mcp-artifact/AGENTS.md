# Artifact MCP Server — Agent Manual

Delta over the repository root `AGENTS.md`. [`DESIGN.md`](DESIGN.md) is the design and
operational contract.

## Purpose

Files for a tenant, with access control, sharing and quotas. MCP carries metadata and
decisions; bytes move between clients and object storage through URLs the server signs.

## Invariants

- Owns the `artifact://` URI scheme and the PostgreSQL schema `artifact`. Identity: slug
  `artifact`, MCP `/artifact/mcp`, share route `/artifact/shares/{token}`, port 8801.
- The `contract` feature builds without sqlx, Tokio or an object-storage client, and
  the `client` feature without sqlx, the S3 SDK, core's store, Task runtime or audit
  writer, or the server's HTTP stack. `tests/contract_boundary.rs` reads the resolved
  dependency graph and fails when one appears. Put every new dependency of the server
  behind the `server` feature.
- Every served tool has one request type that implements the SDK's `ToolCall` in
  `contract/calls.rs`; ready SDK routes bind the typed handler and derive discovery
  and authorization from that association.
- `ArtifactClient` holds at most `parallelism` parts in memory and never the whole body
  beyond one 16 MiB PUT. Its errors never carry a signed URL.
- Owner, tenant, Work Context and provenance come from the forwarded identity. A tool
  argument never names them.
- `artifact.effective_level` decides per-artifact access in SQL. A listing filters on
  it inside the statement, before its row limit. Do not reimplement the decision in Rust.
- Every statement is a `.sql` file under `src/store/` run through sqlx's checked macros.
  Regenerate `.sqlx` from an empty target directory after a query or migration change.
- Every listing is a keyset page with separate first-page and next-page statements and
  one filter at most. Do not write a `$cursor IS NULL OR …` predicate.
- `storage_usage` changes only through the triggers in `migrations/`. A reservation
  locks the tenant's usage row before it checks the quota.
- Row triggers notify the `artifact` channel with each row's key and tenant. Do not
  notify subscribers from handlers.
- Bytes never pass through the server. A tool returns signed requests, and each
  signature binds every header the client sends. Object storage calls stay in
  `src/storage/`, and the store never reads bytes.
- An object's key is `tenants/{tenant}/artifacts/{artifact}`. A failed, aborted or
  expired upload's object is removed when it fails, and the sweep removes it again
  before the row goes.
- Completion makes size-valid bytes available without a content digest or verification
  Task. ArtifactID and create-only storage are identity; consumers own content hashing.
- The storage secret lives in the server's environment, because SigV4 signs with it.
- A share link's secret appears only in the URL `create_share_link` returns. The store
  keeps its SHA-256, records and errors never name it, and the hosted server's request
  span records the route template, never the path. Never log a request path or query.
- Every refused share link answers the same `404`; a refusal never says why. A
  redemption counts in the statement that checks the limit, and its audit record
  commits with it before the route answers.
- Returning an artifact to private revokes its live links in the same transaction. An
  artifact with data labels takes no share link.
- Every decision appends its audit record through `ArtifactStore::record` on its own
  transaction's connection, before the commit. A new decision declares its detail type
  and OCSF activity in `contract/audit.rs`; the server declares the classes at startup.
  The record's target is the Artifact resource the decision concerns, the acting server
  and Task travel in the envelope's `via` rather than the detail, and a failed decision
  names a reason from `ArtifactAuditReason`.

## Build And Test

- `cargo test -p veoveo-mcp-artifact` with the PostgreSQL 18 server programs and
  `weed` (SeaweedFS) on `PATH`, as the `dev` image provides them.
- `cargo test -p veoveo-mcp-artifact --no-default-features --features contract` checks
  the contract alone. `tests/client.rs` needs the `client` feature, so the gate runs
  nextest with `--all-features`.
- `tests/database_conformance.rs` runs core's database checks over every statement and
  the `artifact` schema; regenerate `.sqlx` before running it.
- `docker build --target check .` runs the gate.
