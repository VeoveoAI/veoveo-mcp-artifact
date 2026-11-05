---
name: share-an-artifact
description: Give people or groups read, write or admin access to an artifact, share it by an anonymous link, review who can reach it, and ask for or decide access requests. Use when a person wants others to reach a file, or asks who can.
---

# Share an artifact

An artifact is private to its owner until shared. Sharing is always a separate call, so a
failed share leaves it private. A project is a group: share a project's artifacts with
its group, and whoever joins the group reaches them.

## Levels

A caller's level on an artifact is the highest of: `admin` for its owner, their own
unexpired grants, and their groups' unexpired grants, each capped by their role in that
group: a group's `member` writes at most and its `owner` administers, and a group from
the identity provider reads at most. An artifact carrying a
label the caller does not hold does not exist for them, whatever the grants say.

- `read`: metadata and downloads;
- `write`: also changes to its content;
- `admin`: also grants, release state, share links, access requests and deletion.

## Grant

- `artifact__grant_artifact` gives a principal or a group (by its `id`, which
  `console__groups` lists) `read`, `write` or `admin`, with an optional expiry. A later
  grant to the same subject replaces it.
- `artifact__revoke_artifact_grant` removes one. The owner's admin access cannot be
  revoked.
- `artifact://artifacts/{artifact_id}/grants` lists them, for administrators.

## Share by link

For someone without an account, and only for an artifact without labels:

1. `artifact__set_release_state` to `released`.
2. `artifact__create_share_link`, living 1 to 30 days, with an optional redemption limit.
   Its URL holds the link's secret and is shown only once: hand it to the person
   directly.
3. Opened in a browser, the link previews the file in its viewer, which offers a download;
   any other caller gets the bytes through a short-lived signed URL. Each counts one redemption.
4. `artifact__revoke_share_link` stops a link and its preview at once; `artifact__set_release_state` back
   to `private` ends every live link.

## Review who can reach it

Read the artifact, its grants and `artifact://artifacts/{artifact_id}/share-links`. Report
who can reach it and through which grant or link. Flag grants without an expiry to
groups, links that live longer than needed or near their redemption limit, and a released
artifact nobody shares by link. Propose `artifact__revoke_artifact_grant`,
`artifact__revoke_share_link` or a return to `private`, and call none of them until the
person agrees.

## Access requests

- A caller who can read an artifact asks its administrators for `write` or `admin` with
  `artifact__request_artifact_access` and a justification, and withdraws it with
  `artifact__cancel_access_request`. They hold at most one pending request per artifact.
- An administrator reads `artifact://access-requests?role=decider&state=pending`,
  following `nextCursor` to the end, weighs each requested level against its
  justification and the artifact's labels, presents a recommendation, and calls
  `artifact__decide_access_request` with `approve` or `deny` and a note only for the
  requests the person decides. Approval writes the grant.

`artifact__delete_artifact` deletes an artifact with its grants, links and requests; its
bytes go shortly after. Ask before deleting.
