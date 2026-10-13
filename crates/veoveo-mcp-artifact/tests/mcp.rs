//! The MCP surface through the SDK's in-process Gateway: discovery, permission checks on
//! tools and resources, and a tool call that reaches the store and object storage. The
//! test user is `user-1` of `tenant-a`, owner of Work Context `mission`.
mod support;

use std::sync::Arc;

use chrono::{TimeDelta, Utc};
use serde_json::{Value, json};
use support::service::{Harness, with_service};
use veoveo_audit_contract::AuditRequest;
use veoveo_mcp_artifact::{
    contract::ArtifactPermission,
    mcp::{ArtifactApp, ArtifactMcp, ArtifactSubscriptions, TenantHubs},
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
                "complete_upload",
                "create_download",
                "create_upload",
                "delete_artifact",
                "grant_artifact",
                "revoke_artifact_grant",
                "set_upload_limits",
                "sign_upload_parts",
            ]
        );
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
            "artifact://storage-usage",
            "artifact://contract",
        ] {
            assert!(uris.contains(&expected), "{expected} in {uris:?}");
        }
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
