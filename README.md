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
| `crates/veoveo-mcp-artifact` | The library and binary. The `contract` feature holds Artifact's public types and typed calls, the `client` feature adds `ArtifactClient` for consumers' uploads and downloads, and the default `server` feature adds the MCP server, persistence, object storage, the verification worker and the sweep |
| `crates/veoveo-mcp-artifact/migrations/` | The `artifact` schema, applied in the server's own PostgreSQL schema at startup |
| `.sqlx/` | sqlx's checked metadata for every query the workspace compiles |
| `Dockerfile` | The `dev` environment, the `check` gate, the release `build` and the `runtime` image |
| `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml` | The pinned toolchain, code rules and dependency rules |

## Using The Contract

A consumer depends on the crate by Git revision with only its `contract` feature, which
builds without a database, runtime or object-storage dependency:

```toml
veoveo-mcp-artifact = { git = "https://github.com/VeoveoAI/veoveo-mcp-artifact", rev = "<revision>", default-features = false, features = ["contract"] }
```

## Using The Client

A server that stores files in Artifact declares uses of Artifact's permissions when it
registers, selects the `client` feature and wraps the client the SDK binds to its
caller:

```rust
let declaration = AuthorizationBuilder::<MediaPermission>::new()
    // ... the server's own tools ...
    .uses(ArtifactPermission::ArtifactsRead)
    .uses(ArtifactPermission::ArtifactsWrite)
    .build(&slug, &scheme)?;
```

In a tool handler, the client acts for the request's caller:

```rust
let artifacts = ArtifactClient::new(context.client()?, http.clone());
let artifact = artifacts
    .upload(name, media_type, UploadBody::bytes(rendered))
    .labels(labels)
    .await?;
```

In a Task, it acts for the caller that created the Task while the Task runs, so an
upload may outlast the request:

```rust
let artifacts = ArtifactClient::new(calls.for_task(&snapshot)?, http.clone());
let artifact = artifacts
    .upload(name, media_type, UploadBody::stream(output.bytes_stream().map_err(std::io::Error::other)))
    .max_size(ObjectSize::new(512 * 1024 * 1024)?)
    .idempotency_key(key)
    .until_verified(Duration::from_secs(300))
    .await?;
let first_kib = artifacts
    .download(artifact.artifact_id)
    .range(0..=1023)
    .await?
    .body
    .collect()
    .await?;
```

A body of at most 16 MiB goes in one PUT with its size and SHA-256 declared. A larger
one streams in parts, at most four 16 MiB parts in memory by default. Give a maximum
size for a body of unknown size, because Artifact otherwise reserves the tenant's
largest object and sizes parts to fit it. A large object resolves `verifying` while
Artifact computes its SHA-256; `until_verified` waits for it through the artifact's
resource updates and resolves available. Artifact records the calling server, and the
Task when the call came from one, in the artifact's provenance and as the `via` of its
audit records.
The consumer reaches the Gateway's `/oauth/token` and `/mcp/{profile}`, and object
storage, on the installation's network.

## Database

The server owns the PostgreSQL schema `artifact`. Every statement is a `.sql` file that
sqlx checks at compile time against the migrated schema, and the statements and tables
follow the server schema rules of veoveo-core's
[database design](https://github.com/VeoveoAI/veoveo-core/blob/af12a7f52e5e7a445bfc4dca7bac853d5c3bebc9/docs/DATABASE.md).
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
storage credential it is configured with. Tests start a private SeaweedFS (`weed mini`) per test process
from the `weed` binary the `dev` image copies out of the pinned SeaweedFS image, with a
fresh bucket for each test, the way they start PostgreSQL.

## Running

The server takes the registration settings every server takes (`VEOVEO_PUBLIC_BASE_URL`,
`VEOVEO_GATEWAY_ISSUER`, `VEOVEO_REGISTRATION_SECRET`, `VEOVEO_KEY_DIRECTORY`) and its
storage settings: `VEOVEO_STORAGE_ENDPOINT`, `VEOVEO_STORAGE_SIGNING_ENDPOINT`,
`VEOVEO_STORAGE_REGION`, `VEOVEO_STORAGE_BUCKET`, `VEOVEO_STORAGE_ACCESS_KEY`,
`VEOVEO_STORAGE_SECRET_KEY` and `VEOVEO_STORAGE_CORS_ORIGINS`. It listens on port 8801
and serves MCP at `{public_base_url}/artifact/mcp`. Share links name
`{origin}/artifact/shares/{token}` under the installation's public origin,
`VEOVEO_PUBLIC_ORIGIN`, such as `https://veoveo.example.com`, or under the public base
URL when the installation has no public origin. That route answers people without an
account, so the installation routes it to the server without the Gateway.

## Gate

```sh
docker build --target check .
```

The `check` stage checks the `.sqlx` metadata against a migrated PostgreSQL 18.6
database, then runs rustfmt, clippy with warnings denied, cargo-deny, cargo-hack on each
feature and the nextest `ci` profile with every feature on. It runs the same way on macOS and Linux.

## Development Environment

```sh
docker build --target dev -t veoveo-mcp-artifact:dev .
```

A local installation from
[veoveo-template-installation](https://github.com/VeoveoAI/veoveo-template-installation)
runs this image, under its own layer, as the Compose service `vv-mcp-artifact`, which
works in the installation's Lore repository at `/sandbox/src`. The environment reaches
the Gateway at `vv-gateway.internal:8080`, PostgreSQL at `vv-postgres.internal:5432` and
the object store at `vv-seaweedfs.internal:9000` on the installation's network. Signed
requests to the object store go there directly, because a proxy that changed a signed
header would void the signature. Run the commands below from the installation's
directory.

### Running Artifact in the environment

Artifact runs inside `vv-mcp-artifact` on port 8801 and registers with the Gateway at
`http://vv-gateway.internal:8080`. The environment holds a placeholder for
`VEOVEO_REGISTRATION_SECRET`, which the installation's edge replaces with the secret on
the registration request. The storage credential cannot work that way, because the
server signs URLs with the secret itself, so the installation pipes it into an
owner-only file, `/sandbox/state/storage.env`, that holds `VEOVEO_STORAGE_ACCESS_KEY` and
`VEOVEO_STORAGE_SECRET_KEY`.

The served binary builds into its own target directory, `/sandbox/target-serve`, apart
from `target/`, where the gate and acceptance builds enable test-only dependency
features, so a test build is never served:

```sh
docker compose exec vv-mcp-artifact bash -lc 'cd /sandbox/src \
  && cargo build --locked --target-dir /sandbox/target-serve --bin veoveo-mcp-artifact \
  && set -a && . /sandbox/state/storage.env && set +a \
  && VEOVEO_PUBLIC_BASE_URL=http://vv-mcp-artifact.internal:8801 \
     VEOVEO_GATEWAY_ISSUER=http://vv-gateway.internal:8080 \
     VEOVEO_KEY_DIRECTORY=/sandbox/artifact/keys \
     VEOVEO_STORAGE_ENDPOINT=http://vv-seaweedfs.internal:9000 \
     exec /sandbox/target-serve/debug/veoveo-mcp-artifact'
```

The command keeps running in its terminal. Artifact prepares its bucket and registers
before it listens, and the Gateway reaches it as
`http://vv-mcp-artifact.internal:8801/artifact/mcp`. Signed URLs name the object store as
`http://vv-seaweedfs.internal:9000` and share links name
`http://vv-mcp-artifact.internal:8801`; a browser on the host follows them once the host
maps those names to `127.0.0.1`. Artifact removes its registration when it receives
SIGTERM or Ctrl-C.

## Acceptance

`tests/acceptance.rs` calls Artifact through the Gateway as the installation's
`acceptance-member` and `acceptance-admin` callers and moves bytes through the URLs
Artifact signs: each caller lists exactly the tools its role may call, and the member
is refused the tools whose permissions carry a risk; a small upload with a declared SHA-256 through one signed PUT, available at
completion, read by the owner and by a member of its Work Context, and fetched as a
range; a gibibyte of unknown size uploaded in parts, verified by the server's Task,
fetched as a range across a part boundary, then deleted and released by the sweep; a
grant listed and revoked; an upload refused by the quota the admin sets, then
admitted once the limits are restored; a released artifact's share link opened through
the share route, redirected to the object store and spent after its one redemption; and
the member's request to administer an artifact, approved by the admin. The tests share the tenant's usage and limits,
so nextest runs them one at a time. The local installation has one tenant, so a read
refused across tenants is covered by the database tests.

The tests are ignored by default. With the installation running and the callers' keys
copied to `/sandbox/acceptance`, run them:

```sh
docker compose exec vv-mcp-artifact bash -lc 'cd /sandbox/src \
  && VEOVEO_ACCEPTANCE_GATEWAY=http://vv-gateway.internal:8080 \
     VEOVEO_ACCEPTANCE_KEY_DIRECTORY=/sandbox/acceptance \
     cargo nextest run --test acceptance --run-ignored only'
```

## License

Apache-2.0. See [LICENSE](LICENSE).
