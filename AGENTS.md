# Agent Instructions

These rules apply to veoveo-mcp-artifact.

- The design is in [crates/veoveo-mcp-artifact/DESIGN.md](crates/veoveo-mcp-artifact/DESIGN.md). Change it in the
  same change as the code.
- Use upstream runtimes such as PostgreSQL unmodified. Adapt through
  supported configuration.
- Prefer strong types: typed structs, enums and domain identifiers over raw strings and
  JSON wherever the shape is known.
- The server owns one PostgreSQL schema named after its slug and applies the crate's
  `migrations/` there at startup. Never edit an applied migration; add a new one.
- Every SQL statement is a `.sql` file beside the code that runs it, run through sqlx's
  checked `query_file*!` macros. Builds read the committed `.sqlx` metadata, because
  `.cargo/config.toml` sets `SQLX_OFFLINE`. After changing a query or a migration,
  regenerate the metadata from an empty target directory.
- SQL names only the `artifact` schema. Core state, such as Tasks, process instances,
  usage and audit, comes only through the SDK and core's runtime API, and a missing API
  is reported to its owner rather than worked around in SQL.
  `crates/veoveo-mcp-artifact/tests/sql_confinement.rs` checks every query, migration
  and test query with the SDK's `assert_sql_confined`.
- Statements and tables follow the server schema rules of veoveo-core's
  [database design](https://github.com/VeoveoAI/veoveo-core/blob/9a6b3ff95469ff19f9ccb513421a57ba6751aa9e/docs/DATABASE.md),
  which `crates/veoveo-mcp-artifact/tests/database_conformance.rs` checks. A statement binds its tenant, bounds its
  rows and reaches them through an index in a generic plan. A table has a primary key,
  an index whose leading columns are each foreign key's, and a
  `COMMENT ON TABLE … IS 'Retention: …'`. A statement that cannot comply carries a
  `-- veoveo:` marker that states its reason.
- Page a listing with core's `PageSize` and a `KeysetCursor` from a `CursorScope`
  bound to the listing and the tenant, with one statement for the first page and one
  for the pages after a cursor. Do not write a `$cursor IS NULL OR …` predicate, which
  loses the index seek in a cached generic plan.
- Keep test logic in Rust and tool behavior in each tool's own configuration. Do not add
  a task runner or shell scripts that hold test logic.
- Pin exact versions and image digests. When changing a pin, verify the latest stable
  release at its source.
- Run `docker build --target check .` on macOS and on Linux before pushing. The `check`
  and `build` stages keep Cargo's registry, its Git checkouts and the workspace `target/`
  in BuildKit cache mounts, so consecutive checks reuse compiled crates. These caches
  live only in the build host's BuildKit, are built by the operator from our own
  repositories, and are never used by factory builds, which share no writable cache.
- Names, identifiers and formats follow the
  [naming and format conventions](https://github.com/VeoveoAI/veoveo-core/blob/main/docs/CONVENTIONS.md).
  Where a standard governs a name or format, follow it; extend a standard only through
  its own extension mechanism. URIs, paths and identifiers carry no version segment such
  as `/v1`; releases carry versions.
- Commit small, coherent changes. Commits carry no AI or tool attribution.
