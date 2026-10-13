//! The MCP surface through the SDK's in-process Gateway: discovery, permission checks on
//! tools and resources, and a tool call that reaches the store and object storage. The
//! test user is `user-1` of `tenant-a`, owner of Work Context `mission`.
mod support;

use std::sync::Arc;

use axum::http::{StatusCode, header};
use chrono::{TimeDelta, Utc};
use serde_json::{Value, json};
use support::rustfs::send;
use support::service::{Harness, with_service};
use tower::ServiceExt as _;
use veoveo_audit_contract::AuditRequest;
use veoveo_mcp_artifact::{
    contract::{
        AccessRequestId, AccessRequestListing, ArtifactId, ArtifactPermission,
        CreateShareLinkResult, ShareLinkId, ShareLinkListing, SignedTransfer,
    },
    mcp::{ArtifactApp, ArtifactMcp, ArtifactSubscriptions, TenantHubs},
    share,
};
use veoveo_sdk_rust::{
    AccessTokenSubject, ForwardedRequest, GatewayInternalTokenIssuer, GatewayProfileId,
    GatewayRequestContext, OAuthClientId, ProtectedResourceId, TokenIssuer,
    hosting::{
        Hosted, ListenOnly,
        testing::{self, TestGateway},
    },
    registration::ResourceIndicator,
};
use veoveo_types::InvocationMode;

/// The test Gateway, and the permissions every call through it carries.
struct Gateway {
    inner: TestGateway,
    granted: Vec<ArtifactPermission>,
}

impl Gateway {
    /// Call `method` as the test user with a token that carries the Gateway's signed
    /// request context, as every forwarded MCP request does.
    async fn rpc(&self, method: &str, params: Value) -> Value {
        let token = token(&self.granted);
        self.inner.rpc_with(method, params, Some(&token)).await.1
    }
}

/// A forwarded token for the test user with its request context.
fn token(granted: &[ArtifactPermission]) -> String {
    let principal = testing::principal();
    let authority = testing::authority();
    let now = Utc::now();
    let context = GatewayRequestContext {
        audit: AuditRequest::background(),
        access_token: AccessTokenSubject {
            issuer: principal.issuer.clone(),
            subject: principal.subject.clone(),
            oauth_client_id: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
            session_family: None,
            audience: ProtectedResourceId::parse(testing::TEST_RESOURCE).unwrap(),
            work_context: authority.work_context.clone(),
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
    GatewayInternalTokenIssuer::new(
        TokenIssuer::parse(testing::TEST_ISSUER).unwrap(),
        testing::signing_key(),
    )
    .unwrap()
    .issue(ForwardedRequest {
        profile: GatewayProfileId::parse("operations").unwrap(),
        server: veoveo_mcp_artifact::server(),
        audience: ResourceIndicator::parse(testing::TEST_RESOURCE).unwrap(),
        client: OAuthClientId::parse(testing::TEST_CLIENT).unwrap(),
        actor: principal,
        authority,
        permissions: granted.iter().map(|permission| permission.name()).collect(),
        request_context: Some(context),
        acting_server: None,
        acting_task: None,
        expires_at: now + TimeDelta::minutes(5),
    })
    .unwrap()
    .bearer_token
}

fn gateway(harness: &Harness, granted: &[ArtifactPermission]) -> Gateway {
    let app = Arc::new(ArtifactApp {
        service: (*harness.service).clone(),
        hubs: Arc::new(TenantHubs::default()),
    });
    Gateway {
        inner: TestGateway::new(
            testing::for_domain::<ArtifactMcp>()
                .handler(move || {
                    Hosted::new(ArtifactMcp::new(app.clone()))
                        .with_tasks(ListenOnly::new(ArtifactSubscriptions::new(app.clone())))
                })
                .build(),
        ),
        granted: granted.to_vec(),
    }
}

fn error_message(response: &Value) -> String {
    response["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn discovery_lists_every_tool_and_resource() {
    with_service(|harness| async move {
        let gateway = gateway(&harness, &[ArtifactPermission::ArtifactsRead]);
        let tools = gateway.rpc("tools/list", json!({})).await;
        let mut names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "abort_upload",
                "cancel_access_request",
                "complete_upload",
                "create_download",
                "create_share_link",
                "create_upload",
                "decide_access_request",
                "delete_artifact",
                "grant_artifact",
                "request_artifact_access",
                "revoke_artifact_grant",
                "revoke_share_link",
                "set_release_state",
                "set_upload_limits",
                "sign_upload_parts",
            ]
        );
        // Every typed call a consumer can make names one of these tools.
        let mut typed = veoveo_mcp_artifact::contract::served_tools().to_vec();
        typed.sort_unstable();
        assert_eq!(names, typed);
        let resources = gateway.rpc("resources/list", json!({})).await;
        let uris: Vec<&str> = resources["result"]["resources"]
            .as_array()
            .expect("resources")
            .iter()
            .filter_map(|resource| resource["uri"].as_str())
            .collect();
        for expected in [
            "artifact://artifacts",
            "artifact://uploads",
            "artifact://access-requests",
            "artifact://storage-usage",
            "artifact://contract",
        ] {
            assert!(uris.contains(&expected), "{expected} in {uris:?}");
        }
        let prompts = gateway.rpc("prompts/list", json!({})).await;
        let names: Vec<&str> = prompts["result"]["prompts"]
            .as_array()
            .expect("prompts")
            .iter()
            .filter_map(|prompt| prompt["name"].as_str())
            .collect();
        assert_eq!(names, ["review-access-requests", "review-artifact-sharing"]);
    })
    .await;
}

#[tokio::test]
async fn a_tool_without_its_permission_is_refused() {
    with_service(|harness| async move {
        let gateway = gateway(&harness, &[ArtifactPermission::ArtifactsRead]);
        let response = gateway
            .rpc(
                "tools/call",
                json!({
                    "name": "create_upload",
                    "arguments": {"name": "a.txt", "mediaType": "text/plain", "size": 3}
                }),
            )
            .await;
        assert!(
            error_message(&response).contains("artifact.artifacts.write"),
            "{response}"
        );
    })
    .await;
}

#[tokio::test]
async fn create_upload_signs_a_put_and_storage_usage_counts_the_reservation() {
    with_service(|harness| async move {
        let gateway = gateway(
            &harness,
            &[
                ArtifactPermission::ArtifactsRead,
                ArtifactPermission::ArtifactsWrite,
            ],
        );
        let created = gateway
            .rpc(
                "tools/call",
                json!({
                    "name": "create_upload",
                    "arguments": {"name": "a.txt", "mediaType": "text/plain", "size": 3}
                }),
            )
            .await;
        let result = &created["result"]["structuredContent"];
        assert_eq!(result["transfer"]["form"], "single", "{created}");
        assert_eq!(result["transfer"]["put"]["method"], "PUT");
        assert_eq!(result["artifact"]["workContext"], "mission");
        let usage = gateway
            .rpc("resources/read", json!({"uri": "artifact://storage-usage"}))
            .await;
        let text = usage["result"]["contents"][0]["text"]
            .as_str()
            .expect("usage text");
        let usage: Value = serde_json::from_str(text).unwrap();
        assert_eq!(usage["reservedBytes"], 3);

        let artifact_id = result["artifact"]["artifactId"].as_str().unwrap();
        let read = gateway
            .rpc(
                "resources/read",
                json!({"uri": format!("artifact://artifacts/{artifact_id}")}),
            )
            .await;
        let text = read["result"]["contents"][0]["text"]
            .as_str()
            .expect("artifact");
        let entry: Value = serde_json::from_str(text).unwrap();
        assert_eq!(entry["level"], "admin");
        assert_eq!(entry["artifact"]["state"], "uploading");
    })
    .await;
}

#[tokio::test]
async fn a_resource_without_its_permission_is_refused() {
    with_service(|harness| async move {
        let gateway = gateway(&harness, &[ArtifactPermission::ArtifactsWrite]);
        let response = gateway
            .rpc("resources/read", json!({"uri": "artifact://storage-usage"}))
            .await;
        assert!(
            error_message(&response).contains("artifact.artifacts.read"),
            "{response}"
        );
    })
    .await;
}

#[tokio::test]
async fn upload_limits_need_the_administer_permission() {
    with_service(|harness| async move {
        let arguments = json!({
            "tenant": "tenant-a",
            "limits": {
                "quotaBytes": 1000,
                "largestObject": 500,
                "allowedMediaTypes": {"kind": "any"},
                "activeUploads": 2
            }
        });
        let refused = gateway(&harness, &[ArtifactPermission::ArtifactsWrite])
            .rpc(
                "tools/call",
                json!({"name": "set_upload_limits", "arguments": arguments}),
            )
            .await;
        assert!(error_message(&refused).contains("upload-limits"), "{refused}");
        let administrator = gateway(
            &harness,
            &[
                ArtifactPermission::UploadLimitsAdminister,
                ArtifactPermission::ArtifactsWrite,
            ],
        );
        let set = administrator
            .rpc(
                "tools/call",
                json!({"name": "set_upload_limits", "arguments": arguments}),
            )
            .await;
        assert_eq!(
            set["result"]["structuredContent"]["limits"]["quotaBytes"],
            1000,
            "{set}"
        );
        let too_large = administrator
            .rpc(
                "tools/call",
                json!({
                    "name": "create_upload",
                    "arguments": {"name": "big.bin", "mediaType": "application/octet-stream", "size": 600}
                }),
            )
            .await;
        assert!(error_message(&too_large).contains("largest object"), "{too_large}");
    })
    .await;
}

/// The hosted router with the share route, as the binary serves it: a request reaches
/// `/artifact/shares/{token}` on the test host without Gateway authentication.
fn hosted_router(harness: &Harness) -> axum::Router {
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

fn structured(response: &Value) -> &Value {
    let result = &response["result"]["structuredContent"];
    assert!(result.is_object(), "{response}");
    result
}

/// Upload `body` through the tools and a signed PUT, and return the artifact's ID.
async fn upload(gateway: &Gateway, name: &str, body: &[u8]) -> String {
    let created = gateway
        .rpc(
            "tools/call",
            json!({
                "name": "create_upload",
                "arguments": {"name": name, "mediaType": "text/plain", "size": body.len()}
            }),
        )
        .await;
    let created = structured(&created);
    let put: SignedTransfer = serde_json::from_value(created["transfer"]["put"].clone()).unwrap();
    assert!(send(&put, body.to_vec(), &[]).await.status().is_success());
    let completed = gateway
        .rpc(
            "tools/call",
            json!({
                "name": "complete_upload",
                "arguments": {"uploadId": created["upload"]["uploadId"]}
            }),
        )
        .await;
    assert_eq!(structured(&completed)["artifact"]["state"], "available");
    created["artifact"]["artifactId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn redeem(router: &axum::Router, token: &str) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/artifact/shares/{token}"))
                .header("host", testing::TEST_HOST)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn sharing_tools_need_their_permissions() {
    with_service(|harness| async move {
        let reader = gateway(&harness, &[ArtifactPermission::ArtifactsRead]);
        let artifact = ArtifactId::new();
        for (tool, arguments, permission) in [
            (
                "set_release_state",
                json!({"artifactId": artifact, "releaseState": "released"}),
                "artifact.releases.write",
            ),
            (
                "create_share_link",
                json!({"artifactId": artifact, "expiresInDays": 1}),
                "artifact.share-links.write",
            ),
            (
                "revoke_share_link",
                json!({"artifactId": artifact, "shareLinkId": ShareLinkId::new()}),
                "artifact.share-links.write",
            ),
            (
                "request_artifact_access",
                json!({"artifactId": artifact, "level": "write", "justification": "review"}),
                "artifact.access-requests.write",
            ),
            (
                "cancel_access_request",
                json!({"accessRequestId": AccessRequestId::new()}),
                "artifact.access-requests.write",
            ),
            (
                "decide_access_request",
                json!({"accessRequestId": AccessRequestId::new(), "decision": "approve"}),
                "artifact.grants.write",
            ),
        ] {
            let refused = reader
                .rpc("tools/call", json!({"name": tool, "arguments": arguments}))
                .await;
            assert!(
                error_message(&refused).contains(permission),
                "{tool}: {refused}"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn an_owner_releases_shares_and_the_link_redeems_through_the_share_route() {
    with_service(|harness| async move {
        let owner = gateway(
            &harness,
            &[
                ArtifactPermission::ArtifactsRead,
                ArtifactPermission::ArtifactsWrite,
                ArtifactPermission::ReleasesWrite,
                ArtifactPermission::ShareLinksWrite,
            ],
        );
        let body = b"a shared report".to_vec();
        let artifact = upload(&owner, "shared.txt", &body).await;

        // A private artifact takes no share link.
        let private = owner
            .rpc(
                "tools/call",
                json!({"name": "create_share_link", "arguments": {"artifactId": artifact, "expiresInDays": 1}}),
            )
            .await;
        assert!(error_message(&private).contains("released"), "{private}");

        let released = owner
            .rpc(
                "tools/call",
                json!({"name": "set_release_state", "arguments": {"artifactId": artifact, "releaseState": "released"}}),
            )
            .await;
        assert_eq!(
            structured(&released)["artifact"]["releaseState"],
            "released"
        );
        let created = owner
            .rpc(
                "tools/call",
                json!({
                    "name": "create_share_link",
                    "arguments": {"artifactId": artifact, "expiresInDays": 7, "redemptionLimit": 1}
                }),
            )
            .await;
        let created: CreateShareLinkResult =
            serde_json::from_value(structured(&created).clone()).unwrap();
        let prefix = format!("{}/artifact/shares/", support::service::SHARE_ORIGIN);
        let token = created
            .url
            .expose()
            .strip_prefix(&prefix)
            .expect("the link names the share route under the share origin")
            .to_owned();

        // The administrators' listing shows the link without its secret.
        let listing = owner
            .rpc(
                "resources/read",
                json!({"uri": format!("artifact://artifacts/{artifact}/share-links")}),
            )
            .await;
        let text = listing["result"]["contents"][0]["text"]
            .as_str()
            .expect("share links");
        assert!(!text.contains(&token), "the listing holds no secret");
        let listing: ShareLinkListing = serde_json::from_str(text).unwrap();
        assert_eq!(listing.share_links.len(), 1);

        let router = hosted_router(&harness);
        let redirected = redeem(&router, &token).await;
        assert_eq!(redirected.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            redirected.headers()[header::CACHE_CONTROL],
            "no-store",
            "the redirect is not cached"
        );
        let location = redirected.headers()[header::LOCATION].to_str().unwrap();
        let fetched = reqwest::get(location).await.unwrap();
        assert!(fetched.status().is_success());
        assert_eq!(fetched.bytes().await.unwrap().as_ref(), body.as_slice());

        // The limit of one is spent, and every refusal looks alike.
        let spent = redeem(&router, &token).await;
        assert_eq!(spent.status(), StatusCode::NOT_FOUND);
        let malformed = redeem(&router, "not-a-token").await;
        assert_eq!(malformed.status(), StatusCode::NOT_FOUND);
        let spent_body = axum::body::to_bytes(spent.into_body(), usize::MAX)
            .await
            .unwrap();
        let malformed_body = axum::body::to_bytes(malformed.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(spent_body, malformed_body);

        let review = owner
            .rpc(
                "prompts/get",
                json!({"name": "review-artifact-sharing", "arguments": {"artifact_id": artifact}}),
            )
            .await;
        assert!(
            review["result"]["messages"][0]["content"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("share-links")),
            "{review}"
        );
    })
    .await;
}

#[tokio::test]
async fn an_owner_holds_every_level_and_cannot_request_more() {
    with_service(|harness| async move {
        let owner = gateway(
            &harness,
            &[
                ArtifactPermission::ArtifactsRead,
                ArtifactPermission::ArtifactsWrite,
                ArtifactPermission::AccessRequestsWrite,
            ],
        );
        let artifact = upload(&owner, "mine.txt", b"mine").await;
        let refused = owner
            .rpc(
                "tools/call",
                json!({
                    "name": "request_artifact_access",
                    "arguments": {"artifactId": artifact, "level": "admin", "justification": "more"}
                }),
            )
            .await;
        assert!(
            error_message(&refused).contains("already holds"),
            "{refused}"
        );
        let requests = owner
            .rpc(
                "resources/read",
                json!({"uri": "artifact://access-requests?role=requester"}),
            )
            .await;
        let text = requests["result"]["contents"][0]["text"]
            .as_str()
            .expect("access requests");
        let listing: AccessRequestListing = serde_json::from_str(text).unwrap();
        assert!(listing.access_requests.is_empty());
    })
    .await;
}
