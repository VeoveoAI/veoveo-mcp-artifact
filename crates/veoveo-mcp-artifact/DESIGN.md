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
readiness    /readyz (ready while the bucket answers)
port         8801
```

## Standards And Protocols

| Standard or protocol | Implemented profile |
|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification/) | Version `2026-07-28`, JSON-RPC 2.0 over Streamable HTTP, under the hosted MCP contract `https://veoveo.ai/contracts/hosted-mcp` that `veoveo-sdk-rust` 5165538 implements. The server exposes tools, resources and templates, completions of document IDs, `subscriptions/listen` and typed structured content. |
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/) | Tool inputs and results, resource contents and the audit detail types Artifact declares. |
| [URI Template RFC 6570](https://www.rfc-editor.org/rfc/rfc6570) | Discovery declares simple variables, and collection pages continue through a `cursor` query parameter. |
| Amazon S3 REST API | The subset in [Object Storage](#object-storage): PutObject, CreateMultipartUpload, UploadPart, ListParts, CompleteMultipartUpload, AbortMultipartUpload, HeadObject, GetObject, DeleteObject, CreateBucket, PutBucketCors and PutBucketLifecycleConfiguration. |
| AWS Signature Version 4, query-string authentication | Signed URLs for PutObject, UploadPart and GetObject, valid for at most 15 minutes. Each URL binds the method, object key and every header the client must send. |
| [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) `If-None-Match: *` | Create-only writes on PutObject and CompleteMultipartUpload. |
| [RFC 6266](https://www.rfc-editor.org/rfc/rfc6266) | Download responses carry an attachment disposition with the artifact's name as `filename*`. |
| [RFC 6838](https://www.rfc-editor.org/rfc/rfc6838) media types | An artifact's media type is a bare `type/subtype` without parameters. |
| SHA-256 | Every available artifact records its SHA-256 in the foundational `Sha256Digest` spelling, `sha256:` followed by 64 lowercase hexadecimal digits. |
| [JWT access tokens RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html) and [Token Exchange RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html) | The Gateway forwards each request with an access token naming the caller, their tenant, groups, data labels and Work Context authority. A call from another server carries an `act` chain naming that server, which Artifact records as the acting server. `ArtifactClient` calls through a Gateway profile with a token the SDK obtains by exchange. |
| [HTTP range requests, RFC 9110 §14](https://www.rfc-editor.org/rfc/rfc9110#name-range-requests) | `ArtifactClient` sends one `bytes=first-last` or `bytes=first-` range with a signed GET and expects `206 Partial Content`. |
| [HTTP semantics, RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) `303 See Other` and `Cache-Control: no-store` ([RFC 9111](https://www.rfc-editor.org/rfc/rfc9111)) | The share route answers a live link with `303` to a signed GET, and every answer carries `no-store`. |
| [Base64url, RFC 4648 §5](https://www.rfc-editor.org/rfc/rfc4648#section-5) | A share link's secret is 32 random bytes in unpadded canonical base64url, 43 characters. |
| [Referrer Policy](https://www.w3.org/TR/referrer-policy/) | The share route's answers carry `Referrer-Policy: no-referrer`. |
| [Veoveo server registration](https://github.com/VeoveoAI/veoveo-sdk-rust) | Artifact registers through RFC 7591, removes its registration through RFC 7592, and declares its permissions in `ai.veoveo/authorization`. |
| [PostgreSQL 18](https://www.postgresql.org/docs/18/) | Server 18.6 through sqlx 0.9.0, under the [installation database design](https://github.com/VeoveoAI/veoveo-core/blob/f685fbfd3d28a477d35f77cee3feacc8787772ac/docs/DATABASE.md) of `veoveo-core`. Artifact owns the schema `artifact`, applies its own migrations and runs every statement from a `.sql` file that sqlx checks. Row triggers publish changes on the `artifact` channel. |
| RustFS 1.0.1 | The installation's object store, pinned by image digest. Artifact uses only the S3 calls listed above through aws-sdk-s3 1.152.0, so an S3-compatible store such as Ceph or AWS S3 can replace it. |

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
| `contract` | `ArtifactId`, the typed resource addresses, every tool request and result, metadata, grant levels and subjects, access requests, upload limits, the `ArtifactPermission` vocabulary, the audit detail types, and `ArtifactProvider` with a `ToolCall` for each served tool | `veoveo-types`, the SDK's `contract` feature, Serde and Schemars |
| `client` | `contract` plus `ArtifactClient`, which uploads a stream of known or unknown size and downloads through signed URLs, built on the SDK's `GatewayClient` | the SDK's `client` feature, reqwest, Tokio and sha2 |
| `server` (default) | the hosted server, persistence and object storage | the SDK's `server` feature, sqlx and aws-sdk-s3 |

A consumer such as Media selects `default-features = false, features = ["client"]` and
compiles neither sqlx nor the S3 SDK. `tests/contract_boundary.rs` proves both features'
boundaries with `cargo tree`.

### Hosted Process

The binary parses configuration, prepares its bucket, registers with the Gateway,
connects to PostgreSQL with the login its registration returned, applies its
migrations and serves MCP. Three loops run on every replica beside it: the sweep, which
claims rows with `FOR UPDATE SKIP LOCKED`; the verification worker, which claims
verification Tasks from core's Task runtime; and the change watcher, which hands each
change on the `artifact` channel to its tenant's subscribers.

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
| `request_artifact_access`, `cancel_access_request` | Asks an artifact's administrators for a level above the caller's, or withdraws the caller's own pending request | `artifact.access-requests.write` | read, and a requested level above the caller's |
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
| `artifact://artifacts{?work_context,owner,media_type,name,release_state,cursor}` | A page of artifacts the caller may read, newest first |
| `artifact://artifacts/{artifact_id}` | The artifact's metadata |
| `artifact://artifacts/{artifact_id}/grants` | Its grants, for administrators |
| `artifact://artifacts/{artifact_id}/share-links{?cursor}` | Its share links without their secrets, for administrators |
| `artifact://uploads{?state,cursor}`, `artifact://uploads/{upload_id}` | The caller's uploads and their state |
| `artifact://access-requests{?role,state,cursor}` | Requests the caller made, or can decide |
| `artifact://storage-usage` | The tenant's stored and reserved bytes against its quota |

Single resources and the first page of each collection accept subscriptions. A
change to an artifact, its grants or its share links reaches that artifact's resources
and every artifact listing; a change to an upload reaches that upload and the upload
listing; a change to an access request reaches every access-request listing of its
tenant, whose readers then read only the requests they made or can decide. A
subscription to an artifact's share links needs admin on the artifact.

### Prompts

| Prompt | Arguments | Guides the model to |
|---|---|---|
| `review-artifact-sharing` | `artifact_id` | read the artifact, its grants and its share links, report who can reach it and how, and propose revocations the user then confirms |
| `review-access-requests` | none | read the pending requests the caller can decide, weigh each against its artifact, and decide only the ones the user decides |

Each prompt requires `artifact.artifacts.read`, and acts only through the tools above.

### Share Route

`GET /artifact/shares/{token}` is the only route outside MCP, because a share link must
work for someone without an account. The hosted process serves it under its mount
without Gateway authentication; the link's secret authenticates the request.

| The request names | The answer |
|---|---|
| a live link, neither revoked, expired nor at its redemption limit, to a released, available artifact without labels | `303 See Other` to a signed GET valid for 5 minutes, after one redemption and its audit record commit together |
| anything else: a malformed, unknown, expired, revoked or spent token, or an artifact that is private, deleted, past its retention or unavailable | `404` with one fixed body |
| a `HEAD` request | `405` with `Allow: GET`, counting nothing |

Every answer carries `Cache-Control: no-store`, `Referrer-Policy: no-referrer` and
`X-Content-Type-Options: nosniff`, and no refusal tells the holder why. A database
failure answers `503`. The token is 43 characters of canonical unpadded base64url
holding 256 random bits from the operating system. The server looks the link up by the
SHA-256 of those bits, which is all it stores, so the URL `create_share_link` returns is
the only copy of the secret. One `UPDATE` counts the redemption under the link's row
lock and admits it only while the count is below the limit, so concurrent redemptions
never pass the limit. A redemption's record names no actor, because the holder has no
account, so it lands in the installation's audit partition.

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
during `complete_upload`. A larger object reads `verifying` while a verification Task
streams it once and records the digest. The Task carries the artifact's identifier, so
completing the upload again finds the same Task, and a replica resumes the Task when the
instance that claimed it dies. A declared SHA-256 that does not match fails the
artifact, removes its object and refuses the completion. When a verifying artifact
still has no live Task 15 minutes after its upload completed, the sweep fails it and
removes its object.

An upload expires 24 hours after creation. The sweep then aborts its multipart upload,
removes any object that arrived and releases its reservation, and a bucket lifecycle
rule removes any multipart upload the sweep missed. Aborting an upload removes them at
once.

### Downloads

`create_download` signs a GetObject valid for 5 minutes, with
`response-content-disposition` set to an attachment named after the artifact and
`response-content-type` set to its media type. Ranges and resumed transfers go to
RustFS on the same URL.

### Bucket Preparation

At startup the server creates the bucket when it does not exist, sets CORS to the
installation's application origins for PUT and GET with the headers the signed URLs
bind (`content-type`, `if-none-match` and `x-amz-checksum-sha256`), and sets a
lifecycle rule that aborts incomplete multipart uploads after one day. The server uses aws-sdk-s3 1.152.0 without default features, with rustls on ring
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
| Share link | anyone holding the link | Read only, for a released artifact without data labels, for 1 to 30 days, with an optional redemption limit. An artifact holds at most 256 live links. The server stores only the SHA-256 of the link's secret. |
| Access request | a caller who reads an artifact and asks for write or admin | Pending, then approved, denied or cancelled. Approval writes the grant in the same transaction. |

An administrator releases an available artifact with `set_release_state`. A share link
reaches people without data labels, so an artifact that carries a label cannot be
shared by link. Returning an artifact to private revokes its live links in the same
transaction, so releasing it again revives none; revoking a link or deleting its
artifact also stops new redemptions at once. A URL already issued expires within its 5
minutes. A redemption counts once per visit to the share route, because ranges and
resumes go to RustFS.

A caller holds at most one pending request per artifact, and asks only for a level
above the one it holds; a caller who cannot read the artifact gets the answer for an
unknown ID. Only the requester withdraws a request, and only an administrator of the
artifact decides it. An approval gives the requester a principal grant of the requested
level without an expiry, unless a grant it already holds gives at least that level
without one, and records the grant change beside the decision. The access-request
listing holds the requests the caller made, those on artifacts it administers, or,
without a role, both, newest first.

## Calls From Other Servers

A server such as Media declares uses of `artifact.artifacts.read` and
`artifact.artifacts.write` when it registers, and calls Artifact through the Gateway with
a token from token exchange. The token names the original caller and an `act` chain of
the calling server and the Gateway, and carries only the Artifact permissions the
calling server declared and the caller holds. Artifact authorizes the call for the
caller, records the acting server in the artifact's provenance, and names it as
`actingServer` in the audit record of every decision the call makes.

An idempotency key on `create_upload` is unique per tenant and acting identity. A
repeated call with the same key returns the existing upload or artifact, so a Task
that restarts after a crash does not store an output twice.

The `contract` feature publishes each served tool as a typed call. `ArtifactProvider`
names the `artifact` slug, and each request type names its tool and result, such as
`CreateUploadRequest` with `CreateUploadResult`, so a consumer calls
`client.call(request)` on the SDK's `GatewayClient` and never spells a tool name.

### ArtifactClient

`ArtifactClient::new(calls, http)` wraps a `GatewayClient`: the one `ctx.client()` binds
to a request's caller, the one `ServerCalls::for_task` binds to a Task's caller, or a
program's own. `http` moves the bytes straight to object storage, and the program
chooses its TLS configuration. `upload(name, media_type, body)` and `download(id)` are
builders that run when awaited.

An upload reads the body from an `AsyncRead`, a stream of chunks or bytes in memory:

| Body | Upload |
|---|---|
| at most 16 MiB, size known or not | The client reads it into memory, then declares its size and SHA-256 with `create_upload`. One create-only PUT sends it, and RustFS refuses other bytes. The artifact is available at completion. |
| larger than 16 MiB | The client declares a known size as the upload's maximum size, so the upload is multipart and every part can be signed and sent again on its own. It reads parts of the upload's part size and sends them while it reads the next. |

A part takes a permit before the client reads it, so at most `parallelism` parts are in
memory. The default holds 64 MiB: four 16 MiB parts, or one part when the part size is
larger. A body of unknown size without a maximum size reserves the tenant's largest
object, whose parts can reach 525 MiB, so a consumer gives the maximum size it expects.
The client signs parts in batches of up to 100 as the upload reaches them, signs a part
again when its signature expires within a minute or object storage refuses it, and sends
a part up to four times when the network or storage fails. A body that holds another
number of bytes than its declared size, or more than its bound, fails the upload, and
every failure after `create_upload` aborts the upload, which releases its reserved bytes.
An upload whose future is dropped stays open until it expires.

A download calls `create_download` and streams the signed GET, whole or as one range.
A failure names object storage's status and S3 error code, never the signed URL.

A call bound to a request lasts as long as the forwarded token, at most a minute, so a
handler starts a Task for an upload that may take longer. A consumer's OpenShell policy
lets its binary reach the Gateway's `/oauth/token` and `/mcp/{profile}`, and object
storage as TCP pass-through, like Artifact's own policy.

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
| `artifact` | ID, tenant, Work Context, owner, name, media type, size, SHA-256, labels, release state, provenance, metadata, retention and state (`uploading`, `verifying`, `available`, `failed`, `deleting`) | Until deleted or its retention passes; a failed artifact, which holds no object, 7 days after it failed |
| `grant` | Artifact, subject kind and ID, level, expiry and grantor | With the artifact, or at expiry |
| `upload` | Artifact, creator, acting identity, idempotency key, form, declared size and SHA-256, maximum size, S3 upload ID, part size, state and expiry | 7 days after it ends |
| `share_link` | Artifact, secret digest, expiry, redemption limit and count, and revocation | 30 days after expiry or revocation |
| `access_request` | Artifact, requester, level, justification, state, decider and note | 90 days after a decision |
| `storage_usage` | Per tenant: stored and reserved bytes | With the tenant |
| `upload_limits` | Per tenant: quota, largest object, allowed media types and active-upload limit. A tenant without a row has no quota, objects up to 5 TiB, any media type and 64 open uploads | With the tenant |

Triggers keep `storage_usage` current as uploads reserve, complete and end and as
artifacts are deleted. `create_upload` locks the tenant's usage row while it reserves,
so two uploads cannot both take the last of a quota. A caller without a declared size
reserves its maximum size, or the tenant's largest object when it gives none, and
completion settles the reservation to the real size.

An artifact counts toward stored bytes from completion until its row goes, so a size
exists only in the `verifying`, `available` and `deleting` states. Deleting an artifact
marks it `deleting`, and `artifact.effective_level` hides it from every caller at once.
The sweep then claims a batch of deleted artifacts, deletes each object and then each
row, and commits. The row's grants, uploads, share links and access requests go with it
through foreign-key cascades. The same sweep ends expired uploads and fails their
artifacts, marks artifacts whose retention passed as deleting, removes each failed
artifact's object and then its row 7 days after it failed, and prunes rows past their
retention, each in short batches that `FOR UPDATE SKIP LOCKED` divides between
replicas. The server removes a failed artifact's object when it fails, and the sweep
removes it again in case the process stopped first.

Listings hold `verifying` and `available` artifacts. An upload's progress is read
through the uploads resource. An artifact holds at most 256 grants, and its name holds
no Unicode noncharacters, so a prefix followed by U+10FFFF bounds every name that starts
with it.

## Audit And Notifications

Every decision appends its audit record on the connection of the transaction that
makes the change, so the decision and its record commit together or not at all.
Artifact's contract declares a closed detail type for each class:

| Class | Recorded when | Outcome |
|---|---|---|
| `artifact.upload_created` | `create_upload` reserves an artifact | succeeded |
| `artifact.upload_completed` | `complete_upload` records the bytes that arrived | succeeded, or failed with reason `conflict` when the size differs from the declared one |
| `artifact.verified` | the server records a computed SHA-256, during `complete_upload` or in the verification Task | succeeded, or failed with reason `conflict` when it differs from the declared digest |
| `artifact.upload_aborted` | `abort_upload` | succeeded |
| `artifact.download_issued` | `create_download` signs a URL, before the URL is returned | succeeded |
| `artifact.grant_changed` | `grant_artifact`, or `revoke_artifact_grant` with no level | succeeded |
| `artifact.deleted` | `delete_artifact` marks the artifact | succeeded |
| `artifact.upload_limits_set` | `set_upload_limits` | succeeded |
| `artifact.release_changed` | `set_release_state`, with the number of live share links a return to private ended | succeeded |
| `artifact.share_link_created` | `create_share_link`, with the link's expiry and limit | succeeded |
| `artifact.share_link_revoked` | `revoke_share_link` | succeeded |
| `artifact.share_link_redeemed` | the share route counts a redemption, before it answers; the record names no actor | succeeded |
| `artifact.access_requested` | `request_artifact_access` | succeeded |
| `artifact.access_request_cancelled` | `cancel_access_request` | succeeded |
| `artifact.access_request_decided` | `decide_access_request`, with the decision; an approval that writes a grant also records `artifact.grant_changed` | succeeded |

Records about an artifact target its resource address; a limits record targets the
server. Every detail names the acting server as `actingServer` when another server made
the call for the caller. A record's actor, authority and request come from the Gateway's
signed request context, and a verification Task's records carry its owner and a background request.
Startup declares the classes through the server's own login with core's
`AuditStore::declare_as_server`, and each record goes through
`AuditTransactionWrite::append_as_server`, which the database confines to classes under
the `artifact.` prefix. A record states the authorization, such as a principal allowed
to download an artifact until a time. An installation that must prove each transfer
adds RustFS's own audit feed.

Row triggers notify the `artifact` channel with each row's key and tenant, and
subscriptions wake only for their own tenant.

## Authorization

`ArtifactPermission` names the permissions in the tool table. The requirement table
maps each tool, resource template and prompt to its permission, builds the
`ai.veoveo/authorization` declaration, and drives every handler's check of the
forwarded token. A target the declaration omits is denied.

## Registration

Artifact prepares its bucket, then registers with the Gateway at startup like any
server: it loads its key, discovers the registration endpoint, registers its slug,
scheme, endpoint and permissions, connects with the database login the response grants,
applies its migrations, declares its audit classes and serves. On SIGTERM it stops serving, stops its loops, ends
its process instance and deletes its registration.

## Deployment

The image runs as UID 10001 with a read-only root filesystem. The installation runs
RustFS beside PostgreSQL, pinned by digest, and configures the server through
`VEOVEO_STORAGE_ENDPOINT`, `VEOVEO_STORAGE_SIGNING_ENDPOINT` (the address clients use,
defaulting to the endpoint), `VEOVEO_STORAGE_REGION`, `VEOVEO_STORAGE_BUCKET`,
`VEOVEO_STORAGE_ACCESS_KEY`, `VEOVEO_STORAGE_SECRET_KEY` and
`VEOVEO_STORAGE_CORS_ORIGINS`, beside the registration settings every server takes.
`VEOVEO_SHARE_BASE_URL` names the origin a share link's URL uses, defaulting to the
public base URL, and the server accepts its host in the `Host` header. The installation
routes that origin's `/artifact/shares/*` to the server for people without an account.
SigV4 signs with the secret key inside the server, so the server's environment holds the
credential itself, the way the Gateway's holds the database owner's password; a
placeholder that a proxy substitutes cannot sign. The credential is limited to the
bucket. Locally the signed address is `http://host.openshell.internal:9000` and share
links name `http://host.openshell.internal:8801`, Artifact's forward, so a host browser
follows both once the host maps that name to `127.0.0.1`, as it does for the Gateway. The sandbox policy
passes object storage through as TCP, because an inspecting proxy that changed a signed
header would void the signature.

## Contract Compliance

| Area | State |
|---|---|
| Inline content for a model without a sandbox (`write_artifact` and a content resource) | Open decision D27 |
| The calling server's Task in provenance | Pending: the forwarded token names the acting server but not its Task, so `provenance.task` stays empty for calls from other servers |
| Waiting for a large artifact's verification in `ArtifactClient` | Pending: an upload resolves with a `verifying` artifact, and the client offers no wait, because MCP resource reads are outside the SDK's `GatewayClient` |
| The share route's secret in request telemetry | Pending in `veoveo-sdk-rust`: the hosted server's request span records the raw request path, which for the share route holds a link's secret. The route itself logs nothing within that span; the SDK fix records the matched route template instead |
| The library App | Plan item M8 |
| Helm chart | Pending (C15) |

## Verification

| Layer | Checks |
|---|---|
| Unit | Checksum headers, object keys, attachment dispositions, part plans, permission risks, audit schemas, the requirement table and tool annotations, the MCP setup, subscription matching and configuration |
| Database | Core's database checks over every statement and table; `effective_level` against a matrix of owners, grants, groups, Work Context roles and labels; quota reservation under concurrent uploads; paging and cursor binding; the sweep's claims |
| Object storage | A private RustFS per test process, from the binary in the pinned image: signed single and multipart uploads, a wrong size, checksum or part checksum refused, create-only writes, idempotent completion and abort, ranged downloads and bucket preparation |
| Service | The store and RustFS together: declared and undeclared uploads, refused bytes, aborts, a large object verified in a Task, a digest mismatch, a repeated idempotency key, download access, and the sweep's removal of abandoned, deleted and orphaned objects |
| MCP | The SDK's in-process Gateway with tokens that carry a request context: discovery of the tools, which match the typed calls the contract publishes, the resources and the prompts; permission refusals on tools and resources, including every sharing tool; a signed upload counted in storage usage; upload limits; an owner who releases an artifact, creates a share link, lists it without its secret and redeems it once through the hosted router's share route; and an owner refused a request for more access |
| Sharing | Database tests: returning to private ends only live links and releasing again revives none; links need an administrator and a released, available artifact without labels; twelve concurrent redemptions of a link limited to three count exactly three; expired, revoked, deleted and unknown links redeem nothing, and a dropped redemption counts nothing; share-link pages and their cursors. Access requests: an approval writes the requested grant and records each step, visibility and the held level bound a request, only the requester withdraws and only an administrator decides, an approval keeps a stronger grant, and listings page by role and state |
| Share route | `tests/share_route.rs` through the hosted router and a private RustFS: a live link redirects with `no-store` to a 5-minute signed GET that serves ranges; expired, revoked, private, deleted, spent, unknown and malformed tokens answer the same `404`, headers and body included; a `HEAD` answers `405` and counts nothing |
| Client | `tests/client.rs` through the SDK's in-process mesh, with Artifact over a fresh database and a private RustFS, for Ledger, the SDK's calls fixture server, which declares uses of Artifact's read and write permissions: a request binding uploads a small body of unknown size in one PUT and 40 MiB of unknown size in parallel parts, then downloads whole, as a range across a part boundary and as a tail; a Task binding uploads 20 MiB of declared size after its request returned; a caller without the write permission is refused; the acting server is recorded in provenance and in the audit records; a body shorter than its declared size aborts the upload and releases its bytes. `tests/contract_boundary.rs` holds the `contract` and `client` features to their dependency boundaries |
| Audit | Records read back as the installation owner: a decision and its record commit together, a refused record rolls its decision back, every decision, sharing included, records its declared class with its target, actor and detail, a redemption records no actor in the installation's partition, and a digest mismatch records a failed verification |
| Acceptance | `tests/acceptance.rs` through a running installation's Gateway and RustFS, one test at a time: each caller listing exactly the tools its role may call and the member refused the tools whose permissions carry a risk, a declared upload available at completion and read as a range by a Work Context member, a gibibyte of unknown size uploaded in parts, verified, read as a range across a part boundary and released by the sweep after deletion, a grant listed and revoked, the admin's quota refusing an upload, a share link opened through Artifact's forward and its redirect to the object store until its limit of one is spent, and the member's request for admin approved by the admin. The local installation has one tenant, so the database tests cover a read refused across tenants |
