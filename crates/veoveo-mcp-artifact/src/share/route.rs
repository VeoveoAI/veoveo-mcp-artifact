//! `GET /artifact/shares/{token}`: a share link works for someone without an account. A live
//! link answers `303 See Other`: a browser opening it to the link's preview on the content
//! domain, and any other caller to a fresh signed GET of its artifact. Every other token,
//! whether malformed, unknown, expired, revoked, spent, or for an artifact that is private,
//! deleted or unavailable, answers the same `404`, so a refusal tells the holder nothing about
//! the link.
use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State, rejection::PathRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;

use crate::service::ArtifactService;

/// The prefix of the share route under the mount, `{mount}/shares/`, which the routes
/// declare public.
pub const PATH: &str = "shares/";
/// A redemption is a `GET` without a body, so the declared limit is the least one.
pub const MAX_BODY_BYTES: u64 = 1;
/// Redemptions a minute the Hub admits to the share route, across every link.
pub const REQUESTS_PER_MINUTE: u32 = 600;

/// The share route, which the hosted process serves under its mount without Hub
/// authentication.
pub fn routes(service: Arc<ArtifactService>) -> Router {
    Router::new()
        .route("/shares/{token}", get(redeem).head(no_head))
        .with_state(service)
}

/// Whether a browser is opening the link as a page, rather than a program fetching it.
fn document_navigation(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-mode")
        .is_some_and(|value| value == "navigate")
        && headers
            .get("sec-fetch-dest")
            .is_some_and(|value| value == "document")
}

async fn redeem(
    State(service): State<Arc<ArtifactService>>,
    headers: HeaderMap,
    token: Result<Path<String>, PathRejection>,
) -> Response {
    let Ok(Path(token)) = token else {
        return refused();
    };
    let location = if document_navigation(&headers) && service.previews_links() {
        service
            .preview_share_link(&token, Utc::now())
            .await
            .map(|url| url.map(String::from))
    } else {
        service
            .redeem_share_link(&token, Utc::now())
            .await
            .map(|get| get.map(|get| get.url.as_str().to_owned()))
    };
    match location {
        Ok(Some(location)) => {
            let Ok(location) = HeaderValue::from_str(&location) else {
                return unavailable();
            };
            let mut response = StatusCode::SEE_OTHER.into_response();
            response.headers_mut().insert(header::LOCATION, location);
            private(response)
        }
        Ok(None) => refused(),
        Err(error) => {
            // The request's span records the route template, never the token.
            tracing::warn!(%error, "a share link redemption failed");
            unavailable()
        }
    }
}

/// A HEAD request would count a redemption without receiving the URL, so the route
/// answers only GET.
async fn no_head() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET"));
    private(response)
}

fn refused() -> Response {
    private((StatusCode::NOT_FOUND, "no such share link").into_response())
}

fn unavailable() -> Response {
    private(
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "share links are unavailable",
        )
            .into_response(),
    )
}

/// Neither the answer nor the page that linked here is kept or passed on.
fn private(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
