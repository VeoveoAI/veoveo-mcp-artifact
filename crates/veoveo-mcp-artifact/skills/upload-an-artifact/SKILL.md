---
name: upload-an-artifact
description: Store a file as an artifact that the person you work for owns, sending its bytes straight to object storage in one PUT or in parts. Use whenever a task produces a file to keep or share, from a few bytes to terabytes.
---

# Upload an artifact

Bytes never travel through MCP: Artifact signs requests, and you send the bytes to
object storage yourself. The artifact belongs to the person you work for, is private to
them until they share it, and they administer it.

1. Call `artifact__create_upload` with the `name` and `mediaType`, and the `size` when
   you know it (otherwise a `maxSize`). Optionally give `labels`, `retentionDays`,
   `metadata`, the artifacts it `derivedFrom`, and an `idempotencyKey`: a repeated call
   with the same key returns the same upload, so a retried or restarted task never stores
   the file twice.
2. Send the bytes as the answer says:
   - one signed PUT: send it with exactly its method, URL and headers;
   - a multipart upload: send each part to its signed URL with its headers. Call
     `artifact__sign_upload_parts` for the next 1 to 100 parts, and again for parts whose
     URLs expired, giving each part's size when you know it.
3. Call `artifact__complete_upload`. Stored bytes that pass size validation make the
   artifact `available`; completing it again returns the artifact.
4. If you give up, call `artifact__abort_upload` to release the reserved bytes.

## Labels and limits

- Every label you give must be one of the person's data labels; the artifact then exists
  only for callers who hold all its labels. An artifact with a label cannot be shared by
  link.
- The tenant's quota, largest object, allowed media types and open-upload limit apply;
  `artifact://storage-usage` shows stored and reserved bytes against the quota.
- `artifact://uploads` lists the person's uploads and their state.

To let others read or change the artifact, follow the skill `share-an-artifact`.
