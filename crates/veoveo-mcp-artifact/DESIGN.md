# Artifact MCP Design

This document is the design and operational contract for the `veoveo-mcp-artifact`
crate.

Artifact stores files for a tenant and decides who may read, change, share and delete
each one. People, agents and other servers ask it through MCP. Bytes move directly
between the client and object storage through URLs the server signs, so the Hub
and MCP carry only metadata and decisions. A person who opens a file in a browser previews it
on the installation's content domain instead, where the server streams it, because a signed
URL is an attachment on the Hub's own host.

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
content      /artifact/content/{artifact_id}/{name}, /artifact/content-links/{token}/{name}
App          ui://artifact/library
liveness     /livez
readiness    /readyz (ready while the bucket answers)
port         8801
```

## Standards And Protocols

| Standard or protocol | Implemented profile |
|---|---|
| [Model Context Protocol](https://modelcontextprotocol.io/specification/) | Version `2026-07-28`, JSON-RPC 2.0 over Streamable HTTP, under the hosted MCP contract `https://veoveo.ai/contracts/hosted-mcp` that `veoveo-hub-sdk` typed route construction implements. The server exposes tools, resources and templates, completions of document IDs, `subscriptions/listen` and typed structured content. |
| [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/) | Tool inputs and results, resource contents and the audit detail types Artifact declares. |
| [Veoveo audit record](https://github.com/VeoveoAI/veoveo-core/blob/9a6b3ff95469ff19f9ccb513421a57ba6751aa9e/platform/audit/contract/DESIGN.md), after NIST SP 800-53 Rev. 5 AU-3 and mapped to [OCSF 1.9](https://schema.ocsf.io/1.9.0/) | Each record carries core's envelope: Artifact's class with its OCSF API Activity (6003) and activity, an outcome, Artifact's reason code when a decision fails, the caller, the acting server and Task as `via`, the Artifact resource the decision concerns as its target, and a closed detail. |
| [URI Template RFC 6570](https://www.rfc-editor.org/rfc/rfc6570) | Discovery declares simple variables, and collection pages continue through a `cursor` query parameter. |
| Amazon S3 REST API | The subset in [Object Storage](#object-storage): PutObject, CreateMultipartUpload, UploadPart, ListParts, CompleteMultipartUpload, AbortMultipartUpload, HeadObject, GetObject, DeleteObject, CreateBucket, PutBucketCors and PutBucketLifecycleConfiguration. |
| AWS Signature Version 4, query-string authentication | Signed URLs for PutObject, UploadPart and GetObject, valid for at most 15 minutes. Each URL binds the method, object key and every header the client must send. |
| [RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) `If-None-Match: *` | Create-only writes on PutObject and CompleteMultipartUpload. |
| [RFC 6266](https://www.rfc-editor.org/rfc/rfc6266) | Download responses carry an attachment disposition with the artifact's name as `filename*`. |
| [RFC 6838](https://www.rfc-editor.org/rfc/rfc6838) media types | An artifact's media type is a bare `type/subtype` without parameters. |
| Object identity | ArtifactID and create-only storage identify an object. Artifact checks its size; consumers own any content digest policy. Share-link secrets still use SHA-256 protection. |
| [JWT access tokens RFC 9068](https://www.rfc-editor.org/rfc/rfc9068.html) and [Token Exchange RFC 8693](https://www.rfc-editor.org/rfc/rfc8693.html) | The Hub forwards each request with an access token naming the caller, their tenant, groups with their roles, and data labels. A call from another server carries an `act` chain naming that server, and the server's Task when the call came from one, which Artifact records in each artifact's provenance and as the `via` of its audit records. `ArtifactClient` calls through a Hub profile with a token the SDK obtains by exchange. |
| [HTTP range requests, RFC 9110 §14](https://www.rfc-editor.org/rfc/rfc9110#name-range-requests) | `ArtifactClient` sends one `bytes=first-last` or `bytes=first-` range with a signed GET and expects `206 Partial Content`. A preview answers one range itself, with `If-Range`; several ranges answer the whole file. |
| [HTTP semantics, RFC 9110](https://www.rfc-editor.org/rfc/rfc9110) `303 See Other` and `Cache-Control: no-store` ([RFC 9111](https://www.rfc-editor.org/rfc/rfc9111)) | The share route answers a live link with `303` to a signed GET, or to its preview for a browser, and every answer carries `no-store`. A preview's entity tag is the artifact's ID, because an artifact is immutable once available. |
| [Fetch Metadata](https://www.w3.org/TR/fetch-metadata/) | `Sec-Fetch-Mode: navigate` and `Sec-Fetch-Dest: document` tell the share route a browser is opening a link. |
| [MCP Apps](https://modelcontextprotocol.io/extensions/apps) `2026-01-26` | The library App, `ui://artifact/library`. |
| [Base64url, RFC 4648 §5](https://www.rfc-editor.org/rfc/rfc4648#section-5) | A share link's secret is 32 random bytes in unpadded canonical base64url, 43 characters. |
| [Referrer Policy](https://www.w3.org/TR/referrer-policy/) | The share route's answers carry `Referrer-Policy: no-referrer`. |
| [MCP Skills extension](https://github.com/modelcontextprotocol/ext-skills) `io.modelcontextprotocol/skills`, SEP-2640, and the [Agent Skills](https://agentskills.io/specification) format | The skills under `skills/`, embedded at build time and served at `skill://artifact/{name}/{path}` through the SDK; see [Skills](#skills) |
| [Veoveo server registration](https://github.com/VeoveoAI/veoveo-hub-sdk) | Artifact registers through RFC 7591, removes its registration through RFC 7592, and declares its permissions in `ai.veoveo/authorization`. |
| [PostgreSQL 18](https://www.postgresql.org/docs/18/) | Server 18.6 through sqlx 0.9.0, under the [installation database design](https://github.com/VeoveoAI/veoveo-core/blob/9a6b3ff95469ff19f9ccb513421a57ba6751aa9e/docs/DATABASE.md) of `veoveo-core`. Artifact owns the schema `artifact`, applies its own migrations and runs every statement from a `.sql` file that sqlx checks. Row triggers publish changes on the `artifact` channel. pgvector 0.8.7 in the installation's `_extensions` schema holds the knowledge vectors. |
| [Veoveo knowledge contract](https://github.com/VeoveoAI/veoveo-hub-sdk) `ai.veoveo/knowledge` | `search_knowledge` over `artifact.artifacts` through the SDK's ready-made PostgreSQL search, embedded through Embedding's `embed` tool with the client of `veoveo-mcp-embedding`; see [Knowledge](#knowledge) |
| SeaweedFS 4.48 | The installation's object store, run as `weed mini` and pinned by image digest. Artifact uses only the S3 calls listed above through aws-sdk-s3 1.152.0, so an S3-compatible store such as Ceph or AWS S3 can replace it. |

## Domain Contract

Artifact answers these questions for a tenant:

- Which files may this caller see, and what does each one hold: name, media type,
  size, labels, owner, release state and provenance?
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
| `contract` | `ArtifactId`, the typed resource addresses, every tool request and result, metadata, grant levels and subjects, access requests, upload limits, the `ArtifactPermission` vocabulary, the audit detail types, and `ArtifactProvider` with a `ToolCall` for each served tool and `ReadArtifact`, a `ResourceRead` of one artifact | `veoveo-types`, the SDK's `contract` feature, Serde and Schemars |
| `client` | `contract` plus `ArtifactClient`, which uploads a stream of known or unknown size and downloads through signed URLs, built on the SDK's `HubClient` | the SDK's `client` feature, reqwest, Tokio and sha2 |
| `server` (default) | the hosted server, persistence and object storage | the SDK's `server` feature, sqlx and aws-sdk-s3 |

A consumer such as Media selects `default-features = false, features = ["client"]` and
compiles neither sqlx nor the S3 SDK. `tests/contract_boundary.rs` proves both features'
boundaries with `cargo tree`.

### Hosted Process

The binary parses configuration, prepares its bucket, registers with the Hub,
connects to PostgreSQL with the login its registration returned, applies its
migrations, checks its knowledge table and serves MCP. Three loops run on every replica
beside it: the sweep, which claims rows with `FOR UPDATE SKIP LOCKED`; the change
watcher, which hands each change on the `artifact` channel to its tenant's subscribers;
and the Task worker, which runs the index Tasks of people's uploads.

## MCP Surface

### Tools

| Tool | Does | Permission | Level on the artifact |
|---|---|---|---|
| `create_upload` | Reserves quota and an artifact ID and returns the URLs that receive the bytes | `artifact.artifacts.write` | none: the caller creates an artifact it owns |
| `sign_upload_parts` | Signs the next part URLs of a multipart upload, or signs expired ones again | `artifact.artifacts.write` | the upload's creator |
| `complete_upload` | Completes native create-only storage, validates size, makes the artifact available and indexes it for search | `artifact.artifacts.write` | the upload's creator |
| `abort_upload` | Aborts the upload and releases its reservation | `artifact.artifacts.write` | the upload's creator |
| `create_download` | Returns a signed GET URL valid for 5 minutes | `artifact.artifacts.read` | read |
| `grant_artifact`, `revoke_artifact_grant` | Gives or removes read, write or admin for a principal or group, with an optional expiry | `artifact.grants.write` | admin |
| `set_release_state` | Moves an artifact between private and released | `artifact.releases.write` | admin |
| `create_share_link`, `revoke_share_link` | Issues or revokes an anonymous read link for a released artifact | `artifact.share-links.write` (sensitive) | admin |
| `request_artifact_access`, `cancel_access_request` | Asks an artifact's administrators for a level above the caller's, or withdraws the caller's own pending request | `artifact.access-requests.write` | read, and a requested level above the caller's |
| `decide_access_request` | Approves the request, which writes the grant, or denies it with a note | `artifact.grants.write` | admin |
| `delete_artifact` | Deletes the artifact, its grants and share links, and its object | `artifact.artifacts.delete` (destructive) | admin |
| `set_upload_limits` | Sets a tenant's quota, largest object, allowed media types and active-upload limit | `artifact.upload-limits.administer` (administer) | installation administrator |
| `search_knowledge` | Finds available artifacts by words, media or both; see [Knowledge](#knowledge) | `artifact.artifacts.read` | read |

`create_upload` takes the name and media type, and optionally the size,
a maximum size, data labels, a retention period, metadata, the artifacts it derives
from and an idempotency key. The owner, tenant and provenance come from the forwarded
identity, never from arguments: the artifact belongs to its creator, the person the
call is for, never a server's identity, and is private until shared. A call another
server or its Task makes keeps that server and Task in the provenance. Its labels are
exactly the requested labels, each one the caller's data labels hold. `create_upload`
takes no grants: sharing is a separate `grant_artifact` call, so a failed share leaves
the artifact private.

### Resources

| URI | Returns |
|---|---|
| `artifact://artifacts{?owner,media_type,name,release_state,cursor}` | A page of artifacts the caller may read, newest first |
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

### Skills

How an agent uses Artifact is in its skills, which a host loads when a task needs one.
The server's `instructions` say what it is and name each skill's address, and the skill
text names the tools as callers see them through the Hub, `artifact__{tool}`:

| Skill | Guidance |
|---|---|
| `upload-an-artifact` | `create_upload` with its options and idempotency, a single PUT or signed parts, completion and abort, labels within the caller's and the tenant's limits |
| `download-an-artifact` | the listing and its filters, an artifact's metadata, and `create_download` with its 5-minute URL and ranges |
| `share-an-artifact` | how a level is decided, grants to people and groups, share links for released artifacts without labels, reviewing who can reach an artifact, access requests and deletion |
| `find-artifacts` | `search_knowledge` with words, media or both, and reading a hit; see [Knowledge](#knowledge) |

Each skill file is a resource of the server under its document policy,
`artifact.artifacts.read`.

### Prompts

Each prompt states its task and names the skill that says how:

| Prompt | Arguments | Task | Skill |
|---|---|---|---|
| `review-artifact-sharing` | `artifact_id` | review who can reach the artifact through its grants and share links, changing nothing until the user agrees | `share-an-artifact` |
| `review-access-requests` | none | recommend a decision on each pending request the caller can decide, deciding only the ones the user decides | `share-an-artifact` |

Each prompt requires `artifact.artifacts.read`, and acts only through the tools above.

### Library App

`ui://artifact/library`, under `artifact.artifacts.read`, lists what `artifact://artifacts` holds for
the person, newest first, with a name filter and its next pages, and calls no tool. A row opens
`/view/artifact/{artifact_id}/{name}` with `ui/open-link`, so an artifact opens as any file does. A
media run's outputs are artifacts of the person who ran it, so they are listed and open the same
way, and a row names the server that made its artifact. The document is the image's
`/usr/share/veoveo/artifact/library.html`, read once at startup (`VEOVEO_APP_HTML`).

### Share Route

`GET /artifact/shares/{token}` is outside MCP because a share link must work for someone
without an account. The hosted process serves it under its mount without Hub
authentication; the link's secret authenticates the request. The routes declare `/shares/` as
a public route, with the least body limit, 1 byte, since a redemption carries none, and 600
requests a minute, which the Hub enforces and registration declares as the server's surfaces. A
link's URL is `{veoveo_public_base}/shares/{token}`, the instance's public base from the registration
response, or under the installation's public origin or the public base URL without one.

| The request names | The answer |
|---|---|
| a live link, neither revoked, expired nor at its redemption limit, to a released, available artifact without labels, from a browser opening a page, when the installation has a content domain | `303 See Other` to the link's [preview](#previews), after one redemption, the preview's name and its audit record commit together |
| the same from anyone else | `303 See Other` to a signed GET valid for 5 minutes, after one redemption and its audit record commit together |
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
account, so it lands in the installation's audit partition. The hosted server's request
span records the route template, `http.route=/artifact/shares/{token}`, and never the
request's path or query, so neither the token nor the signed GET reaches logs or traces.

### Previews

A preview is an artifact on the Hub's [content domain](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md#content-domain),
which chooses its viewer by its name and frames it: Artifact declares that content in its surfaces,
and the Hub names no server. Each artifact has its own origin. A signed GET is an attachment on the
Hub's own host, where a file served inline would run with the person's session, and it would expire in
the middle of a video, so a preview streams the object from storage through the server. Nothing is
stripped from a file, because the origin's isolation is the protection.

| Request, from the Hub | For | Does |
|---|---|---|
| `HEAD /artifact/content/{artifact_id}` | a person, with a forwarded token | Answers `204` when they may read it now |
| `POST /artifact/content/{artifact_id}` | the person opening it on the Hub's page | The same, and records `artifact.download_issued`: a visit, once however many requests follow |
| `GET`, `HEAD /artifact/content/{artifact_id}/{name}` | the viewer or the file itself | The bytes |
| `GET /artifact/content/{artifact_id}/` | the item's top | `303` to the viewer of its name |
| `HEAD /artifact/content-links/{token}`, `GET`, `HEAD .../{name}`, `GET .../` | whoever holds a link's preview, without a token | The same for the link |

A person's request needs the read permission, and `artifact.effective_level` decides it as it does
`create_download`, on every request: a grant removed ends the preview on the next one, and a person
who cannot see the artifact gets a `404`. A link's preview has an origin of its own,
`artifact-{token}.{content domain}`, which the Hub reads through the public route `content-links/`
and never serves on its own origin. Its token is 160 bits of a SHA-256, under its own purpose, of the
link's secret, so one link has one origin and a file on that origin cannot redeem the link. The
server stores the token's digest when a browser first opens the link, which is the only way to learn
it, and checks on every request that the link is neither revoked nor expired and its artifact
released, available, without labels and within its retention, so any of those ends the preview at once
with one `404`. A redemption limit counts visits, so a preview already opened outlives it until the
link ends; each visit to the share route is one redemption, and a preview's requests are none.

A file's name is its path, and any other name is a `404`. The answer has the artifact's media type, the
artifact's ID as its entity tag, `Accept-Ranges`, and for one range `206` with `Content-Range`, `416`
beyond the end; the object is read through the server's own storage client, and a `HEAD` does not read
it. The Hub adds the origin's policy and its `Cache-Control`.

## Object Storage

Artifact holds one bucket per installation and one object per artifact, at
`tenants/{tenant_id}/artifacts/{artifact_id}`. The server's storage credential is
limited to that bucket and never leaves the server.

### Uploads

`create_upload` chooses the upload's form:

| The caller declares | Form | The server checks |
|---|---|---|
| a size up to 100 MiB | one signed PutObject | object storage refuses a different size while it writes |
| a larger size, or no size | multipart, with a part size the server chooses | each declared part size, and the completed object's actual size |

Every signed PutObject binds `Content-Length`, `Content-Type` and `If-None-Match: *`.
Every signed UploadPart binds its length when supplied. The part size is 16 MiB, or
larger when the maximum size needs more than 10,000 parts. Parts upload in parallel,
and the public client retries a failed part within its existing bounded policy.

`complete_upload` completes the multipart upload create-only using the part numbers
and native ETags from ListParts, then reads the actual size with HeadObject. A size
that differs from the declaration or exceeds the maximum/reservation fails the
artifact and removes its object. Otherwise one database transaction ends the upload,
settles its reservation, records availability and retention, and appends the completion
audit record. Completion reads no object body, records no content checksum and creates
no verification Task. Repeating an ended upload returns its retained outcome.

An upload expires 24 hours after creation. The sweep then aborts its multipart upload,
removes any object that arrived and releases its reservation, and a bucket lifecycle
rule removes any multipart upload the sweep missed. Aborting an upload removes them at
once.

### Downloads

`create_download` signs a GetObject valid for 5 minutes, with
`response-content-disposition` set to an attachment named after the artifact and
`response-content-type` set to its media type. Ranges and resumed transfers go to
SeaweedFS on the same URL. A [preview](#previews) reads the object with the server's own
client instead.

### Bucket Preparation

At startup the server creates the bucket when it does not exist, sets CORS to the
installation's application origins for PUT and GET with the headers the signed URLs
bind (`content-type` and `if-none-match`), and sets a
lifecycle rule that aborts incomplete multipart uploads after one day. The server uses aws-sdk-s3 1.152.0 without default features, with rustls on ring
and without aws-config. It builds two clients from one configuration: one signs URLs
with the address clients use, because SigV4 binds the Host header, and one makes its
own calls on the installation network. Both address the bucket by path, so a signed
URL is `{signing endpoint}/{bucket}/{key}`. An installation can serve the store on a
path of its public host: with the signing endpoint `https://veoveo.example.com` and
the bucket `artifacts`, clients send `https://veoveo.example.com/artifacts/{key}`, and
the installation routes `/artifacts/` to the store with the Host, path, query and
headers unchanged.

## Access Control

The Hub admits a tool by its permission. Artifact then decides the specific
artifact, and both must allow the call. One SQL function, `artifact.effective_level`,
makes the per-artifact decision from the caller's facts, so lists filter inside the
query before their row limit.

An artifact does not exist for a caller when it belongs to another tenant, has passed
its retention, has been deleted, or carries a label outside the caller's data labels.
Otherwise the caller's level is the highest of:

- admin for the owner;
- the caller's unexpired principal grants;
- the caller's unexpired group grants, each capped by the caller's role in that group.

A project is a group: a project's artifacts are shared with its group, and joining the
group gives what is shared with it while leaving takes that away; a person's own grants
stay. Artifact decides from the groups and roles the forwarded identity names and its
own grants, and nothing else.

A tool needs read, write or admin as the table above lists. A caller below the needed
level who can see the artifact gets a refusal that names the level. A caller who cannot
see it gets the same answer as for an unknown ID.

## Sharing

| Mechanism | Reaches | Rules |
|---|---|---|
| Grant | a principal or group in the tenant | Read, write or admin, with an optional expiry. The owner's admin access cannot be revoked. |
| Share link | anyone holding the link | Read only, for a released artifact without data labels, for 1 to 30 days, with an optional redemption limit. An artifact holds at most 256 live links. The server stores only the SHA-256 of the link's secret, and of its preview's token. |
| Access request | a caller who reads an artifact and asks for write or admin | Pending, then approved, denied or cancelled. Approval writes the grant in the same transaction. |

An administrator releases an available artifact with `set_release_state`. A share link
reaches people without data labels, so an artifact that carries a label cannot be
shared by link. Returning an artifact to private revokes its live links in the same
transaction, so releasing it again revives none; revoking a link or deleting its
artifact also stops new redemptions and its preview at once. A URL already issued expires within
its 5 minutes. A redemption counts once per visit to the share route, because ranges and
resumes go to SeaweedFS or are a preview's.

A caller holds at most one pending request per artifact, and asks only for a level
above the one it holds; a caller who cannot read the artifact gets the answer for an
unknown ID. Only the requester withdraws a request, and only an administrator of the
artifact decides it. An approval gives the requester a principal grant of the requested
level without an expiry, unless a grant it already holds gives at least that level
without one, and records the grant change beside the decision. The access-request
listing holds the requests the caller made, those on artifacts it administers, or,
without a role, both, newest first.

## Knowledge

Artifact expresses the knowledge contract, `ai.veoveo/knowledge`, with the SDK's
ready-made PostgreSQL search: its template `artifact://artifacts/{artifact_id}` is the
collection `artifact.artifacts`, and `search_knowledge` finds available artifacts by
their name, their metadata and their media, under the same read rule as their listings.
A person searches with words, an image, a clip or both, as the contract allows.

- **Columns.** `artifact.artifact` carries `knowledge_text`, generated from its name,
  whole and split at `.`, `_` and `-` so `kite.png` is found by `kite`, and the string
  values of its metadata, `knowledge_vector` and `knowledge_model`, with their GIN and
  HNSW indexes, in migration `0002_knowledge.sql`. A unique index on `artifact_id`, which
  is unique across tenants, keys the SDK's reads and writes of one artifact.
- **Read rule.** `artifact.knowledge_readable(a artifact.artifact, caller jsonb)` admits
  an artifact that is available and whose `artifact.effective_level` for the caller is
  read or more, the rule every listing uses; a label outside the caller's data labels
  therefore hides it. A hit is the artifact's URI, its name as title, and a snippet of
  its media type, size and metadata.
- **Embedding.** The call that makes an artifact available, `complete_upload`, indexes
  it through the SDK after it commits: for a person's own call in a Task named in its
  result, for another server's call, such as Media saving an output, before it returns,
  with the outcome in the result's `indexed`, `{embedded, reason?}`. A retried completion
  returns the artifact without indexing it again. No tool edits a name or metadata, so
  nothing else indexes.
  Indexing signs Artifact's own GET of the object and sends Embedding one item, the
  artifact's name and metadata with its media when its type is `image/*`, `audio/*` or
  `video/*` and its size is within Embedding's source bound; any other artifact is
  embedded by its name and metadata. The GET is on Artifact's signing endpoint, the
  storage origin the installation gives Embedding's runtime, and records no download:
  it is the index's own read, not a person's. The one statement the index reads an
  artifact with names it by ID across tenants, as its marker says. Embedding is called
  for the person who made the artifact, so its usage is theirs. Until its vector is
  stored, an artifact is found by words; a failure is the Task's, or the call's outcome,
  never dropped. Storing a vector wakes the artifact's subscribers like any row change.
- **Calls.** Artifact declares use of `embedding.embeddings.create` when it registers, runs
  a Task worker for its index Tasks beside its loops, and its policy admits the Hub's
  token exchange and `embedding__embed`. Its routes declare `starts_tasks`, so it declares
  the Tasks extension; `tasks/*` serve the caller's own index Tasks, and
  `subscriptions/listen` follows them beside the caller's resources.
- **Skill.** `find-artifacts` says how to call `artifact__search_knowledge`, with words or
  with media signed through `artifact__create_download`, and that a hit is read with
  `artifact://artifacts/{artifact_id}`.

## Calls From Other Servers

A server such as Media declares uses of `artifact.artifacts.read` and
`artifact.artifacts.write` when it registers, and calls Artifact through the Hub with
a token from token exchange. The token names the original caller and an `act` chain of
the calling server and the Hub, and carries only the Artifact permissions the
calling server declared and the caller holds. Artifact authorizes the call for the
caller, records the acting server in the artifact's provenance, and names it as the
`via` of the audit record of every decision the call makes. A call that one of the
calling server's Tasks makes after its request returned also names that Task: in
`provenance.task` as the calling server identifies it, and in `via` when it is a Task
of core's Task runtime.

An idempotency key on `create_upload` is unique per tenant and acting identity. A
repeated call with the same key returns the existing upload or artifact, so a Task
that restarts after a crash does not store an output twice.

The `contract` feature publishes each served tool as a typed call. `ArtifactProvider`
names the `artifact` slug, and each request type names its tool and result, such as
`CreateUploadRequest` with `CreateUploadResult`, so a consumer calls
`client.call(request)` on the SDK's `HubClient` and never spells a tool name.

### ArtifactClient

`ArtifactClient::new(calls, http)` wraps a `HubClient`: the one `ctx.client()` binds
to a request's caller, the one `ServerCalls::for_task` binds to a Task's caller, or a
program's own. `http` moves the bytes straight to object storage, and the program
chooses its TLS configuration. `upload(name, media_type, body)` and `download(id)` are
builders that run when awaited.

An upload reads the body from an `AsyncRead`, a stream of chunks or bytes in memory:

| Body | Upload |
|---|---|
| at most 16 MiB, size known or not | The client reads it into memory, then declares its size with `create_upload`. One create-only PUT sends it, and object storage enforces the signed length. The artifact is available at completion. |
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

An upload resolves with the available artifact after native storage completion and
size validation. A failed retained outcome returns `UploadFailed`. There is no
content-hash worker or verification wait; a consumer that requires a content digest
computes and stores it separately.

A download calls `create_download` and streams the signed GET, whole or as one range.
A failure names object storage's status and S3 error code, never the signed URL.

A call bound to a request lasts as long as the forwarded token, at most a minute, so a
handler starts a Task for an upload that may take longer. A consumer reaches the
Hub's `/oauth/token` and `/mcp/{profile}`, and object storage, on the installation's
network, as Artifact does.

## Lists And Paging

Every collection uses Core's nominal `Collection` and immutable `Page<C>` through
`Pager<C>`; SDK `ReadCollection` owns the resource URI's `limit` and `cursor` query
parameters and derives their discovery template. Page bodies use `items`, `limit`
and `nextCursor`. The default is 50 entries, maximum 100 for artifacts, uploads,
share links and access requests. Grants permit 256, the existing per-artifact bound;
the store reads that bounded sequence in subject-kind/subject order and Pager
selects its page. Missing or null continuation means completion.

Artifacts order by UUIDv7 ID descending except name-prefix selections, which order
by name then ID ascending. The nominal artifact key carries both name and ID; its
filter-aware comparator agrees with the existing indexed SQL. Other SQL collections
order by their unique UUIDv7 ID descending. The separate first/next statements and
lookahead queries are unchanged. Pager admits continuation and constructs bounded
items and continuation together; owners cannot independently assemble a page DTO.

Cursors are opaque bounded tokens, at most 4096 bytes. Scope includes collection,
tenant, typed filters and limit. Artifact, upload and access-request policies also
bind the current admitted principal; nested grants and share-link filters include
the artifact parent. Changed limits, filters, principals or parents are refused;
malformed/scoped cursors require restarting without a cursor. Serialization only
admits the bounded wire token; contextual parsing happens in the owner pager.

These are live keyset listings, not retained snapshots. Inserts before an issued
position are not revisited; inserts after it may appear. Deletes disappear. Name
changes across the position can cause repeated or missed identities, so a caller
requiring a fresh view restarts. Every page evaluates current SQL visibility and
parent administration. Cursors grant no authority and cannot restore revoked access.
On unchanged data and authorization, total ordering and lookahead yield each matching
identity once, including exact boundaries. No server-local retained cursor state is
needed between replicas.

Tools and resources register through one SDK routes table. Typed request/content
associations, explicit policies and resource cache settings generate descriptors,
schemas, discovery and registration authorization together. Upload creation and
completion, and release-state changes return `Product<T>` with canonical upload or
artifact `resultUri`, typed `metadata`, and the adapter-generated matching resource
link. Batch signed parts remain a single action result. The native client unwraps
that typed metadata; no old wire decoder or compatibility page DTO is retained.
Per-rule compliance profiles and their generated manual section are removed.

## Persistence

| Table in `artifact` | Holds | Retention |
|---|---|---|
| `artifact` | ID, tenant, owner, name, media type, size, labels, release state, provenance, metadata, retention and state (`uploading`, `available`, `failed`, `deleting`) | Until deleted or its retention passes; a failed artifact, which holds no object, 7 days after it failed |
| `grant` | Artifact, subject kind and ID, level, expiry and grantor | With the artifact, or at expiry |
| `upload` | Artifact, creator, acting identity, idempotency key, form, declared size, maximum size, S3 upload ID, part size, state and expiry | 7 days after it ends |
| `share_link` | Artifact, secret digest, its preview token's digest once a browser opens it, expiry, redemption limit and count, and revocation | 30 days after expiry or revocation |
| `access_request` | Artifact, requester, level, justification, state, decider and note | 90 days after a decision |
| `storage_usage` | Per tenant: stored and reserved bytes | With the tenant |
| `upload_limits` | Per tenant: quota, largest object, allowed media types and active-upload limit. A tenant without a row has no quota, objects up to 5 TiB, any media type and 64 open uploads | With the tenant |

`artifact` also holds each artifact's [knowledge](#knowledge) columns, and the schema
holds the read rule `artifact.knowledge_readable`; pgvector's type and operators come
from the installation's `_extensions` schema, the one schema outside `artifact` the SQL
names. Index Tasks live in core's Task runtime, not in this schema.

Triggers keep `storage_usage` current as uploads reserve, complete and end and as
artifacts are deleted. `create_upload` locks the tenant's usage row while it reserves,
so two uploads cannot both take the last of a quota. A caller without a declared size
reserves its maximum size, or the tenant's largest object when it gives none, and
completion settles the reservation to the real size.

An artifact counts toward stored bytes from completion until its row goes, so a size
exists only in the `available` and `deleting` states. Deleting an artifact
marks it `deleting`, and `artifact.effective_level` hides it from every caller at once.
The sweep then claims a batch of deleted artifacts, deletes each object and then each
row, and commits. The row's grants, uploads, share links and access requests go with it
through foreign-key cascades. The same sweep ends expired uploads and fails their
artifacts, marks artifacts whose retention passed as deleting, removes each failed
artifact's object and then its row 7 days after it failed, and prunes rows past their
retention, each in short batches that `FOR UPDATE SKIP LOCKED` divides between
replicas. The server removes a failed artifact's object when it fails, and the sweep
removes it again in case the process stopped first.

Listings hold `available` artifacts. An upload's progress is read
through the uploads resource. An artifact holds at most 256 grants, and its name holds
no Unicode noncharacters, so a prefix followed by U+10FFFF bounds every name that starts
with it.

## Audit And Notifications

Every decision appends its audit record on the connection of the transaction that
makes the change, so the decision and its record commit together or not at all. Each
record carries core's envelope, and Artifact's contract declares a closed detail type
for each class with the OCSF API Activity (6003) activity the export maps it to:

| Class | Recorded when | Target | OCSF activity | Outcome |
|---|---|---|---|---|
| `artifact.upload_created` | `create_upload` reserves an artifact | the upload | Create | success |
| `artifact.upload_completed` | `complete_upload` records the bytes that arrived | the upload | Update | success, or failure with reason `size_mismatch` when the size differs from the declared one or exceeds the reservation |
| `artifact.upload_aborted` | `abort_upload` | the upload | Delete | success |
| `artifact.download_issued` | `create_download` signs a URL, before the URL is returned, or a person opens a [preview](#previews), once a visit, whose time is the most a preview's origin lives, 24 hours | the artifact | Read | success |
| `artifact.grant_changed` | `grant_artifact`, `revoke_artifact_grant` with no level, or an approved access request | the artifact's grants | Update | success |
| `artifact.deleted` | `delete_artifact` marks the artifact | the artifact | Delete | success |
| `artifact.upload_limits_set` | `set_upload_limits` | `artifact://storage-usage` | Update | success |
| `artifact.release_changed` | `set_release_state`, with the number of live share links a return to private ended | the artifact | Update | success |
| `artifact.share_link_created` | `create_share_link`, with the link's expiry and limit | the artifact's share links | Create | success |
| `artifact.share_link_revoked` | `revoke_share_link` | the artifact's share links | Delete | success |
| `artifact.share_link_redeemed` | the share route counts a redemption, before it answers; the record names no actor, and its time is the signed URL's expiry or, for a preview, the link's | the artifact | Read | success |
| `artifact.access_requested` | `request_artifact_access` | the artifact | Create | success |
| `artifact.access_request_cancelled` | `cancel_access_request` | the artifact | Update | success |
| `artifact.access_request_decided` | `decide_access_request`, with the decision; an approval that writes a grant also records `artifact.grant_changed` | the artifact | Update | success |

A target is the Artifact resource address the decision concerns, such as
`artifact://artifacts/{artifact_id}`, `artifact://uploads/{upload_id}` or
`artifact://artifacts/{artifact_id}/grants`. Artifact records the decisions it makes;
a refused request changes nothing and records nothing, so its reason codes are
`size_mismatch`. A record's actor and request come from the
Hub's signed request context, and its `via` names the server that made the call for
the caller and that server's Task, so details carry Artifact's facts only. Startup declares the classes through the server's own login with core's
`AuditStore::declare_as_server`, and each record goes through
`AuditTransactionWrite::append_as_server`, which the database confines to classes under
the `artifact.` prefix. A record states the authorization, such as a principal allowed
to download an artifact until a time. An installation that must prove each transfer
adds SeaweedFS's S3 audit log (`s3.auditLogConfig`).

Row triggers notify the `artifact` channel with each row's key and tenant, and
subscriptions wake only for their own tenant.

## Authorization

`ArtifactPermission` names the permissions in the tool table. The requirement table
maps each tool, resource template and prompt to its permission, builds the
`ai.veoveo/authorization` declaration, and drives every handler's check of the
forwarded token. A target the declaration omits is denied.

A permission's risks are Artifact's to declare: the installation's roles select by them,
and its overrides can add a risk or deny a permission but never remove one. Sharing
inside the tenant carries none. `artifact.grants.write` gives an identified person or
group of the tenant a level the caller administers, as a repository's owner shares it in
Lore, and reads no secret or personal data, which is what `sensitive` names. Every role
that writes therefore holds it: the Hub lists `grant_artifact`, `revoke_artifact_grant`
and `decide_access_request` to it, forwards `artifact.grants.write` in the token's
`authorization_details`, and Artifact's admin level on the artifact decides. A share link
keeps `sensitive`: its holder reads with no identity at all, which no role, group or level
bounds after it is issued, so publishing outside the tenant stays the installation's
decision. A group in a grant is named by the ID the Hub answers for it, and the forwarded
identity carries the caller's groups and their roles in them (Hub's [groups and
sharing](https://github.com/VeoveoAI/veoveo-hub/blob/main/crates/veoveo-hub/DESIGN.md#groups-and-sharing)).

## Registration

Artifact requires a trusted-provisioned immutable `VEOVEO_DEPLOYMENT_ID`
(`--deployment-id`), shared by registration, forwarded-token trust and Task execution.
`--describe-registration` needs only that ID, `--public-base-url` and
`--key-directory`: it prints native public ClientMetadata from `KeyDirectory::prepare`
and exits before telemetry, object storage, Hub or database initialization. The
same preparation constructs serving metadata; routes remain the authorization source.
The private deployment key and native saved registration remain in that directory.

Artifact prepares its bucket, then resumes or registers with the Hub at startup:
it loads its deployment key, discovers the registration endpoint, registers its slug,
scheme, endpoint and permissions, connects with the database login the response grants,
applies its migrations, declares its audit classes and serves. Native
`resume_or_register` validates exact metadata/key/Hub authority and reuses saved
RFC7592 credentials plus its original private database grant. On SIGTERM it stops
serving, its loops and its process instance without deleting registration. Explicit
retirement deletes it; the Hub's existing orphan grace still applies. Missing or
expired registration requires fresh trusted scoped admission, with no global initial
credential fallback. A new build/contract/endpoint uses a new admitted deployment;
ordinary process restart keeps the same deployment and key directory.

## Deployment

The image runs as UID 10001 with a read-only root filesystem. The installation runs
SeaweedFS beside PostgreSQL, pinned by digest, and configures the server through
`VEOVEO_STORAGE_ENDPOINT`, `VEOVEO_STORAGE_SIGNING_ENDPOINT` (the address clients use,
defaulting to the endpoint), `VEOVEO_STORAGE_REGION`, `VEOVEO_STORAGE_BUCKET`,
`VEOVEO_STORAGE_ACCESS_KEY`, `VEOVEO_STORAGE_SECRET_KEY` and
`VEOVEO_STORAGE_CORS_ORIGINS`, beside the registration settings every server takes.
Share links name `{veoveo_public_base}/shares/{token}`, the public base the Hub's registration
response names, which the Hub routes for people without an account, and the server accepts the base's
host in the `Host` header. Without a base they name `{origin}/artifact/shares/{token}` under the
installation's public origin, `VEOVEO_PUBLIC_ORIGIN`, or under the public base URL. The response's
`veoveo_content_origin` names the installation's content domain, without which a browser opening a
link gets the signed GET too.
SigV4 signs with the secret key inside the server, so the server's environment holds the
credential itself, the way the Hub's holds the database owner's password; a
placeholder that a proxy substitutes cannot sign. The credential is limited to the
bucket. Locally the signed address is `http://vv-seaweedfs.internal:9000`, and without a
public origin share links name `http://vv-mcp-artifact.internal:8801`, Artifact's own
name. The installation publishes both ports on the host's loopback interface, so a host
browser follows both once the host maps those names to `127.0.0.1`. Environments reach
object storage directly on the installation's network, because a proxy that changed a
signed header would void the signature.

## Contract Compliance

| Area | State |
|---|---|
| Inline content for a model without a sandbox (`write_artifact` and a content resource) | Taken up with the agents (E4), under decision D27 |
| Helm chart | Pending (C15) |

## Verification

| Layer | Checks |
|---|---|
| Unit | The checksum-free content contract, object keys, attachment dispositions, part plans, permission risks, audit schemas, the requirement table and tool annotations, the MCP setup, subscription matching and configuration |
| Database | Core's database checks over every statement and table, with pgvector created as the installation creates it; `tests/sql_confinement.rs` holds every query, migration and test query to the `artifact` schema, and pgvector's `_extensions`, with the SDK's `assert_sql_confined`; `effective_level` against a matrix of owners, principal and group grants, group roles and labels; quota reservation under concurrent uploads; paging and cursor binding; the sweep's claims |
| Object storage | A private SeaweedFS per test process, from the binary in the pinned image: signed single and multipart uploads, wrong object sizes and part lengths refused, create-only writes, idempotent completion and abort, ranged downloads and bucket preparation |
| Service | The store and SeaweedFS together: declared and undeclared uploads, refused bytes, aborts, a large unknown-size object made available without a background Task, a repeated idempotency key, download access, and the sweep's removal of abandoned, deleted and orphaned objects |
| MCP | The SDK's in-process Hub with tokens that carry a request context: discovery of the tools, which match the typed calls the contract publishes, the resources and the prompts; permission refusals on tools and resources, including every sharing tool; a signed upload counted in storage usage; upload limits; an owner who releases an artifact, creates a share link, lists it without its secret and redeems it once through the hosted router's share route; and an owner refused a request for more access |
| Sharing | Database tests: returning to private ends only live links and releasing again revives none; links need an administrator and a released, available artifact without labels; twelve concurrent redemptions of a link limited to three count exactly three; expired, revoked, deleted and unknown links redeem nothing, and a dropped redemption counts nothing; share-link pages and their cursors. Access requests: an approval writes the requested grant and records each step, visibility and the held level bound a request, only the requester withdraws and only an administrator decides, an approval keeps a stronger grant, and listings page by role and state |
| Previews | `tests/content_route.rs` through the hosted router and a private SeaweedFS, with tokens as the Hub forwards them: the read permission and the bearer are both needed; whole files, ranges, a suffix, `416`, `If-Range`, `If-None-Match` and `HEAD`; the file has one name and its top goes to its viewer; a visit records one download however many reads follow; someone else's artifact is unknown until a grant, and the next request after the grant goes is refused. A browser's visit to a link goes to a preview whose token redeems nothing, a program gets the signed GET, a visit is one redemption, the same link has the same origin; revoking, expiry and returning to private end it at once, and a limit of one lets the opened preview stay and no second visit |
| Share route | `tests/share_route.rs` through the hosted router and a private SeaweedFS: a live link redirects a program with `no-store` to a 5-minute signed GET that serves ranges; expired, revoked, private, deleted, spent, unknown and malformed tokens answer the same `404`, headers and body included; a `HEAD` answers `405` and counts nothing; the request spans of a redemption and a refusal record the route template and neither the token nor the redirect's signature |
| Knowledge | `tests/knowledge.rs` through the SDK's in-process mesh, with Artifact over a fresh database with pgvector and a private SeaweedFS, and a synthetic stand-in for Embedding that serves Embedding's `embed` contract, fetches each media part through its signed GET and answers vectors hashed from words and bytes, never a model's: a person's image upload names its index Task, which embeds the name, metadata and bytes for the uploader from that Task and is then found first by a word and by an image of its bytes; a grant to the person's group makes another person's artifact found and its revocation hides it; and Ledger's upload is embedded before its completion returns, with `indexed` in the result and both servers in the chain |
| Client | `tests/client.rs` through the SDK's in-process mesh, with Artifact over a fresh database and a private SeaweedFS, for Ledger, the SDK's calls fixture server, which declares uses of Artifact's read and write permissions: a request binding uploads a small body of unknown size in one PUT and 40 MiB of unknown size in parallel parts, which resolves available, then downloads whole, as a range across a part boundary and as a tail; a Task binding uploads 20 MiB of declared size after its request returned and resolves available; a caller without the write permission is refused; the acting server is recorded in provenance and as the audit records' `via`, and for a Task binding the Task as well; a body shorter than its declared size aborts the upload and releases its bytes. `tests/contract_boundary.rs` holds the `contract` and `client` features to their dependency boundaries |
| Audit | Records read back as the installation owner: a decision and its record commit together, a refused record rolls its decision back, every decision, sharing included, records its declared class with the resource it concerns as its target, a success outcome, its actor and detail, a redemption records no actor in the installation's partition, a size mismatch records a failure with Artifact's reason code, and a decision another server asked for names that server as `via` and keeps it out of the detail |
| Acceptance | `tests/acceptance.rs` through a running installation's Hub and SeaweedFS, one test at a time: each caller listing exactly the tools its role may call, the sharing tools among the member's, and the member refused the tools whose permissions carry a risk, a share link and a deletion among them, a declared upload available at completion, private to its owner and read as a range by the member once granted, a gibibyte of unknown size uploaded in parts, independently fingerprinted by the consumer, read as a range across a part boundary and released by the sweep after deletion, a grant listed and revoked, the admin's quota refusing an upload, a share link opened through Artifact's forward and its redirect to the object store until its limit of one is spent, and the member's request for admin on an artifact shared with them approved by the admin. The local installation has one tenant, so the database tests cover a read refused across tenants |
