//! Artifact's resource addresses: round trips, reserved characters, filters, cursors,
//! malformed identifiers, wrong parents and unsupported or duplicate query parameters.
use veoveo_mcp_artifact::contract::{
    AccessRequestRole, ArtifactId, ArtifactResource, ArtifactResourceError, MediaType, NamePrefix,
    ReleaseState, UploadId, UploadState,
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
    });
    round_trip(ArtifactResource::Uploads {
        state: Some(UploadState::Open),
    });
    round_trip(ArtifactResource::AccessRequests {
        role: Some(AccessRequestRole::Decider),
        state: None,
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
fn collection_selection_uses_shared_typed_cursor_and_limit_admission() {
    use veoveo_hub_sdk::{
        calls::ResourceRead,
        definition::{CollectionRequest, ReadCollection},
    };
    use veoveo_mcp_artifact::contract::{ReadShareLinks, ShareLinks};
    use veoveo_types::{CursorToken, PageSize, ResourceUri};
    let artifact = ArtifactId::new();
    let cursor: CursorToken<ShareLinks> =
        serde_json::from_value(serde_json::json!("opaque+query&value")).unwrap();
    let read = ReadCollection::new(ReadShareLinks { artifact }, PageSize::new(1).unwrap())
        .continuing(cursor);
    let uri = read.uri().unwrap();
    let (base, limit, cursor) =
        CollectionRequest::<ShareLinks>::split_uri(&uri, PageSize::new(50).unwrap()).unwrap();
    assert_eq!(limit.get(), 1);
    assert_eq!(cursor.unwrap().as_str(), "opaque+query&value");
    assert_eq!(
        ArtifactResource::parse(base.as_str()).unwrap(),
        ArtifactResource::ShareLinks {
            artifact_id: artifact
        }
    );
    for query in [
        "limit=0",
        "limit=101",
        "cursor=",
        "cursor=a&cursor=b",
        "%63ursor=a&cursor=b",
    ] {
        assert!(
            ResourceUri::new(format!(
                "artifact://artifacts/{artifact}/share-links?{query}"
            ))
            .map_or(true, |uri| CollectionRequest::<ShareLinks>::split_uri(
                &uri,
                PageSize::new(50).unwrap()
            )
            .is_err())
        );
    }
}
