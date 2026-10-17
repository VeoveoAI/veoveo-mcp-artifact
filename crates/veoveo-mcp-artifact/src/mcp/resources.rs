//! Reads of Artifact's resources. Every read is private to its caller and never reused,
//! because an artifact's access can change between reads.
use chrono::Utc;
use rmcp::{
    ErrorData as McpError, RoleServer, model::ReadResourceRequestParams, service::RequestContext,
};
use veoveo_hub_sdk::hosting::{DomainRead, json_read, served_by_host};
use veoveo_store::PageSize;

use crate::{
    contract::{
        AccessRequestListing, ArtifactEntry, ArtifactListing, ArtifactResource, GrantListing,
        ListCursor, ShareLinkListing, UploadListing,
    },
    store::ArtifactFilter,
};

use super::{
    ArtifactApp,
    errors::store_error,
    identity,
    requirements::{Target, require},
};

/// Entries a page holds when the address names no size.
const PAGE_ENTRIES: u32 = 50;

pub(super) async fn read(
    app: &ArtifactApp,
    address: ArtifactResource,
    request: &ReadResourceRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<DomainRead, McpError> {
    let uri = request.uri.as_str();
    let store = app.service.store();
    let now = Utc::now();
    let result = match address {
        ArtifactResource::Docs | ArtifactResource::Document(_) | ArtifactResource::Contract => {
            return Err(served_by_host());
        }
        ArtifactResource::Artifacts {
            work_context,
            owner,
            media_type,
            name,
            release_state,
            cursor,
        } => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let filter =
                ArtifactFilter::from_parts(work_context, owner, media_type, name, release_state)
                    .map_err(store_error)?;
            let page = store
                .artifacts_page(
                    &caller,
                    filter.as_ref(),
                    cursor.as_ref(),
                    page_size::<{ crate::store::MAX_ARTIFACTS_PER_PAGE }>(),
                    now,
                )
                .await
                .map_err(store_error)?;
            let listing = ArtifactListing {
                artifacts: page
                    .items
                    .into_iter()
                    .map(|visible| ArtifactEntry {
                        artifact: visible.record,
                        level: visible.level,
                    })
                    .collect(),
                next_cursor: page.next,
            };
            json_read(uri, &listing)?
        }
        ArtifactResource::Artifact(artifact_id) => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let visible = store
                .artifact(&caller, artifact_id, now)
                .await
                .map_err(store_error)?;
            json_read(
                uri,
                &ArtifactEntry {
                    artifact: visible.record,
                    level: visible.level,
                },
            )?
        }
        ArtifactResource::Grants(artifact_id) => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let grants = store
                .artifact_grants(&caller, artifact_id, now)
                .await
                .map_err(store_error)?;
            json_read(
                uri,
                &GrantListing {
                    artifact_id,
                    grants,
                },
            )?
        }
        ArtifactResource::Uploads { state, cursor } => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let page = store
                .uploads_page(
                    &caller,
                    state,
                    cursor.as_ref(),
                    page_size::<{ crate::store::MAX_UPLOADS_PER_PAGE }>(),
                )
                .await
                .map_err(store_error)?;
            let next_cursor = page
                .next
                .map(|cursor| ListCursor::parse(cursor.into_wire()))
                .transpose()
                .map_err(|_| McpError::internal_error("an upload cursor failed admission", None))?;
            json_read(
                uri,
                &UploadListing {
                    uploads: page.items,
                    next_cursor,
                },
            )?
        }
        ArtifactResource::Upload(upload_id) => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let (upload, _) = store
                .upload(&caller, upload_id)
                .await
                .map_err(store_error)?;
            json_read(uri, &upload)?
        }
        ArtifactResource::StorageUsage => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let usage = store.storage_usage(&caller).await.map_err(store_error)?;
            json_read(uri, &usage)?
        }
        ArtifactResource::ShareLinks {
            artifact_id,
            cursor,
        } => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let page = store
                .share_links_page(
                    &caller,
                    artifact_id,
                    cursor.as_ref(),
                    page_size::<{ crate::store::MAX_SHARE_LINKS_PER_PAGE }>(),
                    now,
                )
                .await
                .map_err(store_error)?;
            json_read(
                uri,
                &ShareLinkListing {
                    artifact_id,
                    share_links: page.items,
                    next_cursor: page.next,
                },
            )?
        }
        ArtifactResource::AccessRequests {
            role,
            state,
            cursor,
        } => {
            let identity = require(context, Target::Resource(uri))?;
            let caller = identity::caller(&identity)?;
            let page = store
                .access_requests_page(
                    &caller,
                    role,
                    state,
                    cursor.as_ref(),
                    page_size::<{ crate::store::MAX_ACCESS_REQUESTS_PER_PAGE }>(),
                    now,
                )
                .await
                .map_err(store_error)?;
            json_read(
                uri,
                &AccessRequestListing {
                    access_requests: page.items,
                    next_cursor: page.next,
                },
            )?
        }
    };
    Ok(DomainRead::no_store(result))
}

fn page_size<const MAX: u32>() -> PageSize<MAX> {
    PageSize::new(PAGE_ENTRIES.min(MAX)).expect("50 entries fit every page bound")
}
