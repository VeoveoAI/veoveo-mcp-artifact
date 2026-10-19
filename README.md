# veoveo-mcp-artifact

The Artifact MCP server stores files for a tenant and decides who may read, change,
share and delete each one. People, agents and other servers reach it through the
Hub over MCP. Bytes move directly between the client and object storage through
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
let routes = Routes::<YourDomain>::builder(RoutePolicy::Authenticated)
    .uses(ArtifactPermission::ArtifactsRead.name())
    .uses(ArtifactPermission::ArtifactsWrite.name());
// Register this server's typed tools and readers, then finish the same table.
let routes = routes.build();
let declaration = routes.authorization();
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
The consumer reaches the Hub's `/oauth/token` and `/mcp/{profile}`, and object
storage, on the installation's network.

## Database

The server owns the PostgreSQL schema `artifact`. Every statement is a `.sql` file that
sqlx checks at compile time against the migrated schema, and the statements and tables
follow the server schema rules of veoveo-core's
[database design](https://github.com/VeoveoAI/veoveo-core/blob/9a6b3ff95469ff19f9ccb513421a57ba6751aa9e/docs/DATABASE.md).
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
`VEOVEO_HUB_ISSUER`, `VEOVEO_REGISTRATION_SECRET`, `VEOVEO_KEY_DIRECTORY`) and its
storage settings: `VEOVEO_STORAGE_ENDPOINT`, `VEOVEO_STORAGE_SIGNING_ENDPOINT`,
`VEOVEO_STORAGE_REGION`, `VEOVEO_STORAGE_BUCKET`, `VEOVEO_STORAGE_ACCESS_KEY`,
`VEOVEO_STORAGE_SECRET_KEY` and `VEOVEO_STORAGE_CORS_ORIGINS`. It listens on port 8801
and serves MCP at `{public_base_url}/artifact/mcp`. Share links name
`{origin}/artifact/shares/{token}` under the installation's public origin,
`VEOVEO_PUBLIC_ORIGIN`, such as `https://veoveo.example.com`, or under the public base
URL when the installation has no public origin. That route answers people without an
account, so the installation routes it to the server without the Hub.

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

The native [installation setup](https://github.com/VeoveoAI/veoveo-template-installation/blob/main/openshell/ARTIFACT.md)
uses the reviewed runtime image in OpenShell workspace `vv-services`, sandbox
`artifact`. It listens privately on 8801, advertises
`http://artifact.internal:8801/artifact/mcp` and registers at the approved native
HTTPS Hub origin. Hub reaches it through its explicit native `ForwardTcp` binding.
No public service exposure or Compose application container is required.

Keep `/var/lib/veoveo/artifact/keys` on the approved persistent named volume, owned
by runtime UID10001. Recover the existing installation key before creating any new
identity. Registration client IDs may change on restart; the persisted signing key,
service slug, registered endpoint and public-key thumbprint must remain stable.
OpenShell supplies registration's opaque reference and workload CA trust. Do not
copy private caller keys or replace TLS verification.

The trusted service receives a dedicated bucket-scoped S3 credential through its
private environment mount. SigV4 needs the real secret inside Artifact; opaque
references cannot sign requests. Both storage endpoints use the same canonical
`http://host.openshell.internal:9000`, region `us-east-1`, bucket `artifacts`, and
path-style addressing. Native TCP policy lets Artifact and authorized clients reach
that endpoint directly without changing signed hosts/headers. Neither signing keys
nor S3 credentials belong in a factory or acceptance runner. Keep
`VEOVEO_PUBLIC_ORIGIN` unset; public share ingress is a separate qualification.

## Acceptance

`tests/acceptance.rs` calls Artifact through Hub as typed member/admin roles. Native
inputs select the installation's distinct member and admin credentials (locally
`vv-hub-member` and `vv-hub-admin`). Explicit typed tenant, admin-principal and
Work Context inputs must match the reviewed installation, rather than values
inferred from returned metadata. A declared upload also
checks the exact admin principal retained as owner/provenance. The six private live
cases cover role/tool refusals, SHA-256 upload and range download, a multipart
upload over a gibibyte verified by Artifact's Task and released by the sweep,
grant listing/revocation, quota enforcement, and access-request approval. They share
one tenant's usage and limits, so run serially. Both callers belong to the same
Work Context: object-level isolation and cross-tenant refusal remain component-test
coverage, not deployed multi-tenant acceptance.

Prepare a trusted runner with the installation's qualification policy and native
dual-credential provider. Set selectors, never token values on command arguments:

```sh
export VEOVEO_ACCEPTANCE_HUB=https://vv-control--hub--http.openshell.localhost:17670
export VEOVEO_ACCEPTANCE_MEMBER_BEARER_ENV=HUB_MEMBER_REFERENCE
export VEOVEO_ACCEPTANCE_ADMIN_BEARER_ENV=HUB_ADMIN_REFERENCE
export VEOVEO_ACCEPTANCE_TENANT=local
export VEOVEO_ACCEPTANCE_ADMIN_PRINCIPAL=https://auth.localhost:8443/realms/veoveo#vv-hub-admin
export VEOVEO_ACCEPTANCE_WORK_CONTEXT=local
/sandbox/bin/artifact-acceptance --ignored --skip restart_persistence_ \
  --skip a_share_link_redeems_through_the_share_route_until_its_limit \
  --nocapture --test-threads=1
```

The three coordinate values above describe the Mac installation. For Entra/reference
qualification, supply its actual tenant, stable admitted admin principal and Work
Context. The Hub profile path remains independent of these coordinates. Missing or
invalid inputs fail before connecting; owner/provenance, quota tenant and retained
Work Context assertions remain exact.

The equivalent source invocation is `cargo nextest run --locked --all-features
--test acceptance --run-ignored only -E 'not (test(restart_persistence_) or
test(a_share_link_redeems_through_the_share_route_until_its_limit))'`.
The retained seventh live case qualifies public share redemption only when that
separate ingress is deliberately configured. It is excluded from native private
acceptance. No test result is inferred from compilation.

Native credential renewal intentionally fences existing connections. It can
interrupt transfers/subscriptions; preserve that evidence and use an explicit fresh
client after an applied refresh. Do not replay mutating operations automatically.
Signed-transfer errors omit signed URLs and signatures.

### Restart persistence

Run the phases separately, outside the ordinary suite, after a known applied
credential refresh with sufficient margin before the next update:

```sh
/sandbox/bin/artifact-acceptance --ignored --exact restart_persistence_prepare \
  --nocapture --test-threads=1
```

It retains a small available object and prints only
`VEOVEO_ACCEPTANCE_RESTART_ARTIFACT_ID` and
`VEOVEO_ACCEPTANCE_RESTART_AVAILABLE_AT`. The trusted operator records the Artifact
key's public thumbprint, sandbox/volume/process identities and native stop/start
readiness for Artifact and Hub, retaining the state volume and keys. Set the two
printed domain inputs in a fresh runner invocation, then run:

```sh
/sandbox/bin/artifact-acceptance --ignored --exact restart_persistence_verify \
  --nocapture --test-threads=1
```

Verification opens a fresh resource subscription, requires its opening update,
checks the same available timestamp, size, digest, name, owner, provenance, Work
Context and access, downloads the exact retained bytes, then deletes the test
artifact. Independently compare the public signing-key thumbprint before/after.
These phases carry no management authority and never restart a workload. They
qualify retained object/metadata state; in-flight verification-Task recovery is not
claimed by this probe.

## License

Apache-2.0. See [LICENSE](LICENSE).
