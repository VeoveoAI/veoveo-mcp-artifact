//! Artifact's resource addresses under the `artifact` scheme. The route declaration
//! owns each shape and its typed parameters; the shared URI machinery owns encoding and
//! parsing, so no address is built or split by hand.
use std::{borrow::Cow, error::Error, fmt};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use veoveo_types::{
    PrincipalId, ResourceFieldCodec, ResourceRouteError, ResourceUriError, WorkContextId,
};

use super::{
    AccessRequestState, ArtifactId, MediaType, NamePrefix, ReleaseState, UploadId, UploadState,
};

/// The most bytes a listing cursor holds on the wire.
pub const MAX_CURSOR_BYTES: usize = 4096;

/// A resource address that failed admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactResourceError {
    Uri(ResourceUriError),
    UnknownResource,
    InvalidId,
    InvalidFilter,
    InvalidCursor,
}

impl fmt::Display for ArtifactResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uri(error) => error.fmt(f),
            Self::UnknownResource => {
                f.write_str("unknown or noncanonical Artifact resource address")
            }
            Self::InvalidId => f.write_str("invalid Artifact resource identifier"),
            Self::InvalidFilter => f.write_str("invalid Artifact listing filter"),
            Self::InvalidCursor => f.write_str("invalid Artifact listing cursor"),
        }
    }
}

impl Error for ArtifactResourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Uri(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ResourceUriError> for ArtifactResourceError {
    fn from(error: ResourceUriError) -> Self {
        Self::Uri(error)
    }
}

/// Whose access requests a listing holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, veoveo_types::Vocabulary)]
pub enum AccessRequestRole {
    /// Requests the caller made.
    Requester,
    /// Requests the caller can decide.
    Decider,
}

/// An opaque listing cursor as it travels in an address: 1 to 4096 base64url
/// characters. The server admits it against the listing, tenant and filter that issued
/// it; this type checks only its spelling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ListCursor(String);

impl ListCursor {
    pub fn parse(value: impl Into<String>) -> Result<Self, ArtifactResourceError> {
        let value = value.into();
        let spelled = !value.is_empty()
            && value.len() <= MAX_CURSOR_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
        if !spelled {
            return Err(ArtifactResourceError::InvalidCursor);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ListCursor {
    type Error = ArtifactResourceError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<ListCursor> for String {
    fn from(value: ListCursor) -> Self {
        value.0
    }
}

impl fmt::Display for ListCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl JsonSchema for ListCursor {
    fn schema_name() -> Cow<'static, str> {
        "ListCursor".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": MAX_CURSOR_BYTES,
            "pattern": "^[A-Za-z0-9_-]+$",
            "description": "An opaque cursor from the previous page of the same listing."
        })
    }
}

/// An Artifact resource. A usable address carries only the identifiers, filters and
/// cursor of its own family.
#[veoveo_types::resource_address(routes(ArtifactAddresses), schema = derived, wire, parse = borrowed, display)]
pub enum ArtifactResource {
    #[resource(
        template = "artifact://artifacts{?work_context,owner,media_type,name,release_state,cursor}",
        route_error = listing_route_error
    )]
    Artifacts {
        #[resource(error = |_| ArtifactResourceError::InvalidFilter)]
        work_context: Option<WorkContextId>,
        #[resource(error = |_| ArtifactResourceError::InvalidFilter)]
        owner: Option<PrincipalId>,
        #[resource(codec = ArtifactFieldCodec)]
        media_type: Option<MediaType>,
        #[resource(codec = ArtifactFieldCodec)]
        name: Option<NamePrefix>,
        #[resource(codec = ArtifactFieldCodec)]
        release_state: Option<ReleaseState>,
        #[resource(codec = ArtifactFieldCodec)]
        cursor: Option<ListCursor>,
    },
    #[resource(template = "artifact://artifacts/{artifact_id}")]
    Artifact(
        #[resource(variable = "artifact_id", error = |_| ArtifactResourceError::InvalidId)]
        ArtifactId,
    ),
    #[resource(template = "artifact://artifacts/{artifact_id}/grants")]
    Grants(
        #[resource(variable = "artifact_id", error = |_| ArtifactResourceError::InvalidId)]
        ArtifactId,
    ),
    #[resource(
        template = "artifact://artifacts/{artifact_id}/share-links{?cursor}",
        route_error = listing_route_error
    )]
    ShareLinks {
        #[resource(variable = "artifact_id", error = |_| ArtifactResourceError::InvalidId)]
        artifact_id: ArtifactId,
        #[resource(codec = ArtifactFieldCodec)]
        cursor: Option<ListCursor>,
    },
    #[resource(template = "artifact://uploads{?state,cursor}", route_error = listing_route_error)]
    Uploads {
        #[resource(codec = ArtifactFieldCodec)]
        state: Option<UploadState>,
        #[resource(codec = ArtifactFieldCodec)]
        cursor: Option<ListCursor>,
    },
    #[resource(template = "artifact://uploads/{upload_id}")]
    Upload(
        #[resource(variable = "upload_id", error = |_| ArtifactResourceError::InvalidId)] UploadId,
    ),
    #[resource(
        template = "artifact://access-requests{?role,state,cursor}",
        route_error = listing_route_error
    )]
    AccessRequests {
        #[resource(codec = ArtifactFieldCodec)]
        role: Option<AccessRequestRole>,
        #[resource(codec = ArtifactFieldCodec)]
        state: Option<AccessRequestState>,
        #[resource(codec = ArtifactFieldCodec)]
        cursor: Option<ListCursor>,
    },
    #[resource(template = "artifact://storage-usage")]
    StorageUsage,
}

impl ArtifactResource {
    /// The address of one artifact.
    pub fn artifact(artifact_id: ArtifactId) -> Self {
        Self::Artifact(artifact_id)
    }

    /// Collection pages change through their collection root, so only roots and single
    /// records accept subscriptions.
    pub fn is_subscribable(&self) -> bool {
        match self {
            Self::Artifacts { cursor, .. }
            | Self::ShareLinks { cursor, .. }
            | Self::Uploads { cursor, .. }
            | Self::AccessRequests { cursor, .. } => cursor.is_none(),
            Self::Artifact(_) | Self::Grants(_) | Self::Upload(_) | Self::StorageUsage => true,
        }
    }
}

fn route_error(error: ResourceRouteError) -> ArtifactResourceError {
    match error {
        ResourceRouteError::Uri(error) => ArtifactResourceError::Uri(error),
        _ => ArtifactResourceError::UnknownResource,
    }
}

fn listing_route_error(error: ResourceRouteError) -> ArtifactResourceError {
    match error {
        ResourceRouteError::Query => ArtifactResourceError::InvalidFilter,
        _ => route_error(error),
    }
}

struct ArtifactFieldCodec;

impl ResourceFieldCodec<MediaType> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<MediaType, Self::Error> {
        MediaType::parse(value).map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &MediaType) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<NamePrefix> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<NamePrefix, Self::Error> {
        NamePrefix::parse(value).map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &NamePrefix) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<ReleaseState> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<ReleaseState, Self::Error> {
        value
            .parse()
            .map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &ReleaseState) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<UploadState> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<UploadState, Self::Error> {
        value
            .parse()
            .map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &UploadState) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<AccessRequestState> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<AccessRequestState, Self::Error> {
        value
            .parse()
            .map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &AccessRequestState) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<AccessRequestRole> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<AccessRequestRole, Self::Error> {
        value
            .parse()
            .map_err(|_| ArtifactResourceError::InvalidFilter)
    }
    fn text(value: &AccessRequestRole) -> Cow<'_, str> {
        value.as_str().into()
    }
}

impl ResourceFieldCodec<ListCursor> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<ListCursor, Self::Error> {
        ListCursor::parse(value)
    }
    fn text(value: &ListCursor) -> Cow<'_, str> {
        value.as_str().into()
    }
}

#[doc(hidden)]
pub struct ArtifactAddresses;
impl veoveo_types::ResourceProfile for ArtifactAddresses {
    type Error = ArtifactResourceError;
    const PROFILE: veoveo_types::ResourceProfileSpec<Self::Error> =
        veoveo_types::ResourceProfileSpec {
            route_error: |_, error| route_error(error),
        };
}
