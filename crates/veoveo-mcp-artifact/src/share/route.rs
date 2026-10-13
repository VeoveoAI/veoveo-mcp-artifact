//! `GET /artifact/shares/{token}`: the one route outside MCP, because a share link works
//! for someone without an account. A live link answers `303 See Other` to a fresh signed
//! GET of its artifact. Every other token, whether malformed, unknown, expired, revoked,
//! spent, or for an artifact that is private, deleted or unavailable, answers the same
//! `404`, so a refusal tells the holder nothing about the link.
use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State, rejection::PathRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;

use crate::service::ArtifactService;

/// The share route, which the hosted process serves under its mount without Gateway
/// authentication.
pub fn routes(service: Arc<ArtifactService>) -> Router {
    Router::new()
        .route("/shares/{token}", get(redeem).head(no_head))
        .with_state(service)
}

async fn redeem(
    State(service): State<Arc<ArtifactService>>,
    token: Result<Path<String>, PathRejection>,
) -> Response {
    let Ok(Path(token)) = token else {
        return refused();
    };
    match service.redeem_share_link(&token, Utc::now()).await {
        Ok(Some(get)) => {
            let Ok(location) = HeaderValue::from_str(get.url.as_str()) else {
                return unavailable();
            };
            let mut response = StatusCode::SEE_OTHER.into_response();
            response.headers_mut().insert(header::LOCATION, location);
            private(response)
        }
        Ok(None) => refused(),
        Err(error) => {
            // A root event: the request's span records its path, which holds the secret.
            tracing::warn!(parent: None, %error, "a share link redemption failed");
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
