//! Artifact's resource addresses: round trips, reserved characters, filters, cursors,
//! malformed identifiers, wrong parents and unsupported or duplicate query parameters.
use veoveo_mcp_artifact::contract::{
    AccessRequestRole, ArtifactId, ArtifactResource, ArtifactResourceError, ListCursor, MediaType,
    NamePrefix, ReleaseState, UploadId, UploadState,
};
use veoveo_types::{PrincipalId, WorkContextId};

fn round_trip(resource: ArtifactResource) -> String {
    let wire = resource.to_string();
    let parsed = ArtifactResource::parse(&wire).unwrap_or_else(|error| panic!("{wire}: {error}"));
    assert_eq!(parsed, resource, "{wire}");
    wire
}

#[test]
fn records_and_collections_round_trip() {
    let artifact = ArtifactId::new();
    assert_eq!(
        round_trip(ArtifactResource::artifact(artifact)),
        format!("artifact://artifacts/{artifact}")
    );
    assert_eq!(
        round_trip(ArtifactResource::Grants(artifact)),
        format!("artifact://artifacts/{artifact}/grants")
    );
    let upload = UploadId::new();
    assert_eq!(
        round_trip(ArtifactResource::Upload(upload)),
        format!("artifact://uploads/{upload}")
    );
    assert_eq!(
        round_trip(ArtifactResource::StorageUsage),
        "artifact://storage-usage"
    );
    round_trip(ArtifactResource::ShareLinks {
        artifact_id: artifact,
        cursor: Some(ListCursor::parse("abc_DEF-123").unwrap()),
    });
    round_trip(ArtifactResource::Uploads {
        state: Some(UploadState::Open),
        cursor: None,
    });
    round_trip(ArtifactResource::AccessRequests {
        role: Some(AccessRequestRole::Decider),
        state: None,
        cursor: None,
    });
}

#[test]
fn listing_filters_encode_reserved_characters() {
    let resource = ArtifactResource::Artifacts {
        work_context: Some(WorkContextId::parse("ops").unwrap()),
        owner: Some(PrincipalId::parse("alice").unwrap()),
        media_type: Some(MediaType::parse("application/vnd.api+json").unwrap()),
        name: Some(NamePrefix::parse("Q3 report & notes?#").unwrap()),
        release_state: Some(ReleaseState::Released),
        cursor: None,
    };
    let wire = round_trip(resource);
    assert!(!wire.contains(" & "), "{wire}");
    assert_eq!(
        round_trip(ArtifactResource::Artifacts {
            work_context: None,
            owner: None,
            media_type: None,
            name: None,
            release_state: None,
            cursor: None,
        }),
        "artifact://artifacts"
    );
}

#[test]
fn malformed_addresses_are_refused() {
    let artifact = ArtifactId::new();
    for wire in [
        "artifact://artifacts/not-a-uuid".to_owned(),
        "artifact://artifacts/9f5c3e0a-3c1e-4d7b-9f0a-6f1f2b3c4d5e".to_owned(),
        format!("artifact://uploads/{artifact}/grants"),
        format!("artifact://artifacts/{artifact}/unknown"),
        "artifact://elsewhere".to_owned(),
        "time://artifacts".to_owned(),
        "artifact://artifacts?colour=red".to_owned(),
        "artifact://artifacts?release_state=public".to_owned(),
        "artifact://artifacts?media_type=Text/Plain".to_owned(),
        "artifact://artifacts?cursor=a&cursor=b".to_owned(),
        "artifact://artifacts?cursor=not%20base64".to_owned(),
        "artifact://uploads?state=pending".to_owned(),
    ] {
        assert!(ArtifactResource::parse(&wire).is_err(), "{wire} parsed");
    }
    assert_eq!(
        ArtifactResource::parse("artifact://artifacts?release_state=public").unwrap_err(),
        ArtifactResourceError::InvalidFilter
    );
}

#[test]
fn only_roots_and_records_accept_subscriptions() {
    let artifact = ArtifactId::new();
    assert!(ArtifactResource::artifact(artifact).is_subscribable());
    assert!(
        !ArtifactResource::ShareLinks {
            artifact_id: artifact,
            cursor: Some(ListCursor::parse("next").unwrap()),
        }
        .is_subscribable()
    );
}
