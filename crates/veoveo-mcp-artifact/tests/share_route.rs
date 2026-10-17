//! The share route as the hosted process serves it, at `/artifact/shares/{token}` without
//! Hub authentication, over a private SeaweedFS: a live link redirects to a fresh signed
//! GET, and every refused link answers the same `404`.
mod support;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
    response::Response,
};
use chrono::{TimeDelta, Utc};
use support::{
    Who,
    seaweedfs::send,
    service::{Harness, owner, sha256, with_service},
    upload_request,
};
use tower::ServiceExt as _;
use veoveo_hub_sdk::hosting::{Hosted, ListenOnly, testing};
use veoveo_mcp_artifact::{
    contract::{
        ArtifactId, CreateShareLinkRequest, ObjectSize, ReleaseState, ShareLinkDays, UploadTransfer,
    },
    mcp::{ArtifactApp, ArtifactMcp, ArtifactSubscriptions, TenantHubs},
    share::{self, ShareLinkSecret},
};
use veoveo_types::Checked;

fn router(harness: &Harness) -> axum::Router {
    let app = Arc::new(ArtifactApp {
        service: (*harness.service).clone(),
        hubs: Arc::new(TenantHubs::default()),
    });
    testing::for_domain::<ArtifactMcp>()
        .handler(move || {
            Hosted::new(ArtifactMcp::new(app.clone()))
                .with_tasks(ListenOnly::new(ArtifactSubscriptions::new(app.clone())))
        })
        .public_routes(share::routes(harness.service.clone()))
        .build()
        .into_router()
}

async fn visit(router: &axum::Router, method: Method, path: &str) -> Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", testing::TEST_HOST)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn redeem(router: &axum::Router, token: &str) -> Response {
    visit(router, Method::GET, &format!("/artifact/shares/{token}")).await
}

/// An available artifact Ada owns, uploaded through a signed PUT.
async fn uploaded(harness: &Harness, name: &str, body: &[u8]) -> ArtifactId {
    let ada = Who::new("ada");
    let caller = ada.caller();
    let mut request = upload_request(name, None);
    request.size = Some(ObjectSize::new(body.len() as u64).unwrap());
    request.sha256 = Some(sha256(body));
    let created = harness
        .service
        .create_upload(&caller, &Checked::new(request).unwrap(), Utc::now())
        .await
        .unwrap();
    let Some(UploadTransfer::Single { put }) = &created.transfer else {
        panic!("a small declared upload takes one PUT");
    };
    assert!(send(put, body.to_vec(), &[]).await.status().is_success());
    harness
        .service
        .complete_upload(&caller, &owner(&ada), created.upload.upload_id, Utc::now())
        .await
        .unwrap()
        .artifact_id
}

/// A released artifact and the token of a link to it, created `age` ago for `days`,
/// with an optional redemption limit.
async fn shared(
    harness: &Harness,
    artifact: ArtifactId,
    age: TimeDelta,
    days: u8,
    limit: Option<u32>,
) -> (String, veoveo_mcp_artifact::contract::ShareLinkId) {
    let caller = Who::new("ada").caller();
    harness
        .service
        .set_release_state(&caller, artifact, ReleaseState::Released, Utc::now())
        .await
        .unwrap();
    let created = harness
        .service
        .create_share_link(
            &caller,
            &CreateShareLinkRequest {
                artifact_id: artifact,
                expires_in_days: ShareLinkDays::new(days).unwrap(),
                redemption_limit: limit.map(|limit| std::num::NonZeroU32::new(limit).unwrap()),
            },
            Utc::now() - age,
        )
        .await
        .unwrap();
    let token = created.url.expose().rsplit('/').next().unwrap().to_owned();
    (token, created.share_link.share_link_id)
}

#[tokio::test]
async fn a_live_link_redirects_to_a_short_signed_get_that_serves_ranges() {
    with_service(|harness| async move {
        let body: Vec<u8> = (0..=255u8).cycle().take(64 * 1024).collect();
        let artifact = uploaded(&harness, "data.bin", &body).await;
        let (token, _) = shared(&harness, artifact, TimeDelta::zero(), 1, None).await;
        let router = router(&harness);

        let redirected = redeem(&router, &token).await;
        assert_eq!(redirected.status(), StatusCode::SEE_OTHER);
        let headers = redirected.headers();
        assert_eq!(headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        let location = headers[header::LOCATION].to_str().unwrap().to_owned();
        assert!(
            location.contains("X-Amz-Expires=300"),
            "a redemption signs a GET for 5 minutes"
        );
        assert!(!location.contains(&token), "the download names no secret");

        let ranged = reqwest::Client::new()
            .get(&location)
            .header("range", "bytes=1000-1999")
            .send()
            .await
            .unwrap();
        assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(ranged.bytes().await.unwrap().as_ref(), &body[1000..2000]);
        // Each visit signs a fresh GET.
        assert_eq!(
            redeem(&router, &token).await.status(),
            StatusCode::SEE_OTHER
        );
    })
    .await;
}

#[tokio::test]
async fn every_refused_link_answers_the_same_not_found() {
    with_service(|harness| async move {
        let caller = Who::new("ada").caller();
        let router = router(&harness);
        let mut refused = Vec::new();

        let expired = uploaded(&harness, "expired.txt", b"expired").await;
        refused.push(
            shared(&harness, expired, TimeDelta::days(2), 1, None)
                .await
                .0,
        );

        let revoked = uploaded(&harness, "revoked.txt", b"revoked").await;
        let (token, link) = shared(&harness, revoked, TimeDelta::zero(), 1, None).await;
        harness
            .service
            .revoke_share_link(&caller, revoked, link, Utc::now())
            .await
            .unwrap();
        refused.push(token);

        let private = uploaded(&harness, "private.txt", b"private").await;
        let (token, _) = shared(&harness, private, TimeDelta::zero(), 1, None).await;
        harness
            .service
            .set_release_state(&caller, private, ReleaseState::Private, Utc::now())
            .await
            .unwrap();
        refused.push(token);

        let deleted = uploaded(&harness, "deleted.txt", b"deleted").await;
        let (token, _) = shared(&harness, deleted, TimeDelta::zero(), 1, None).await;
        harness
            .service
            .delete_artifact(&caller, deleted, Utc::now())
            .await
            .unwrap();
        refused.push(token);

        let spent = uploaded(&harness, "spent.txt", b"spent").await;
        let (token, _) = shared(&harness, spent, TimeDelta::zero(), 1, Some(1)).await;
        assert_eq!(
            redeem(&router, &token).await.status(),
            StatusCode::SEE_OTHER
        );
        refused.push(token);

        // A well-formed token no link was issued for, and malformed ones.
        refused.push(ShareLinkSecret::generate().unwrap().token());
        refused.extend(
            [
                "short",
                "!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA%2F",
            ]
            .map(str::to_owned),
        );

        let reference = redeem(&router, &refused[0]).await;
        let reference_headers = reference.headers().clone();
        let reference_body = axum::body::to_bytes(reference.into_body(), usize::MAX)
            .await
            .unwrap();
        for token in &refused {
            let response = redeem(&router, token).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{token}");
            assert_eq!(response.headers(), &reference_headers, "{token}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(body, reference_body, "{token}");
        }
        assert_eq!(reference_headers[header::CACHE_CONTROL], "no-store");
    })
    .await;
}

#[tokio::test]
async fn a_head_request_is_refused_without_counting_a_redemption() {
    with_service(|harness| async move {
        let artifact = uploaded(&harness, "once.txt", b"once").await;
        let (token, _) = shared(&harness, artifact, TimeDelta::zero(), 1, Some(1)).await;
        let router = router(&harness);
        let head = visit(&router, Method::HEAD, &format!("/artifact/shares/{token}")).await;
        assert_eq!(head.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(head.headers()[header::ALLOW], "GET");
        // The one redemption the link allows is still there.
        assert_eq!(
            redeem(&router, &token).await.status(),
            StatusCode::SEE_OTHER
        );
        assert_eq!(
            redeem(&router, &token).await.status(),
            StatusCode::NOT_FOUND
        );
    })
    .await;
}

/// Trace output written to memory, so a test can read what the request spans recorded.
#[derive(Clone, Default)]
struct CapturedTrace(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedTrace {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedTrace {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[tokio::test]
async fn request_spans_record_the_share_route_template_never_its_secret() {
    use tracing::instrument::WithSubscriber as _;

    with_service(|harness| async move {
        let artifact = uploaded(&harness, "traced.txt", b"traced").await;
        let (token, _) = shared(&harness, artifact, TimeDelta::zero(), 1, None).await;
        let router = router(&harness);
        let captured = CapturedTrace::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .without_time()
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
            .with_writer(move || writer.clone())
            .finish();
        let (redirected, refused) = async {
            (
                redeem(&router, &token).await,
                redeem(&router, &format!("{token}x")).await,
            )
        }
        .with_subscriber(subscriber)
        .await;
        assert_eq!(redirected.status(), StatusCode::SEE_OTHER);
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        let location = redirected.headers()[header::LOCATION].to_str().unwrap();
        let signature = location
            .split(['?', '&'])
            .find_map(|pair| pair.strip_prefix("X-Amz-Signature="))
            .expect("the redirect is a signed GET");

        let trace = captured.text();
        assert!(
            trace.contains("http.route=/artifact/shares/{token}"),
            "the span lacks the share route's template: {trace}"
        );
        assert!(
            !trace.contains(&token),
            "a request span recorded the link's secret"
        );
        assert!(
            !trace.contains(signature),
            "a request span recorded the signed GET"
        );
    })
    .await;
}
