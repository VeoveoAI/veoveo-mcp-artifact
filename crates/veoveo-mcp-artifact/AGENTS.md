# Artifact MCP Server — Agent Manual

Delta over the repository root `AGENTS.md`. [`DESIGN.md`](DESIGN.md) is the design and
operational contract.

## Purpose

Files for a tenant, with access control, sharing and quotas. MCP carries metadata and
decisions; bytes move between clients and object storage through URLs the server signs.

## Invariants

- Owns the `artifact://` URI scheme and the PostgreSQL schema `artifact`. Identity: slug
  `artifact`, MCP `/artifact/mcp`, share route `/artifact/shares/{token}`, port 8801.
- The `contract` feature builds without sqlx, Tokio or an object-storage client.
  `tests/contract_boundary.rs` reads the resolved dependency graph and fails when one
  appears. Put every new dependency of the server behind the `server` feature.
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
- A verification Task carries its artifact's identifier. Do not queue a second one.
- The storage secret lives in the server's environment, because SigV4 signs with it.
- Every decision appends its audit record through `ArtifactStore::record` on its own
  transaction's connection, before the commit. A new decision declares its detail type
  in `contract/audit.rs`; the server declares the classes at startup.

## Build And Test

- `cargo test -p veoveo-mcp-artifact` with the PostgreSQL 18 server programs and
  `rustfs` on `PATH`, as the `dev` image provides them.
- `cargo test -p veoveo-mcp-artifact --no-default-features --features contract` checks
  the contract alone.
- `tests/database_conformance.rs` runs core's database checks over every statement and
  the `artifact` schema; regenerate `.sqlx` before running it.
- `docker build --target check .` runs the gate.

## Contract Compliance

<!-- veoveo:contract-compliance:start -->
Contract: https://veoveo.ai/contracts/hosted-mcp

- C01: met
- C02: met
- C03: met — no tool runs past its request; checksum verification of a large object runs as a durable Task on the shared Task runtime
- C04: met — keyset pages of at most 100 records filtered by per-artifact access inside each statement; exact reads by key
- C05: met
- C06: met
- C07: met
- C08: met
- C09: met
- C10: met
- C11: met
- C12: met
- C13: met
- C14: met — bytes move only between clients and object storage through signed URLs; the server serves no byte route
- C15: pending — the OCI image builds; the Helm chart is pending
- C16: met
- C17: met
- C18: met
- C19: met
- C20: met
- C21: met
- C22: met
- C23: met
- C24: met
- C25: met
- C26: met
- C27: met
- C28: met
- C29: met — every replica sweeps and verifies; FOR UPDATE SKIP LOCKED and Task claims hand each row and Task to one replica
- C30: met
- C31: pending — installed Discover and list readiness qualification is pending
- C33: pending — Owner naming producers, consumers and installed qualification are pending.
<!-- veoveo:contract-compliance:end -->
