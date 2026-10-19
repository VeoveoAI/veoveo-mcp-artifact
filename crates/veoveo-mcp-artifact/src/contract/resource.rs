//! Artifact's resource addresses under the `artifact` scheme. The route declaration
//! owns each shape and its typed parameters; the shared URI machinery owns encoding and
//! parsing, so no address is built or split by hand.
use std::{borrow::Cow, error::Error, fmt};

use veoveo_types::{
    PrincipalId, ResourceFieldCodec, ResourceRouteError, ResourceUriError, WorkContextId,
};

use super::{
    AccessRequestState, ArtifactId, MediaType, NamePrefix, ReleaseState, UploadId, UploadState,
};

/// A resource address that failed admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactResourceError {
    Uri(ResourceUriError),
    UnknownResource,
    InvalidId,
    InvalidFilter,
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

/// An Artifact resource path and its owner filters. Shared paging selection is
/// carried by SDK ReadCollection with a nominal family cursor.
#[veoveo_types::resource_address(routes(ArtifactAddresses), schema = derived, wire, parse = borrowed, display)]
pub enum ArtifactResource {
    #[resource(template = "artifact://docs")]
    Docs,
    #[resource(template = "artifact://docs/{doc_id}")]
    Document(#[resource(variable = "doc_id", codec = ArtifactFieldCodec)] ArtifactDocument),
    #[resource(template = "artifact://contract")]
    Contract,
    #[resource(
        template = "artifact://artifacts{?work_context,owner,media_type,name,release_state}",
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
        template = "artifact://artifacts/{artifact_id}/share-links",
        route_error = listing_route_error
    )]
    ShareLinks {
        #[resource(variable = "artifact_id", error = |_| ArtifactResourceError::InvalidId)]
        artifact_id: ArtifactId,
    },
    #[resource(template = "artifact://uploads{?state}", route_error = listing_route_error)]
    Uploads {
        #[resource(codec = ArtifactFieldCodec)]
        state: Option<UploadState>,
    },
    #[resource(template = "artifact://uploads/{upload_id}")]
    Upload(
        #[resource(variable = "upload_id", error = |_| ArtifactResourceError::InvalidId)] UploadId,
    ),
    #[resource(
        template = "artifact://access-requests{?role,state}",
        route_error = listing_route_error
    )]
    AccessRequests {
        #[resource(codec = ArtifactFieldCodec)]
        role: Option<AccessRequestRole>,
        #[resource(codec = ArtifactFieldCodec)]
        state: Option<AccessRequestState>,
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
            Self::Artifacts { .. }
            | Self::ShareLinks { .. }
            | Self::Uploads { .. }
            | Self::AccessRequests { .. }
            | Self::Artifact(_)
            | Self::Grants(_)
            | Self::Upload(_)
            | Self::StorageUsage => true,
            Self::Docs | Self::Document(_) | Self::Contract => false,
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

/// A document the server embeds and serves under `artifact://docs/{doc_id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, veoveo_types::Vocabulary)]
pub enum ArtifactDocument {
    /// The agent work manual, `AGENTS.md`.
    Agents,
    /// The design, `DESIGN.md`.
    Design,
}

struct ArtifactFieldCodec;

impl ResourceFieldCodec<ArtifactDocument> for ArtifactFieldCodec {
    type Error = ArtifactResourceError;
    fn parse(value: &str) -> Result<ArtifactDocument, Self::Error> {
        value
            .parse()
            .map_err(|_| ArtifactResourceError::UnknownResource)
    }
    fn text(value: &ArtifactDocument) -> Cow<'_, str> {
        value.as_str().into()
    }
}

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

#[doc(hidden)]
pub struct ArtifactAddresses;
impl veoveo_types::ResourceProfile for ArtifactAddresses {
    type Error = ArtifactResourceError;
    const PROFILE: veoveo_types::ResourceProfileSpec<Self::Error> =
        veoveo_types::ResourceProfileSpec {
            route_error: |_, error| route_error(error),
        };
}
