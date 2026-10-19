//! Typed readers retain current row-level authorization on every page/read.
use super::{ArtifactMcp, errors::store_error, identity};
use crate::contract::{
    AccessRequests, ArtifactEntry, ArtifactPermission, Artifacts, Grants, ReadAccessRequests,
    ReadArtifact, ReadArtifacts, ReadGrants, ReadShareLinks, ReadStorageUsage, ReadUpload,
    ReadUploads, ShareLinks, StorageUsage, UploadRecord, Uploads,
};
use chrono::Utc;
use rmcp::{ErrorData, RoleServer, service::RequestContext};
use veoveo_hub_sdk::{
    definition::ReadCollection,
    hosting::{ReadCache, ResourceBuilder, RoutePolicy, RoutesBuilder, hub_identity},
};
use veoveo_types::{LiveKeyset, Page, Pager};

fn policy() -> RoutePolicy {
    RoutePolicy::permission(ArtifactPermission::ArtifactsRead).expect("Artifact read permission")
}
pub(super) fn register(routes: RoutesBuilder<ArtifactMcp>) -> RoutesBuilder<ArtifactMcp> {
    routes
        .template::<ReadArtifact, _, _>(
            ResourceBuilder::new("Artifact")
                .reader(read_artifact)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .template::<ReadUpload, _, _>(
            ResourceBuilder::new("Upload")
                .reader(read_upload)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .singleton(
            ReadStorageUsage,
            ResourceBuilder::new("Storage usage")
                .reader(read_storage_usage)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadArtifacts, _, _>(
            Some(ReadArtifacts { filter: None }),
            ResourceBuilder::new("Artifacts")
                .reader(read_artifacts)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadUploads, _, _>(
            Some(ReadUploads { state: None }),
            ResourceBuilder::new("Uploads")
                .reader(read_uploads)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadShareLinks, _, _>(
            None,
            ResourceBuilder::new("Share links")
                .reader(read_share_links)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadGrants, _, _>(
            None,
            ResourceBuilder::new("Grants")
                .reader(read_grants)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
        .collection_resource::<ReadAccessRequests, _, _>(
            Some(ReadAccessRequests {
                role: None,
                state: None,
            }),
            ResourceBuilder::new("Access requests")
                .reader(read_access_requests)
                .policy(policy())
                .cache(ReadCache::NoStore),
        )
}
async fn read_artifact(
    domain: ArtifactMcp,
    read: ReadArtifact,
    context: RequestContext<RoleServer>,
) -> Result<ArtifactEntry, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let visible = domain
        .app
        .service
        .store()
        .artifact(&caller, read.artifact, Utc::now())
        .await
        .map_err(store_error)?;
    Ok(ArtifactEntry {
        artifact: visible.record,
        level: visible.level,
    })
}
async fn read_upload(
    domain: ArtifactMcp,
    read: ReadUpload,
    context: RequestContext<RoleServer>,
) -> Result<UploadRecord, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (upload, _) = domain
        .app
        .service
        .store()
        .upload(&caller, read.upload)
        .await
        .map_err(store_error)?;
    Ok(upload)
}
async fn read_storage_usage(
    domain: ArtifactMcp,
    _: ReadStorageUsage,
    context: RequestContext<RoleServer>,
) -> Result<StorageUsage, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    domain
        .app
        .service
        .store()
        .storage_usage(&caller)
        .await
        .map_err(store_error)
}
async fn read_artifacts(
    domain: ArtifactMcp,
    read: ReadCollection<ReadArtifacts>,
    context: RequestContext<RoleServer>,
) -> Result<Page<Artifacts>, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (filter, limit, cursor) = read.into_selection().into_parts();
    domain
        .app
        .service
        .store()
        .artifacts_page(&caller, filter.as_ref(), cursor.as_ref(), limit, Utc::now())
        .await
        .map_err(store_error)
}
async fn read_uploads(
    domain: ArtifactMcp,
    read: ReadCollection<ReadUploads>,
    context: RequestContext<RoleServer>,
) -> Result<Page<Uploads>, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (filter, limit, cursor) = read.into_selection().into_parts();
    domain
        .app
        .service
        .store()
        .uploads_page(&caller, filter.state, cursor.as_ref(), limit)
        .await
        .map_err(store_error)
}
async fn read_share_links(
    domain: ArtifactMcp,
    read: ReadCollection<ReadShareLinks>,
    context: RequestContext<RoleServer>,
) -> Result<Page<ShareLinks>, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (artifact, limit, cursor) = read.into_selection().into_parts();
    domain
        .app
        .service
        .store()
        .share_links_page(&caller, artifact, cursor.as_ref(), limit, Utc::now())
        .await
        .map_err(store_error)
}
async fn read_grants(
    domain: ArtifactMcp,
    read: ReadCollection<ReadGrants>,
    context: RequestContext<RoleServer>,
) -> Result<Page<Grants>, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (artifact, limit, cursor) = read.into_selection().into_parts();
    let grants = domain
        .app
        .service
        .store()
        .artifact_grants(&caller, artifact, Utc::now())
        .await
        .map_err(store_error)?;
    Pager::<Grants>::new(
        caller.tenant(),
        artifact,
        limit,
        LiveKeyset,
        cursor.as_ref(),
    )
    .and_then(|pager| pager.select(grants))
    .map_err(|error| store_error(error.into()))
}
async fn read_access_requests(
    domain: ArtifactMcp,
    read: ReadCollection<ReadAccessRequests>,
    context: RequestContext<RoleServer>,
) -> Result<Page<AccessRequests>, ErrorData> {
    let caller = identity::caller(&hub_identity(&context)?)?;
    let (filter, limit, cursor) = read.into_selection().into_parts();
    domain
        .app
        .service
        .store()
        .access_requests_page(
            &caller,
            filter.role,
            filter.state,
            cursor.as_ref(),
            limit,
            Utc::now(),
        )
        .await
        .map_err(store_error)
}
