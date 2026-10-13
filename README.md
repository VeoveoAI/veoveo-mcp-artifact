# veoveo-mcp-artifact

The Artifact MCP server stores files for a tenant and decides who may read, change,
share and delete each one. People, agents and other servers reach it through the
Gateway over MCP. Bytes move directly between the client and object storage through
URLs the server signs, so MCP carries only metadata and decisions.

[`crates/veoveo-mcp-artifact/DESIGN.md`](crates/veoveo-mcp-artifact/DESIGN.md) is the
design and operational contract.

## Contents

| Path | Purpose |
|---|---|
| `crates/veoveo-mcp-artifact` | The library and binary. The `contract` feature holds Artifact's public types; the default `server` feature adds the MCP server, persistence, object storage, the verification worker and the sweep |
| `crates/veoveo-mcp-artifact/migrations/` | The `artifact` schema, applied in the server's own PostgreSQL schema at startup |
| `.sqlx/` | sqlx's checked metadata for every query the workspace compiles |
| `Dockerfile` | The `dev` environment, the `check` gate, the release `build` and the `runtime` image |
| `openshell/policy.yaml` | The OpenShell policy: the Gateway, PostgreSQL and object storage |
| `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml` | The pinned toolchain, code rules and dependency rules |

## Using The Contract

A consumer depends on the crate by Git revision with only its `contract` feature, which
builds without a database, runtime or object-storage dependency:

```toml
veoveo-mcp-artifact = { git = "https://github.com/VeoveoAI/veoveo-mcp-artifact", rev = "<revision>", default-features = false, features = ["contract"] }
```

## Database

The server owns the PostgreSQL schema `artifact`. Every statement is a `.sql` file that
sqlx checks at compile time against the migrated schema, and the statements and tables
follow the server schema rules of veoveo-core's
[database design](https://github.com/VeoveoAI/veoveo-core/blob/7a6c210ff1f9ee9f5150c0a5d101afeb98d659da/docs/DATABASE.md).
Tests need the PostgreSQL 18 server programs on `PATH`; each test clones its own
database from a migrated template.

After changing a query or a migration, regenerate `.sqlx` against a database with the
kernel migrations of the pinned veoveo-core and the crate's migrations applied, from a
target directory with no earlier build:

```sh
CARGO_TARGET_DIR=<empty directory> cargo sqlx prepare --workspace -- --all-targets --all-features
```

## Object Storage

The server signs every request that moves bytes and makes its own calls with the
storage credential it is configured with. Tests start a private RustFS per test process
from the `rustfs` binary the `dev` image copies out of the pinned RustFS image, with a
fresh bucket for each test, the way they start PostgreSQL.

## Running

The server takes the registration settings every server takes (`VEOVEO_PUBLIC_BASE_URL`,
`VEOVEO_GATEWAY_ISSUER`, `VEOVEO_REGISTRATION_SECRET`, `VEOVEO_KEY_DIRECTORY`) and its
storage settings: `VEOVEO_STORAGE_ENDPOINT`, `VEOVEO_STORAGE_SIGNING_ENDPOINT`,
`VEOVEO_STORAGE_REGION`, `VEOVEO_STORAGE_BUCKET`, `VEOVEO_STORAGE_ACCESS_KEY`,
`VEOVEO_STORAGE_SECRET_KEY` and `VEOVEO_STORAGE_CORS_ORIGINS`. It listens on port 8801
and serves MCP at `{public_base_url}/artifact/mcp`.

## Gate

```sh
docker build --target check .
```

The `check` stage checks the `.sqlx` metadata against a migrated PostgreSQL 18.6
database, then runs rustfmt, clippy with warnings denied, cargo-deny, cargo-hack on each
feature and the nextest `ci` profile. It runs the same way on macOS and Linux.

## Development Environment

Build the `dev` image on the host and create an OpenShell sandbox from it:

```sh
docker build --target dev -t veoveo-mcp-artifact:dev .
openshell sandbox create --name vv-mcp-artifact --from veoveo-mcp-artifact:dev \
  --policy openshell/policy.yaml --detach -- sleep infinity
```

The policy lets the server reach the Gateway at `host.openshell.internal:8080`, the
installation's PostgreSQL server at port 5432 and its object store at port 9000, and lets
acceptance tests reach the Gateway and the object store. OpenShell pins each policy-listed executable to the
SHA-256 it first observes, so the server runs from a content-addressed copy under
`/sandbox/run`.

## License

Apache-2.0. See [LICENSE](LICENSE).
