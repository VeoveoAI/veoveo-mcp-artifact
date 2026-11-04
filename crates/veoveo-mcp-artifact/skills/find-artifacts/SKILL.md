---
name: find-artifacts
description: Search the artifacts the person you work for may read by words, by an image, a clip or both, and read a hit. Use when a task needs a stored file you cannot name exactly.
---

# Find artifacts

## Search

Call `artifact__search_knowledge` with `parts`: 1 to 16 ordered parts, each `text` with
its `text`, or `image`, `audio` or `video` with a signed GET of the media: `url`, any
`headers` it binds, `mediaType` and `size` in bytes. Words and media may be combined; the
search answers any of them. Add `limit`, 1 to 20 (10 by default), and
`collections: ["artifact.artifacts"]` when you search several servers' collections.

- Words find an artifact by its name, split at `.`, `_` and `-`, so `kite` finds
  `kite.png`, and by the text of its metadata.
- Media find artifacts whose own image, audio or video is alike. To search with an
  artifact you already have, call `artifact__create_download` for it and pass its
  `get.url`, `get.headers`, media type and size as the part, within the URL's 5 minutes.

Each hit names the artifact by `artifact://artifacts/{artifact_id}`, with its name as the
title and a snippet of its media type, size and metadata, best first. Only artifacts the
person may read appear, and only available ones; a page is never short for others the
person may not see.

## Read a hit

Read `artifact://artifacts/{artifact_id}` for the artifact's metadata and fetch its bytes
with the skill `download-an-artifact`.

## Fresh uploads

An artifact is found by its words as soon as its upload completes. Its media are found
once its index Task finishes: `artifact__complete_upload` names that Task in its result's
`_meta["io.modelcontextprotocol/related-task"]`, and you can follow it like any of the
person's Tasks. When another server completes the upload, the result's `indexed` says
whether the artifact's media were embedded and, if not, why.

Never paste a signed URL into a chat or a file another person reads.
