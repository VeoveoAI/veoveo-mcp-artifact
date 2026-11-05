//! An artifact on the content domain: the routes the Hub reads, as the person who opened it or
//! for whoever holds a share link's preview. The Hub answers the viewers and the origin's own
//! policy, and these answer whether the artifact may be read now and its bytes, streamed from
//! object storage with ranges, because the signed GET of a download is an attachment on the Hub's
//! own host and would expire in the middle of a video.
mod file;
mod routes;

pub use routes::{
    LINKS_PATH, LINKS_REQUESTS_PER_MINUTE, MAX_BODY_BYTES, PATH, link_routes, routes,
};
