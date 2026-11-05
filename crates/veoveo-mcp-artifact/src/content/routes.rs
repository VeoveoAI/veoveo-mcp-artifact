//! The Hub's reads of an artifact, under the mount:
//!
//! - `{artifact}` and `{artifact}/{name}`, behind Hub authentication, as the person, whose read
//!   level decides each request as it decides `create_download`.
//! - `content-links/{token}` and `content-links/{token}/{name}`, a public route for the preview of
//!   a share link, whose token names the link. Each request checks the link again, and each refusal
//!   is one `404`. The Hub serves this route only on the link's own origin.
use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, head},
};
use chrono::Utc;
use veoveo_hub_sdk::{HubInternalIdentity, registration::ContentLinkToken};

use super::file::{self, StoredFile};
use crate::{
    contract::{ArtifactId, ArtifactName, ArtifactPermission, ArtifactRecord},
    mcp::identity,
    service::{ArtifactError, ArtifactService},
    share::preview_digest,
    storage::ObjectKey,
    store::{ArtifactStoreError, Caller, PreviewedArtifact},
};

/// The route of a person's reads under the mount.
pub const PATH: &str = "content/";
/// The public route of a link's preview under the mount.
pub const LINKS_PATH: &str = "content-links/";
/// A read is a `GET` or `HEAD` without a body, so the declared limit is the least one.
pub const MAX_BODY_BYTES: u64 = 1;
/// Reads a minute the Hub admits to the links' route, across every link: the most a route may
/// declare, because one page of a preview takes many.
pub const LINKS_REQUESTS_PER_MINUTE: u32 = 6000;

/// The person's reads, among the server's authenticated routes.
pub fn routes(service: Arc<ArtifactService>) -> Router {
    Router::new()
        .route("/content/{artifact}", head(admit).post(visit))
        .route("/content/{artifact}/", get(person_top))
        .route("/content/{artifact}/{*name}", get(person_file))
        .with_state(service)
}

/// A link's reads, among the server's public routes.
pub fn link_routes(service: Arc<ArtifactService>) -> Router {
    Router::new()
        .route("/content-links/{token}", head(link_check))
        .route("/content-links/{token}/", get(link_top))
        .route("/content-links/{token}/{*name}", get(link_file))
        .with_state(service)
}

/// The caller of a forwarded identity that holds the read permission.
fn reader(identity: &HubInternalIdentity) -> Result<Caller, StatusCode> {
    if !identity.grants(&ArtifactPermission::ArtifactsRead.name()) {
        return Err(StatusCode::FORBIDDEN);
    }
    identity::caller(identity).map_err(|_| StatusCode::FORBIDDEN)
}

/// A refusal keeps what it refuses to itself: a caller who cannot see an artifact gets the answer
/// for an unknown ID.
fn refusal(error: &ArtifactError) -> Response {
    match error {
        ArtifactError::Store(
            ArtifactStoreError::ArtifactNotFound | ArtifactStoreError::ArtifactState { .. },
        ) => StatusCode::NOT_FOUND.into_response(),
        ArtifactError::Store(ArtifactStoreError::Forbidden { .. }) => {
            StatusCode::FORBIDDEN.into_response()
        }
        failure => {
            tracing::warn!(error = %failure, "a preview could not be decided");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// The relative address of the viewer of a file named `name`, on its own origin.
fn to_viewer(name: &ArtifactName) -> Response {
    let mut url = url::Url::parse("https://origin.invalid/_veoveo/view").expect("a fixed URL");
    url.path_segments_mut()
        .expect("an https URL has a path")
        .push(name.as_str());
    let Ok(location) = HeaderValue::from_str(url.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response
}

async fn admit(
    State(service): State<Arc<ArtifactService>>,
    Extension(identity): Extension<HubInternalIdentity>,
    Path(artifact): Path<ArtifactId>,
) -> Response {
    let caller = match reader(&identity) {
        Ok(caller) => caller,
        Err(refused) => return refused.into_response(),
    };
    match service.readable(&caller, artifact, Utc::now()).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(&error),
    }
}

async fn visit(
    State(service): State<Arc<ArtifactService>>,
    Extension(identity): Extension<HubInternalIdentity>,
    Path(artifact): Path<ArtifactId>,
) -> Response {
    let caller = match reader(&identity) {
        Ok(caller) => caller,
        Err(refused) => return refused.into_response(),
    };
    match service.open_preview(&caller, artifact, Utc::now()).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => refusal(&error),
    }
}

async fn person_top(
    State(service): State<Arc<ArtifactService>>,
    Extension(identity): Extension<HubInternalIdentity>,
    Path(artifact): Path<ArtifactId>,
) -> Response {
    let caller = match reader(&identity) {
        Ok(caller) => caller,
        Err(refused) => return refused.into_response(),
    };
    match service.readable(&caller, artifact, Utc::now()).await {
        Ok(record) => to_viewer(&record.name),
        Err(error) => refusal(&error),
    }
}

async fn person_file(
    State(service): State<Arc<ArtifactService>>,
    Extension(identity): Extension<HubInternalIdentity>,
    Path((artifact, name)): Path<(ArtifactId, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let caller = match reader(&identity) {
        Ok(caller) => caller,
        Err(refused) => return refused.into_response(),
    };
    let record: ArtifactRecord = match service.readable(&caller, artifact, Utc::now()).await {
        Ok(record) => record,
        Err(error) => return refusal(&error),
    };
    // The path is the artifact's name, which is what the viewers go by, and nothing else is here.
    let Some(size) = record.size.filter(|_| record.name.as_str() == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let stored = StoredFile {
        key: ObjectKey::for_artifact(caller.tenant(), artifact),
        artifact_id: artifact,
        media_type: &record.media_type,
        size,
    };
    file::answer(service.storage(), stored, &method, &headers).await
}

/// The one answer of a link that cannot be read.
fn no_link() -> Response {
    (StatusCode::NOT_FOUND, "no such link").into_response()
}

/// What the preview `token` names, while its link is live.
async fn linked(
    service: &ArtifactService,
    token: &str,
) -> Result<Option<PreviewedArtifact>, StatusCode> {
    let Some(token) = ContentLinkToken::parse(token) else {
        return Ok(None);
    };
    service
        .store()
        .share_link_preview(&preview_digest(&token), Utc::now())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "a share link's preview could not be read");
            StatusCode::SERVICE_UNAVAILABLE
        })
}

async fn link_check(
    State(service): State<Arc<ArtifactService>>,
    Path(token): Path<String>,
) -> Response {
    match linked(&service, &token).await {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => no_link(),
        Err(failure) => failure.into_response(),
    }
}

async fn link_top(
    State(service): State<Arc<ArtifactService>>,
    Path(token): Path<String>,
) -> Response {
    match linked(&service, &token).await {
        Ok(Some(artifact)) => to_viewer(&artifact.name),
        Ok(None) => no_link(),
        Err(failure) => failure.into_response(),
    }
}

async fn link_file(
    State(service): State<Arc<ArtifactService>>,
    Path((token, name)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let artifact = match linked(&service, &token).await {
        Ok(Some(artifact)) if artifact.name.as_str() == name => artifact,
        Ok(_) => return no_link(),
        Err(failure) => return failure.into_response(),
    };
    let stored = StoredFile {
        key: ObjectKey::for_artifact(artifact.tenant, artifact.artifact_id),
        artifact_id: artifact.artifact_id,
        media_type: &artifact.media_type,
        size: artifact.size,
    };
    file::answer(service.storage(), stored, &method, &headers).await
}
