//! An artifact on the content domain, through the hosted router over a private SeaweedFS: the
//! Hub's reads as the person, whose read level decides each request, and the reads of a share
//! link's preview, whose link decides each request. The test user is `user-1` of `tenant-a`.
mod support;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
    response::Response,
};
use chrono::{TimeDelta, Utc};
use support::{
    Who,
    seaweedfs::send,
    service::{Harness, with_service},
    upload_request,
};
use tower::ServiceExt as _;
use veoveo_audit::AuditStore;
use veoveo_audit_contract::{AuditPartition, AuditQuery, AuditReadScope, AuditRequest};
use veoveo_hub_sdk::{
    AccessTokenSubject, ForwardedRequest, HubInternalTokenIssuer, HubProfileId, HubRequestContext,
    OAuthClientId, ProtectedResourceId, TokenIssuer,
    hosting::testing,
    registration::{ContentOrigin, ResourceIndicator},
};
use veoveo_mcp_artifact::{
    content,
    contract::{
        ArtifactId, ArtifactPermission, CreateShareLinkRequest, GrantArtifactRequest, ObjectSize,
        ReleaseState, ShareLinkDays, ShareLinkId, UploadTransfer, audit_registry,
    },
    mcp::{ArtifactApp, ArtifactMcp, TenantHubs},
    share,
};
use veoveo_types::{AccessLevel, AccessSubject, Checked, InvocationMode, PrincipalId, TenantId};

const USER: &str = "https://idp.example.com#user-1";

/// A forwarded token for the test user with its request context, as the Hub sends one.
fn token(granted: &[ArtifactPermission]) -> String {
    let principal = testing::principal();
    let authority = testing::authority();
    let now = Utc::now();
    let context = HubRequestContext {
        execution: testing::execution_context(),
        audit: AuditRequest::background(),
        access_token: AccessTokenSubject {
            issuer: principal.issuer.clone(),
            subject: principal.subject.clone(),
            oauth_client_id: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
            session_family: None,
            audience: ProtectedResourceId::parse(testing::TEST_RESOURCE).unwrap(),
            invocation_mode: InvocationMode::Direct,
            initiator: Some(principal.id.clone()),
            delegation_id: None,
            scopes: principal.scopes.clone(),
            jwt_id: None,
            issued_at: now,
            not_before: None,
            expires_at: now + TimeDelta::minutes(5),
        },
        principal: principal.clone(),
    };
    HubInternalTokenIssuer::new(
        TokenIssuer::parse(testing::TEST_ISSUER).unwrap(),
        testing::signing_key(),
    )
    .unwrap()
    .issue(ForwardedRequest {
        deployment: testing::deployment_id(),
        profile: HubProfileId::parse("operations").unwrap(),
        server: veoveo_mcp_artifact::server(),
        audience: ResourceIndicator::parse(testing::TEST_RESOURCE).unwrap(),
        client: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
        actor: principal,
        authority,
        permissions: granted.iter().map(|permission| permission.name()).collect(),
        request_context: Some(context),
        acting_server: None,
        acting_task: None,
        delegation_context: None,
        expires_at: now + TimeDelta::minutes(5),
    })
    .unwrap()
    .bearer_token
}

fn router(harness: &Harness) -> axum::Router {
    let service = Arc::new((*harness.service).clone().with_content_origin(Some(
        ContentOrigin::parse("https://content.example").unwrap(),
    )));
    let app = Arc::new(ArtifactApp {
        service: (*service).clone(),
        hubs: Arc::new(TenantHubs::default()),
        tasks: harness.tasks.clone(),
        library: support::library(),
        pictures: None,
    });
    testing::for_domain::<ArtifactMcp>()
        .handler(move || veoveo_mcp_artifact::mcp::hosted(app.clone()))
        .authenticated_routes(content::routes(service.clone()))
        .public_routes(share::routes(service.clone()).merge(content::link_routes(service)))
        .build()
        .into_router()
}

async fn request(
    router: &axum::Router,
    method: Method,
    path: &str,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
) -> Response {
    let mut built = Request::builder()
        .method(method)
        .uri(path)
        .header("host", testing::TEST_HOST);
    if let Some(bearer) = bearer {
        built = built.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    for (name, value) in headers {
        built = built.header(*name, *value);
    }
    router
        .clone()
        .oneshot(built.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> &str {
    headers[name].to_str().unwrap()
}

/// An available artifact `who` owns, uploaded through a signed PUT.
async fn uploaded(harness: &Harness, who: &Who, name: &str, body: &[u8]) -> ArtifactId {
    let caller = who.caller();
    let mut upload = upload_request(name, None);
    upload.size = Some(ObjectSize::new(body.len() as u64).unwrap());
    let created = harness
        .service
        .create_upload(&caller, &Checked::new(upload).unwrap(), Utc::now())
        .await
        .unwrap();
    let Some(UploadTransfer::Single { put }) = &created.transfer else {
        panic!("a small declared upload takes one PUT");
    };
    assert!(send(put, body.to_vec(), &[]).await.status().is_success());
    harness
        .service
        .complete_upload(&caller, created.upload.upload_id, Utc::now())
        .await
        .unwrap()
        .artifact_id
}

/// How many records of `class` the audit log holds, of `tenant` or else of the installation.
async fn recorded(harness: &Harness, tenant: Option<&str>, class: &str) -> usize {
    let reader = AuditStore::new(
        harness.db.connect().await,
        Arc::new(audit_registry().unwrap()),
    );
    let tenant = tenant.map(|tenant| TenantId::parse(tenant).unwrap());
    let mut query = AuditQuery::new(match &tenant {
        Some(tenant) => AuditPartition::Tenant(tenant.clone()),
        None => AuditPartition::Installation,
    });
    query.limit = 1000;
    reader
        .page(
            &AuditReadScope::new(tenant.clone(), tenant.is_none()),
            &query,
        )
        .await
        .unwrap()
        .records
        .into_iter()
        .filter(|record| record.draft.class().to_string() == class)
        .count()
}

/// A released artifact's link, created `age` ago for `days`, and the token of its URL.
async fn shared(
    harness: &Harness,
    artifact: ArtifactId,
    who: &Who,
    age: TimeDelta,
    days: u8,
    limit: Option<u32>,
) -> (String, ShareLinkId) {
    let caller = who.caller();
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

const BROWSER: [(&str, &str); 2] = [
    ("sec-fetch-mode", "navigate"),
    ("sec-fetch-dest", "document"),
];

/// A browser opens the link: the token of its preview, from where it is sent.
async fn opened(router: &axum::Router, link: &str) -> String {
    let answer = request(
        router,
        Method::GET,
        &format!("/artifact/shares/{link}"),
        None,
        &BROWSER,
    )
    .await;
    assert_eq!(answer.status(), StatusCode::SEE_OTHER);
    let location = header_value(answer.headers(), header::LOCATION).to_owned();
    let host = location
        .strip_prefix("https://artifact-")
        .and_then(|rest| rest.strip_suffix(".content.example/"))
        .unwrap_or_else(|| panic!("a preview's origin: {location}"));
    assert_eq!(host.len(), 40, "{location}");
    host.to_owned()
}

#[tokio::test]
async fn the_person_reads_what_their_level_lets_them_with_ranges_and_validators() {
    with_service(|harness| async move {
        let body: Vec<u8> = (0..=255u8).cycle().take(64 * 1024).collect();
        let user = Who::new(USER).tenant("tenant-a");
        let artifact = uploaded(&harness, &user, "a clip.mp4", &body).await;
        let router = router(&harness);
        let read = token(&[ArtifactPermission::ArtifactsRead]);
        let item = format!("/artifact/content/{artifact}");
        let file = format!("{item}/a%20clip.mp4");

        // The Hub's bearer and the read permission are both needed.
        let admit = request(&router, Method::HEAD, &item, None, &[]).await;
        assert_eq!(admit.status(), StatusCode::UNAUTHORIZED);
        let no_read = token(&[ArtifactPermission::ArtifactsWrite]);
        let refused = request(&router, Method::HEAD, &item, Some(&no_read), &[]).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let admit = request(&router, Method::HEAD, &item, Some(&read), &[]).await;
        assert_eq!(admit.status(), StatusCode::NO_CONTENT);

        // A whole file, with its own type and identity.
        let whole = request(&router, Method::GET, &file, Some(&read), &[]).await;
        assert_eq!(whole.status(), StatusCode::OK);
        let headers = whole.headers().clone();
        assert_eq!(header_value(&headers, header::ACCEPT_RANGES), "bytes");
        assert_eq!(
            header_value(&headers, header::CONTENT_LENGTH),
            body.len().to_string()
        );
        let tag = format!("\"{artifact}\"");
        assert_eq!(header_value(&headers, header::ETAG), tag);
        assert!(!headers.contains_key(header::CONTENT_DISPOSITION));
        assert_eq!(bytes(whole).await, body);

        // One range, with what it covers, from the bytes the store holds.
        let part = request(
            &router,
            Method::GET,
            &file,
            Some(&read),
            &[("range", "bytes=1000-1999")],
        )
        .await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_value(part.headers(), header::CONTENT_RANGE),
            format!("bytes 1000-1999/{}", body.len())
        );
        assert_eq!(header_value(part.headers(), header::CONTENT_LENGTH), "1000");
        assert_eq!(bytes(part).await, body[1000..2000]);
        let tail = request(
            &router,
            Method::GET,
            &file,
            Some(&read),
            &[("range", "bytes=-10")],
        )
        .await;
        assert_eq!(tail.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(bytes(tail).await, body[body.len() - 10..]);
        let beyond = request(
            &router,
            Method::GET,
            &file,
            Some(&read),
            &[("range", "bytes=99999999-")],
        )
        .await;
        assert_eq!(beyond.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            header_value(beyond.headers(), header::CONTENT_RANGE),
            format!("bytes */{}", body.len())
        );
        // A range under an `If-Range` that names something else is the whole file.
        let stale = request(
            &router,
            Method::GET,
            &file,
            Some(&read),
            &[("range", "bytes=0-1"), ("if-range", "\"other\"")],
        )
        .await;
        assert_eq!(stale.status(), StatusCode::OK);

        // A validator answers without the bytes, and HEAD says the same as GET.
        let same = request(
            &router,
            Method::GET,
            &file,
            Some(&read),
            &[("if-none-match", tag.as_str())],
        )
        .await;
        assert_eq!(same.status(), StatusCode::NOT_MODIFIED);
        assert!(bytes(same).await.is_empty());
        let head = request(&router, Method::HEAD, &file, Some(&read), &[]).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            header_value(head.headers(), header::CONTENT_LENGTH),
            body.len().to_string()
        );
        assert!(bytes(head).await.is_empty());

        // The file has one name, and the item's top goes to its viewer.
        let other = request(
            &router,
            Method::GET,
            &format!("{item}/other.mp4"),
            Some(&read),
            &[],
        )
        .await;
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
        let top = request(&router, Method::GET, &format!("{item}/"), Some(&read), &[]).await;
        assert_eq!(top.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            header_value(top.headers(), header::LOCATION),
            "/_veoveo/view/a%20clip.mp4"
        );

        // Nothing here is a download record but a visit, and a visit is one however many reads.
        assert_eq!(
            recorded(&harness, Some("tenant-a"), "artifact.download_issued").await,
            0
        );
        let visit = request(&router, Method::POST, &item, Some(&read), &[]).await;
        assert_eq!(visit.status(), StatusCode::NO_CONTENT);
        for _ in 0..3 {
            let part = request(
                &router,
                Method::GET,
                &file,
                Some(&read),
                &[("range", "bytes=0-1")],
            )
            .await;
            assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        }
        assert_eq!(
            recorded(&harness, Some("tenant-a"), "artifact.download_issued").await,
            1
        );
    })
    .await;
}

#[tokio::test]
async fn a_grant_decides_each_request_and_someone_elses_artifact_is_unknown() {
    with_service(|harness| async move {
        let ada = Who::new("ada").tenant("tenant-a");
        let artifact = uploaded(&harness, &ada, "plan.pdf", b"%PDF-1.7 fixture").await;
        let router = router(&harness);
        let read = token(&[ArtifactPermission::ArtifactsRead]);
        let item = format!("/artifact/content/{artifact}");
        let file = format!("{item}/plan.pdf");
        let status = |method: Method, path: String| {
            let router = router.clone();
            let read = read.clone();
            async move {
                request(&router, method, &path, Some(&read), &[])
                    .await
                    .status()
            }
        };

        // Without a grant it does not exist for them, and a visit records nothing.
        assert_eq!(
            status(Method::HEAD, item.clone()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(Method::POST, item.clone()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(Method::GET, file.clone()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            recorded(&harness, Some("tenant-a"), "artifact.download_issued").await,
            0
        );

        let subject = AccessSubject::Principal(PrincipalId::parse(USER).unwrap());
        harness
            .service
            .grant_artifact(
                &ada.caller(),
                &GrantArtifactRequest {
                    artifact_id: artifact,
                    subject: subject.clone(),
                    level: AccessLevel::Read,
                    expires_at: None,
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(
            status(Method::HEAD, item.clone()).await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(status(Method::GET, file.clone()).await, StatusCode::OK);

        // The next request after a grant goes is refused.
        harness
            .service
            .revoke_artifact_grant(&ada.caller(), artifact, &subject, Utc::now())
            .await
            .unwrap();
        assert_eq!(status(Method::GET, file).await, StatusCode::NOT_FOUND);
        assert_eq!(status(Method::HEAD, item).await, StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn a_browser_previews_a_link_that_every_request_checks_and_others_get_the_signed_get() {
    with_service(|harness| async move {
        let body: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let ada = Who::new("ada");
        let artifact = uploaded(&harness, &ada, "photo.png", &body).await;
        let (link, link_id) = shared(&harness, artifact, &ada, TimeDelta::zero(), 1, None).await;
        let router = router(&harness);

        // A program gets the signed GET it always got.
        let program = request(
            &router,
            Method::GET,
            &format!("/artifact/shares/{link}"),
            None,
            &[],
        )
        .await;
        assert_eq!(program.status(), StatusCode::SEE_OTHER);
        assert!(
            header_value(program.headers(), header::LOCATION).contains("X-Amz-Expires=300"),
            "a program gets a signed GET"
        );
        assert_eq!(
            recorded(&harness, None, "artifact.share_link_redeemed").await,
            1
        );

        // A browser goes to the preview, on an origin that says nothing of the link's secret, and
        // the visit is one redemption, however many reads follow.
        let token = opened(&router, &link).await;
        assert!(!token.contains(&link));
        assert_eq!(
            recorded(&harness, None, "artifact.share_link_redeemed").await,
            2
        );
        let check = format!("/artifact/content-links/{token}");
        let file = format!("{check}/photo.png");
        let top = request(&router, Method::GET, &format!("{check}/"), None, &[]).await;
        assert_eq!(
            header_value(top.headers(), header::LOCATION),
            "/_veoveo/view/photo.png"
        );
        let head = request(&router, Method::HEAD, &check, None, &[]).await;
        assert_eq!(head.status(), StatusCode::NO_CONTENT);
        let whole = request(&router, Method::GET, &file, None, &[]).await;
        assert_eq!(whole.status(), StatusCode::OK);
        assert_eq!(bytes(whole).await, body);
        let part = request(
            &router,
            Method::GET,
            &file,
            None,
            &[("range", "bytes=10-19")],
        )
        .await;
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(bytes(part).await, body[10..20]);
        assert_eq!(
            recorded(&harness, None, "artifact.share_link_redeemed").await,
            2
        );

        // The same browser opening it again keeps the same origin, and its cache.
        assert_eq!(opened(&router, &link).await, token);

        // A preview's token redeems nothing, a person's route takes no token of a link, and a name
        // that is not the artifact's or a token that names no link answer the one refusal.
        let as_secret = request(
            &router,
            Method::GET,
            &format!("/artifact/shares/{token}"),
            None,
            &[],
        )
        .await;
        assert_eq!(as_secret.status(), StatusCode::NOT_FOUND);
        let as_person = request(
            &router,
            Method::GET,
            &format!("/artifact/content/{artifact}/photo.png"),
            None,
            &[],
        )
        .await;
        assert_eq!(as_person.status(), StatusCode::UNAUTHORIZED);
        for path in [
            format!("{check}/other.png"),
            format!("/artifact/content-links/{}/photo.png", "0".repeat(40)),
            "/artifact/content-links/nonsense/photo.png".to_owned(),
        ] {
            let refused = request(&router, Method::GET, &path, None, &[]).await;
            assert_eq!(refused.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(bytes(refused).await, b"no such link", "{path}");
        }

        // Revoking the link ends the preview at once, with the same refusal.
        harness
            .service
            .revoke_share_link(&ada.caller(), artifact, link_id, Utc::now())
            .await
            .unwrap();
        for (method, path) in [
            (Method::HEAD, check.clone()),
            (Method::GET, format!("{check}/")),
            (Method::GET, file.clone()),
        ] {
            let refused = request(&router, method, &path, None, &[]).await;
            assert_eq!(refused.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let again = request(
            &router,
            Method::GET,
            &format!("/artifact/shares/{link}"),
            None,
            &BROWSER,
        )
        .await;
        assert_eq!(again.status(), StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
async fn a_links_limit_counts_visits_and_its_expiry_and_release_end_a_preview() {
    with_service(|harness| async move {
        let ada = Who::new("ada");
        let router = router(&harness);

        // A limit of one is one visit: the preview it opened stays, and no second visit does.
        let spent = uploaded(&harness, &ada, "spent.png", b"spent").await;
        let (link, _) = shared(&harness, spent, &ada, TimeDelta::zero(), 1, Some(1)).await;
        let token = opened(&router, &link).await;
        let path = format!("/artifact/content-links/{token}/spent.png");
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::OK);
        let second = request(
            &router,
            Method::GET,
            &format!("/artifact/shares/{link}"),
            None,
            &BROWSER,
        )
        .await;
        assert_eq!(second.status(), StatusCode::NOT_FOUND);
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::OK);

        // Returning the artifact to private ends the preview, and releasing it revives nothing.
        harness
            .service
            .set_release_state(&ada.caller(), spent, ReleaseState::Private, Utc::now())
            .await
            .unwrap();
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::NOT_FOUND);
        harness
            .service
            .set_release_state(&ada.caller(), spent, ReleaseState::Released, Utc::now())
            .await
            .unwrap();
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::NOT_FOUND);

        // An expired link has no preview: created a day ago for a day, one second over.
        let expired = uploaded(&harness, &ada, "expired.png", b"expired").await;
        let (link, id) = shared(
            &harness,
            expired,
            &ada,
            TimeDelta::days(1) - TimeDelta::seconds(2),
            1,
            None,
        )
        .await;
        let token = opened(&router, &link).await;
        let path = format!("/artifact/content-links/{token}/expired.png");
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::OK);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let read = request(&router, Method::GET, &path, None, &[]).await;
        assert_eq!(read.status(), StatusCode::NOT_FOUND, "link {id:?}");
    })
    .await;
}
