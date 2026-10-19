//! Domain collection identities, total SQL orders and contextual keyset policies.
use super::{
    AccessRequestId, AccessRequestRecord, AccessRequestRole, AccessRequestState, ArtifactEntry,
    ArtifactGrant, ArtifactId, ArtifactResourceError, MediaType, NamePrefix, ReleaseState,
    ShareLinkId, ShareLinkRecord, UploadId, UploadRecord, UploadState,
};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt};
use veoveo_types::{
    Collection, CursorError, CursorScope, LiveKeyset, PageSize, PagingPolicy, PrincipalId,
    WorkContextId,
};

/// Exactly one effective artifact filter; absence lists all visible artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ArtifactFilter {
    WorkContext(WorkContextId),
    Owner(PrincipalId),
    MediaType(MediaType),
    NamePrefix(NamePrefix),
    ReleaseState(ReleaseState),
}
impl ArtifactFilter {
    pub fn from_parts(
        work_context: Option<WorkContextId>,
        owner: Option<PrincipalId>,
        media_type: Option<MediaType>,
        name: Option<NamePrefix>,
        release_state: Option<ReleaseState>,
    ) -> Result<Option<Self>, ArtifactResourceError> {
        let mut filters = [
            work_context.map(Self::WorkContext),
            owner.map(Self::Owner),
            media_type.map(Self::MediaType),
            name.map(Self::NamePrefix),
            release_state.map(Self::ReleaseState),
        ]
        .into_iter()
        .flatten();
        let first = filters.next();
        if filters.next().is_some() {
            return Err(ArtifactResourceError::InvalidFilter);
        }
        Ok(first)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactPosition {
    pub name: String,
    pub artifact_id: ArtifactId,
}
pub struct Artifacts;
impl Collection for Artifacts {
    type Item = ArtifactEntry;
    type Filters = Option<ArtifactFilter>;
    type Key = ArtifactPosition;
    type Limit = PageSize<100>;
    type Policy = PrincipalKeyset;
    const NAME: &'static str = "artifact.artifacts";
    fn key(item: &ArtifactEntry) -> ArtifactPosition {
        ArtifactPosition {
            name: item.artifact.name.as_str().to_owned(),
            artifact_id: item.artifact.artifact_id,
        }
    }
    fn compare(filters: &Self::Filters, a: &ArtifactPosition, b: &ArtifactPosition) -> Ordering {
        if matches!(filters, Some(ArtifactFilter::NamePrefix(_))) {
            a.cmp(b)
        } else {
            b.artifact_id.cmp(&a.artifact_id)
        }
    }
}
/// Caller identity is taken from current admission, not from resource query data.
/// Both applicable families serialize it with their typed filters and page size.
pub struct PrincipalKeyset {
    principal: PrincipalId,
}
impl PrincipalKeyset {
    pub fn new(principal: PrincipalId) -> Self {
        Self { principal }
    }
}
impl<C: Collection<Policy = Self>> PagingPolicy<C> for PrincipalKeyset {
    fn scope(
        &self,
        tenant: impl fmt::Display,
        filters: &C::Filters,
        limit: u32,
    ) -> Result<CursorScope, CursorError> {
        CursorScope::new(C::NAME, tenant, &(filters, limit, &self.principal))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadFilters {
    pub state: Option<UploadState>,
}
pub struct Uploads;
impl Collection for Uploads {
    type Item = UploadRecord;
    type Filters = UploadFilters;
    type Key = UploadId;
    type Limit = PageSize<100>;
    type Policy = PrincipalKeyset;
    const NAME: &'static str = "artifact.uploads";
    fn key(item: &UploadRecord) -> UploadId {
        item.upload_id
    }
    fn compare(_: &Self::Filters, a: &UploadId, b: &UploadId) -> Ordering {
        b.cmp(a)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessRequestFilters {
    pub role: Option<AccessRequestRole>,
    pub state: Option<AccessRequestState>,
}
pub struct AccessRequests;
impl Collection for AccessRequests {
    type Item = AccessRequestRecord;
    type Filters = AccessRequestFilters;
    type Key = AccessRequestId;
    type Limit = PageSize<100>;
    type Policy = PrincipalKeyset;
    const NAME: &'static str = "artifact.access_requests";
    fn key(item: &AccessRequestRecord) -> AccessRequestId {
        item.access_request_id
    }
    fn compare(_: &Self::Filters, a: &AccessRequestId, b: &AccessRequestId) -> Ordering {
        b.cmp(a)
    }
}
pub struct ShareLinks;
impl Collection for ShareLinks {
    type Item = ShareLinkRecord;
    type Filters = ArtifactId;
    type Key = ShareLinkId;
    type Limit = PageSize<100>;
    type Policy = LiveKeyset;
    const NAME: &'static str = "artifact.share_links";
    fn key(item: &ShareLinkRecord) -> ShareLinkId {
        item.share_link_id
    }
    fn compare(_: &Self::Filters, a: &ShareLinkId, b: &ShareLinkId) -> Ordering {
        b.cmp(a)
    }
}
pub struct Grants;
impl Collection for Grants {
    type Item = ArtifactGrant;
    type Filters = ArtifactId;
    type Key = (String, String);
    type Limit = PageSize<256>;
    type Policy = LiveKeyset;
    const NAME: &'static str = "artifact.grants";
    fn key(item: &ArtifactGrant) -> Self::Key {
        match &item.subject {
            veoveo_types::AccessSubject::Principal(id) => ("principal".into(), id.to_string()),
            veoveo_types::AccessSubject::Group(id) => ("group".into(), id.to_string()),
        }
    }
}
