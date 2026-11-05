---
name: download-an-artifact
description: Find artifacts the person you work for may read and fetch their bytes through a short-lived signed URL, whole or by range. Use when a task needs a stored file's content or metadata.
---

# Download an artifact

## Find it

- `artifact://artifacts` lists the artifacts the person may read, newest first. Filter by
  one of `owner`, `media_type`, `name` (a prefix) or `release_state`, and follow
  `nextCursor` to read further pages.
- `artifact://artifacts/{artifact_id}` reads one artifact's metadata: name, media type,
  size, labels, owner, release state and provenance (who made it, through which server
  and Task, and what it derives from).

An artifact the person may not read looks the same as one that does not exist.

## Fetch its bytes

1. Call `artifact__create_download` with the artifact's ID. It needs read access and an
   `available` artifact.
2. Fetch `get.url` with `get.method` and the unchanged `get.headers` within 5 minutes.
   The download names the artifact as an attachment with its media type.
3. Use HTTP ranges or resume a transfer on the same URL; ask for a new URL once it
   expires.

Never paste a signed URL into a chat or a file another person reads: anyone holding it can
fetch the bytes until it expires. To give someone lasting access, follow the skill
`share-an-artifact`.

## Show a file to a person

A signed URL downloads. To show a person a file, give them the address of its page on the Hub,
`/view/artifact/{artifact_id}/{name}`: it opens in a viewer chosen by its name, for whoever may
read it, and an HTML file runs as itself, apart from everything else they can reach.
