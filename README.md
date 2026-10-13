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
[database design](https://github.com/VeoveoAI/veoveo-core/blob/f685fbfd3d28a477d35f77cee3feacc8787772ac/docs/DATABASE.md).
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

Build the `dev` image on the host and create an OpenShell sandbox from it with the
installation's `vv-registration` provider attached:

```sh
docker build --target dev -t veoveo-mcp-artifact:dev .
openshell sandbox create --name vv-mcp-artifact --from veoveo-mcp-artifact:dev \
  --policy openshell/policy.yaml --provider vv-registration --detach -- sleep infinity
```

The policy lets the server reach the Gateway at `host.openshell.internal:8080`, the
installation's PostgreSQL server at port 5432 and its object store at port 9000, and lets
acceptance tests reach the Gateway and the object store. Signed requests to the object
store pass through as TCP, because a proxy that changed a signed header would void the
signature.

### Running Artifact in the environment

Artifact runs inside `vv-mcp-artifact` on port 8801 and registers with a Gateway that the
environment reaches at `http://host.openshell.internal:8080`. The `vv-registration`
provider supplies `VEOVEO_REGISTRATION_SECRET` as a placeholder that OpenShell resolves
only on the Gateway endpoint. The storage credential cannot work that way, because the
server signs URLs with the secret itself, so the installation uploads it as an
owner-only file, `/sandbox/state/storage.env`, that holds `VEOVEO_STORAGE_ACCESS_KEY` and
`VEOVEO_STORAGE_SECRET_KEY`.

OpenShell pins every policy-listed executable path to the SHA-256 it first observes and
denies the path once the file changes. Artifact therefore runs from a content-addressed
copy under `/sandbox/run`, so each build gets a fresh path:

```sh
openshell sandbox exec vv-mcp-artifact -- bash -lc 'cd /sandbox/src \
  && cargo build --locked --bin veoveo-mcp-artifact \
  && bin=/sandbox/run/veoveo-mcp-artifact-$(sha256sum target/debug/veoveo-mcp-artifact | cut -c1-16) \
  && install -D -m 0755 target/debug/veoveo-mcp-artifact "$bin" \
  && set -a && . /sandbox/state/storage.env && set +a \
  && VEOVEO_PUBLIC_BASE_URL=http://host.openshell.internal:8801 \
     VEOVEO_GATEWAY_ISSUER=http://host.openshell.internal:8080 \
     VEOVEO_KEY_DIRECTORY=/sandbox/artifact/keys \
     VEOVEO_STORAGE_ENDPOINT=http://host.openshell.internal:9000 \
     "$bin"'
openshell forward start 8801 vv-mcp-artifact -d
```

The first command keeps running in its terminal. Artifact prepares its bucket and
registers before it listens, and the Gateway reaches it as
`http://host.openshell.internal:8801/artifact/mcp`. Signed URLs name the object store as
`http://host.openshell.internal:9000`; a browser on the host resolves that name only
when the host maps it to `127.0.0.1`. Artifact removes its registration when it
receives SIGTERM or Ctrl-C.

## Acceptance

`tests/acceptance.rs` calls Artifact through the Gateway as the installation's
`acceptance-member` and `acceptance-admin` callers and moves bytes through the URLs
Artifact signs: each caller lists exactly the tools its role may call, and the member
is refused the tools whose permissions carry a risk; a small upload with a declared SHA-256 through one signed PUT, available at
completion, read by the owner and by a member of its Work Context, and fetched as a
range; a gibibyte of unknown size uploaded in parts, verified by the server's Task,
fetched as a range across a part boundary, then deleted and released by the sweep; a
grant listed and revoked; and an upload refused by the quota the admin sets, then
admitted once the limits are restored. The tests share the tenant's usage and limits,
so nextest runs them one at a time. The local installation has one tenant, so a read
refused across tenants is covered by the database tests.

The tests are ignored by default. With the installation running and the callers' keys
copied to `/sandbox/acceptance`, run them from a fresh nextest archive extraction, so the
test binaries also get fresh paths:

```sh
openshell sandbox exec vv-mcp-artifact -- bash -lc 'cd /sandbox/src \
  && cargo nextest archive --test acceptance --archive-file /sandbox/run/acceptance.tar.zst \
  && run=/sandbox/run/acceptance-$(date +%s%N) && mkdir -p "$run" \
  && VEOVEO_ACCEPTANCE_GATEWAY=http://host.openshell.internal:8080 \
     VEOVEO_ACCEPTANCE_KEY_DIRECTORY=/sandbox/acceptance \
     cargo nextest run --archive-file /sandbox/run/acceptance.tar.zst \
       --workspace-remap /sandbox/src --extract-to "$run" --run-ignored only'
```

## License

Apache-2.0. See [LICENSE](LICENSE).
