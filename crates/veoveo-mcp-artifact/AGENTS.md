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

## Build And Test

- `cargo test -p veoveo-mcp-artifact` with the PostgreSQL 18 server programs on `PATH`.
- `cargo test -p veoveo-mcp-artifact --no-default-features --features contract` checks
  the contract alone.
- `tests/database_conformance.rs` runs core's database checks over every statement and
  the `artifact` schema; regenerate `.sqlx` before running it.
- `docker build --target check .` runs the gate.
