# Artifact MCP Design

This document is the design and operational contract for the `veoveo-mcp-artifact`
crate.

Artifact stores files for a tenant and decides who may read, change, share and delete
each one. People, agents and other servers ask it through MCP. Bytes move directly
between the client and object storage through URLs the server signs, so the Gateway
and MCP carry only metadata and decisions.

## Service Identity

```text
crate        veoveo-mcp-artifact
binary       veoveo-mcp-artifact
repository   veoveo-mcp-artifact
slug         artifact
URI scheme   artifact
permissions  artifact.*
MCP          /artifact/mcp
share links  /artifact/shares/{token}
liveness     /livez
readiness    /readyz (ready while PostgreSQL and the bucket answer)
port         8801
```

## Standards And Protocols

| Standard or protocol | Implemented profile |
|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification/) | Version `2026-07-28`, JSON-RPC 2.0 over Streamable HTTP, under the hosted MCP contract `https://veoveo.ai/contracts/hosted-mcp` that `veoveo-sdk-rust` implements. The server exposes tools, resources and templates, prompts, completions, subscriptions and typed structured content. |
| MCP Tasks extension `io.modelcontextprotocol/tasks` | Version `2026-07-28`. Checksum verification of a completed upload runs as a Task. |
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/) | Tool inputs and results, resource contents and the audit details Artifact declares at registration. |
| [URI Template RFC 6570](https://www.rfc-editor.org/rfc/rfc6570) | Discovery declares simple variables, and collection pages continue through a `cursor` query parameter. |
| Amazon S3 REST API | The subset in [Object Storage](#object-storage): PutObject, CreateMultipartUpload, UploadPart, ListParts, CompleteMultipartUpload, AbortMultipartUpload, HeadObject, GetObject, DeleteObject, CreateBucket, PutBucketCors and PutBucketLifecycleConfiguration. |
| AWS Signature Version 4, query-string authentication | Signed URLs for PutObject, UploadPart and GetObject, valid for at most 15 minutes. Each URL binds the method, object key and every header the client must send. |
| [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) `If-None-Match: *` | Create-only writes on PutObject and CompleteMultipartUpload. |
| [RFC 6266](https://www.rfc-editor.org/rfc/rfc6266) | Download responses carry an attachment disposition with the artifact's name as `filename*`. |
| [RFC 6838](https://www.rfc-editor.org/rfc/rfc6838) media types | An artifact's media type is a bare `type/subtype` without parameters. |
| SHA-256 | Every available artifact records its SHA-256 in the foundational `Sha256Digest` spelling, `sha256:` followed by 64 lowercase hexadecimal digits. |
| [JWT access tokens RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html) and [Token Exchange RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html) | The Gateway forwards each request with an access token naming the caller, their tenant, groups, data labels and Work Context authority. A call from another server carries an `act` chain naming that server. |
| [Veoveo server registration](https://github.com/VeoveoAI/veoveo-sdk-rust) | Artifact registers through RFC 7591, removes its registration through RFC 7592, and declares its permissions and audit details. |
| [PostgreSQL 18](https://www.postgresql.org/docs/18/) | Server 18.6 through sqlx 0.9.0, under the [installation database design](https://github.com/VeoveoAI/veoveo-core/blob/7a6c210ff1f9ee9f5150c0a5d101afeb98d659da/docs/DATABASE.md) of `veoveo-core`. Artifact owns the schema `artifact`, applies its own migrations and runs every statement from a `.sql` file that sqlx checks. Row triggers publish changes on the `artifact` channel. |
| RustFS 1.0.1 | The installation's object store, pinned by image digest. Artifact uses only the S3 calls listed above, so an S3-compatible store such as Ceph or AWS S3 can replace it. |

## Domain Contract

Artifact answers these questions for a tenant:

- Which files may this caller see, and what does each one hold: name, media type,
  size, SHA-256, labels, owner, Work Context, release state and provenance?
- Where does a caller send the bytes of a new file, and where do they fetch an
  existing one?
- Who else may read, change or administer a file, and until when?
- Which files has this tenant stored, and how much of its quota remains?

An artifact is immutable once available. A changed file is a new artifact whose
provenance names the artifact it derives from.

## Architecture

### Library Features

The crate is a library and a binary.

| Feature | Contains | Depends on |
|---|---|---|
| `contract` | `ArtifactId`, the typed resource addresses, every tool request and result, metadata, grant levels and subjects, access requests, upload limits, the `ArtifactPermission` vocabulary and the audit detail types | `veoveo-types`, the SDK's `contract` feature, Serde and Schemars |
| `client` | `contract` plus `ArtifactClient`, which uploads a stream of known or unknown size and downloads through signed URLs, built on the SDK's `ctx.client()` | the SDK's `client` feature and an HTTP client |
| `server` (default) | the hosted server, persistence and object storage | the SDK's `server` feature, sqlx and aws-sdk-s3 |

A consumer such as Media selects `default-features = false, features = ["client"]` and
compiles neither sqlx nor the S3 SDK. The check stage proves that with `cargo tree`.

### Hosted Process

The binary parses configuration, registers with the Gateway, connects to PostgreSQL
with the login its registration returned, applies its migrations, prepares its bucket
and serves MCP and the share route. One sweep loop runs on every replica and claims
work with `FOR UPDATE SKIP LOCKED`.

## MCP Surface

### Tools

| Tool | Does | Permission | Level on the artifact |
|---|---|---|---|
| `create_upload` | Reserves quota and an artifact ID and returns the URLs that receive the bytes | `artifact.artifacts.write` | contributor or above in the caller's selected Work Context |
| `sign_upload_parts` | Signs the next part URLs of a multipart upload, or signs expired ones again | `artifact.artifacts.write` | the upload's creator |
| `complete_upload` | Completes the upload and makes the artifact available, or starts its checksum Task | `artifact.artifacts.write` | the upload's creator |
| `abort_upload` | Aborts the upload and releases its reservation | `artifact.artifacts.write` | the upload's creator |
| `create_download` | Returns a signed GET URL valid for 5 minutes | `artifact.artifacts.read` | read |
| `grant_artifact`, `revoke_artifact_grant` | Gives or removes read, write or admin for a principal or group, with an optional expiry | `artifact.grants.write` (sensitive) | admin |
| `set_release_state` | Moves an artifact between private and released | `artifact.releases.write` | admin |
| `create_share_link`, `revoke_share_link` | Issues or revokes an anonymous read link for a released artifact | `artifact.share-links.write` (sensitive) | admin |
| `request_artifact_access`, `cancel_access_request` | Asks an artifact's administrators for a level the caller lacks | `artifact.access-requests.write` | the artifact is visible to the caller |
| `decide_access_request` | Approves the request, which writes the grant, or denies it with a note | `artifact.grants.write` (sensitive) | admin |
| `delete_artifact` | Deletes the artifact, its grants and share links, and its object | `artifact.artifacts.delete` (destructive) | admin |
| `set_upload_limits` | Sets a tenant's quota, largest object, allowed media types and active-upload limit | `artifact.upload-limits.administer` (administer) | installation administrator |

`create_upload` takes the name and media type, and optionally the size, the SHA-256,
a maximum size, data labels, a retention period, metadata, the artifacts it derives
from and an idempotency key. The owner, tenant, Work Context and provenance come from
the forwarded identity, never from arguments: the artifact belongs to the caller's
selected Work Context. Its labels are the requested labels together with the Work
Context's output classification and labels, and all of them must lie within the
caller's data labels.

### Resources

| URI | Returns |
|---|---|
| `artifact://artifacts{?work_context,owner,media_type,name,released,cursor}` | A page of artifacts the caller may read, newest first |
| `artifact://artifacts/{artifact_id}` | The artifact's metadata |
| `artifact://artifacts/{artifact_id}/grants` | Its grants, for administrators |
| `artifact://artifacts/{artifact_id}/share-links{?cursor}` | Its share links without their secrets, for administrators |
| `artifact://uploads{?state,cursor}`, `artifact://uploads/{upload_id}` | The caller's uploads and their state |
| `artifact://access-requests{?role,state,cursor}` | Requests the caller made, or can decide |
| `artifact://storage-usage` | The tenant's stored and reserved bytes against its quota |

Every resource accepts subscriptions. The prompts `review-artifact-sharing` and
`review-access-requests` guide an administrator through those two resources.

### Share Route

`GET /artifact/shares/{token}` is the only route outside MCP, because a share link must
work for someone without an account. The server hashes the token, finds a live link
to a released artifact, counts one redemption and answers `303 See Other` with a fresh
signed GET.

## Object Storage

Artifact holds one bucket per installation and one object per artifact, at
`tenants/{tenant_id}/artifacts/{artifact_id}`. The server's storage credential is
limited to that bucket and never leaves the server.

### Uploads

`create_upload` chooses the upload's form:

| The caller declares | Form | The server checks |
|---|---|---|
| a size up to 100 MiB, with or without a SHA-256 | one signed PutObject | RustFS refuses a different size, or a different SHA-256 when one was declared, while it writes |
| a larger size, or no size | multipart, with a part size the server chooses | each part's size, and each part's SHA-256 when the client sends one |

Every signed PutObject binds `Content-Length`, `Content-Type` and `If-None-Match: *`,
plus `x-amz-checksum-sha256` when the caller declared a SHA-256. Every signed
UploadPart binds the part's length and, when the client supplies one, its SHA-256. The
part size is 16 MiB, or larger when the maximum size needs more than 10,000 parts.
Parts upload in parallel, and a failed part is sent again on a URL from
`sign_upload_parts`.

`complete_upload` completes the multipart upload create-only with the part list from
ListParts, then reads the object's size with HeadObject. When RustFS checked a
declared SHA-256 during a single PUT, the artifact becomes available at once. In every
other case the server computes the SHA-256 itself. Objects up to 8 MiB are hashed
during `complete_upload`. A larger object reads `verifying` while a Task streams it
once, and becomes available when the Task records the digest. A declared SHA-256 that
does not match fails the artifact and deletes the object.

An upload expires 24 hours after creation. The sweep aborts it and releases its
reservation, and a bucket lifecycle rule removes any multipart upload the sweep missed.

### Downloads

`create_download` signs a GetObject valid for 5 minutes, with
`response-content-disposition` set to an attachment named after the artifact and
`response-content-type` set to its media type. Ranges and resumed transfers go to
RustFS on the same URL.

### Bucket Preparation

At startup the server creates the bucket when it does not exist, sets CORS to the
installation's application origins for PUT and GET with the headers the signed URLs
bind, and sets a lifecycle rule that aborts incomplete multipart uploads after one
day. The server uses aws-sdk-s3 1.152.0 without default features, with rustls on ring
and without aws-config. It builds two clients from one configuration: one signs URLs
with the address clients use, because SigV4 binds the Host header, and one makes its
own calls on the installation network.

## Access Control

The Gateway admits a tool by its permission. Artifact then decides the specific
artifact, and both must allow the call. One SQL function, `artifact.effective_level`,
makes the per-artifact decision from the caller's facts, so lists filter inside the
query before their row limit.

An artifact does not exist for a caller when it belongs to another tenant, has passed
its retention, has been deleted, or carries a label outside the caller's data labels.
Otherwise the caller's level is the highest of:

- admin for the owner;
- the caller's unexpired principal grants;
- the caller's unexpired group grants, each capped by the caller's role in that group;
- the caller's membership in the artifact's Work Context, while it is the caller's
  selected one: a viewer reads, a contributor writes, and a custodian or owner
  administers.

A tool needs read, write or admin as the table above lists. A caller below the needed
level who can see the artifact gets a refusal that names the level. A caller who cannot
see it gets the same answer as for an unknown ID.

## Sharing

| Mechanism | Reaches | Rules |
|---|---|---|
| Grant | a principal or group in the tenant | Read, write or admin, with an optional expiry. The owner's admin access cannot be revoked. |
| Work Context membership | the members of the artifact's Work Context | The level follows the membership role. |
| Share link | anyone holding the link | Read only, for a released artifact, for at most 30 days, with an optional redemption limit. The server stores only the SHA-256 of the link's secret. |
| Access request | a caller who can see an artifact but lacks a level | Pending, then approved, denied or cancelled. Approval writes the grant in the same transaction. |

Revoking a link or returning an artifact to private stops new redemptions at once. A
URL already issued expires within its 5 minutes. A redemption counts once per visit to
the share route, because ranges and resumes go to RustFS.

## Calls From Other Servers

A server such as Media calls Artifact through the Gateway with a token from token
exchange. The token names the original caller and an `act` chain of the calling server
and the Gateway, and carries only the Artifact permissions the calling server declared
and the caller holds. Artifact authorizes the call for the caller and records the acting
server and the Task in the artifact's provenance.

An idempotency key on `create_upload` is unique per tenant and acting identity. A
repeated call with the same key returns the existing upload or artifact, so a Task
that restarts after a crash does not store an output twice.

## Lists And Paging

Every collection pages by keyset in a fixed order: artifacts by UUIDv7 ID, newest
first, and the others by creation time and ID. An artifact listing applies at most one
filter, and each filter has its own pair of statements. A name filter matches a prefix
and orders the matches by name, then ID, so the prefix stays an index range. A page holds 1 to 100 entries, 50 by
default, through core's `PageSize<100>`, and the query reads one row past the page to
decide whether a next cursor exists. Cursors are core's `KeysetCursor`, bound to the
tenant and the filter set, so a cursor from another listing, tenant or filter fails to
parse. First and next pages are separate statements, and every filter has its own
index. Visibility is part of each statement, so a page holds only artifacts the caller
may read.

## Persistence

| Table in `artifact` | Holds | Retention |
|---|---|---|
| `artifact` | ID, tenant, Work Context, owner, name, media type, size, SHA-256, labels, release state, provenance, metadata, retention and state (`uploading`, `verifying`, `available`, `failed`, `deleting`) | Until deleted or its retention passes |
| `grant` | Artifact, subject kind and ID, level, expiry and grantor | With the artifact, or at expiry |
| `upload` | Artifact, creator, acting identity, idempotency key, form, declared size and SHA-256, maximum size, S3 upload ID, part size, state and expiry | 7 days after it ends |
| `share_link` | Artifact, secret digest, expiry, redemption limit and count, and revocation | 30 days after expiry or revocation |
| `access_request` | Artifact, requester, level, justification, state, decider and note | 90 days after a decision |
| `storage_usage` | Per tenant: stored and reserved bytes | With the tenant |
| `upload_limits` | Per tenant: quota, largest object, allowed media types and active-upload limit | With the tenant |

Triggers keep `storage_usage` current as uploads reserve, complete and end and as
artifacts are deleted. `create_upload` locks the tenant's usage row while it reserves,
so two uploads cannot both take the last of a quota. A caller without a declared size
reserves its maximum size, or the tenant's largest object when it gives none, and
completion settles the reservation to the real size.

Deleting an artifact marks it `deleting` in the transaction that removes its grants
and links. The sweep then deletes the object and the row. The same sweep ends expired
uploads and artifacts whose retention passed, in short transactions of at most 500
rows.

## Audit And Notifications

Every decision appends an audit record in the transaction that makes the change:
uploads created and completed, downloads issued, grants, release changes, share links
created, revoked and redeemed, access requests and deletions. Artifact declares these
classes as `artifact.<name>` with a JSON Schema for each detail when it registers. A
record states the authorization, such as a principal allowed to download an artifact
until a time. An installation that must prove each transfer adds RustFS's own audit
feed.

Row triggers notify the `artifact` channel with each row's key and tenant, and
subscriptions wake only for their own tenant.

## Authorization

`ArtifactPermission` names the permissions in the tool table. The requirement table
maps each tool, resource template and prompt to its permission, builds the
`ai.veoveo/authorization` declaration, and drives every handler's check of the
forwarded token. A target the declaration omits is denied.

## Registration

Artifact registers with the Gateway at startup like any server: it loads its key,
discovers the registration endpoint, registers its slug, scheme, endpoint, permissions
and audit details, connects with the database login the response grants, applies its
migrations, prepares its bucket and serves. On SIGTERM it stops serving and deletes
its registration.

## Deployment

The image runs as UID 10001 with a read-only root filesystem. The installation runs
RustFS beside PostgreSQL, pinned by digest, and gives the server its storage address,
bucket name and an access key limited to that bucket through an OpenShell provider.
Locally the signed address is `http://host.openshell.internal:9000`, and a host browser
maps that name to `127.0.0.1` as it does for the Gateway. A client's sandbox policy
names the RustFS endpoint for the binaries that transfer bytes.

## Contract Compliance

| Area | State |
|---|---|
| Inline content for a model without a sandbox (`write_artifact` and a content resource) | Open decision D27 |
| Calls from other servers | Needs Gateway token exchange (A5) |
| Sharing, share links and access requests | Delivered by M7 |
| The library App | Delivered by M8 |

## Verification

| Layer | Checks |
|---|---|
| Unit | Each signed URL binds the declared headers and method; media type, name, label and metadata admission; part size selection; level computation |
| Database | Core's database checks over every statement and table; `effective_level` against a matrix of owners, grants, groups, Work Context roles and labels; quota reservation under concurrent uploads |
| Object storage | A private RustFS per test process, from the binary in the pinned image: single and multipart uploads with and without declared checksums, a wrong size and a wrong checksum refused, create-only completion, ranged downloads, and sweep clean-up |
| Acceptance | Through the Gateway on macOS and Linux: an upload of several GiB and a ranged download, a cross-tenant read refused, a quota refusal, deletion releasing bytes |
