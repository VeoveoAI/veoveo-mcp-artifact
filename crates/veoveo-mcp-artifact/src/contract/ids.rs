//! Artifact's identifiers. Each is a lowercase hyphenated UUIDv7 that the server
//! generates, so key order is creation order. The server binds them to PostgreSQL
//! `uuid`; contract consumers carry no database dependency.
use veoveo_types::{IdFailure, IdMetadata, IdProfile, IdProfileSpec, UuidGrammar};

/// One stored file.
#[cfg_attr(
    feature = "server",
    veoveo_types::id(uuid(ArtifactIds), fresh, postgres)
)]
#[cfg_attr(not(feature = "server"), veoveo_types::id(uuid(ArtifactIds), fresh))]
pub struct ArtifactId(uuid::Uuid);

/// One upload, which reserves an artifact and receives its bytes.
#[cfg_attr(
    feature = "server",
    veoveo_types::id(uuid(ArtifactIds), fresh, postgres)
)]
#[cfg_attr(not(feature = "server"), veoveo_types::id(uuid(ArtifactIds), fresh))]
pub struct UploadId(uuid::Uuid);

/// One share link. The link's secret is separate and never stored.
#[cfg_attr(
    feature = "server",
    veoveo_types::id(uuid(ArtifactIds), fresh, postgres)
)]
#[cfg_attr(not(feature = "server"), veoveo_types::id(uuid(ArtifactIds), fresh))]
pub struct ShareLinkId(uuid::Uuid);

/// One request for access to an artifact.
#[cfg_attr(
    feature = "server",
    veoveo_types::id(uuid(ArtifactIds), fresh, postgres)
)]
#[cfg_attr(not(feature = "server"), veoveo_types::id(uuid(ArtifactIds), fresh))]
pub struct AccessRequestId(uuid::Uuid);

/// An identifier that is not a lowercase hyphenated UUIDv7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{kind} must be a lowercase hyphenated UUIDv7")]
pub struct ArtifactIdentifierError {
    kind: &'static str,
}

#[doc(hidden)]
pub struct ArtifactIds;
impl IdProfile for ArtifactIds {
    type Error = ArtifactIdentifierError;
    const PROFILE: IdProfileSpec<Self::Error> =
        IdProfileSpec::generated_uuid(UuidGrammar::canonical(&[7]), identifier_error);
}

fn identifier_error(_: &str, metadata: IdMetadata, _: IdFailure) -> ArtifactIdentifierError {
    ArtifactIdentifierError {
        kind: metadata.type_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_fresh_uuid_v7_in_canonical_spelling() {
        let id = ArtifactId::new();
        assert_eq!(id.as_uuid().get_version_num(), 7);
        assert_eq!(ArtifactId::parse(id.to_string()).unwrap(), id);
        let upper = id.to_string().to_uppercase();
        assert!(ArtifactId::parse(upper).is_err());
        let v4 = "9f5c3e0a-3c1e-4d7b-9f0a-6f1f2b3c4d5e";
        let error = UploadId::parse(v4).unwrap_err();
        assert_eq!(
            error.to_string(),
            "UploadId must be a lowercase hyphenated UUIDv7"
        );
    }
}
