//! Artifact listings: newest-first keyset pages, name prefixes ordered by name,
//! visibility inside the statement, one filter at most, and cursors bound to their
//! tenant, listing and filter.
mod support;

use support::*;
use veoveo_mcp_artifact::{
    contract::{
        ArtifactFilter, ArtifactId, ArtifactResourceError, Artifacts, MediaType, NamePrefix,
        ReleaseState, UploadState,
    },
    store::ArtifactStoreError,
};
use veoveo_types::{CursorToken, Page, PageSize, PrincipalId};

fn ids(page: &Page<Artifacts>) -> Vec<ArtifactId> {
    page.items()
        .iter()
        .map(|item| item.artifact.artifact_id)
        .collect()
}

#[tokio::test]
async fn pages_walk_newest_first_and_hold_only_readable_artifacts() {
    with_store(|store| async move {
        let owner = Who::new("owen");
        let mut created = Vec::new();
        for index in 0..5 {
            created.push(
                available(
                    &store,
                    &owner,
                    upload_request(&format!("file-{index}.txt"), Some(1)),
                )
                .await,
            );
        }
        // An artifact still uploading is not listed.
        create(&store, &owner, upload_request("pending.txt", Some(1))).await;
        // Someone else's artifact, not shared with the owner, is invisible to them.
        available(
            &store,
            &Who::new("sam"),
            upload_request("hidden.txt", Some(1)),
        )
        .await;

        let two = PageSize::<100>::new(2).unwrap();
        let caller = owner.caller();
        let first = store
            .artifacts_page(&caller, None, None, two, now())
            .await
            .unwrap();
        assert_eq!(ids(&first), vec![created[4], created[3]]);
        let next = first.next_cursor().cloned().expect("a second page follows");
        let second = store
            .artifacts_page(&caller, None, Some(&next), two, now())
            .await
            .unwrap();
        assert_eq!(ids(&second), vec![created[2], created[1]]);
        let third = store
            .artifacts_page(&caller, None, second.next_cursor(), two, now())
            .await
            .unwrap();
        assert_eq!(ids(&third), vec![created[0]]);
        assert!(third.next_cursor().is_none());

        // A member of a group the artifacts are shared with reads every one of them; a
        // stranger reads none.
        for artifact in &created {
            store
                .grant_artifact(
                    &caller,
                    &veoveo_mcp_artifact::contract::GrantArtifactRequest {
                        artifact_id: *artifact,
                        subject: veoveo_types::AccessSubject::Group(
                            veoveo_types::GroupId::parse("ops").unwrap(),
                        ),
                        level: veoveo_types::AccessLevel::Read,
                        expires_at: None,
                    },
                    now(),
                )
                .await
                .unwrap();
        }
        let viewer = Who::new("vic").group("ops", veoveo_types::AccessLevel::Write);
        let page = store
            .artifacts_page(
                &viewer.caller(),
                None,
                None,
                PageSize::<100>::new(100).unwrap(),
                now(),
            )
            .await
            .unwrap();
        assert_eq!(page.items().len(), 5);
        let stranger = Who::new("tom").group("elsewhere", veoveo_types::AccessLevel::Admin);
        let page = store
            .artifacts_page(&stranger.caller(), None, None, two, now())
            .await
            .unwrap();
        assert!(page.items().is_empty() && page.next_cursor().is_none());

        // Cursors stay with their tenant, listing and filter.
        let elsewhere = Who::new("owen").tenant("globex");
        assert!(matches!(
            store
                .artifacts_page(&elsewhere.caller(), None, Some(&next), two, now())
                .await,
            Err(ArtifactStoreError::Paging(
                veoveo_types::PagingError::Cursor(_)
            ))
        ));
        let owner_filter = ArtifactFilter::Owner(PrincipalId::parse("owen").unwrap());
        assert!(matches!(
            store
                .artifacts_page(&caller, Some(&owner_filter), Some(&next), two, now())
                .await,
            Err(ArtifactStoreError::Paging(
                veoveo_types::PagingError::Cursor(_)
            ))
        ));
        for (different_caller, limit) in [
            (viewer.caller(), two),
            (caller.clone(), PageSize::<100>::new(1).unwrap()),
        ] {
            assert!(matches!(
                store
                    .artifacts_page(&different_caller, None, Some(&next), limit, now())
                    .await,
                Err(ArtifactStoreError::Paging(
                    veoveo_types::PagingError::Cursor(_)
                ))
            ));
        }
        let forged: CursorToken<Artifacts> =
            serde_json::from_value(serde_json::json!("AAAA")).unwrap();
        assert!(matches!(
            store
                .artifacts_page(&caller, None, Some(&forged), two, now())
                .await,
            Err(ArtifactStoreError::Paging(
                veoveo_types::PagingError::Cursor(_)
            ))
        ));
    })
    .await;
}

#[tokio::test]
async fn each_filter_selects_its_artifacts() {
    with_store(|store| async move {
        let owner = Who::new("owen");
        let report_b = available(&store, &owner, upload_request("report-b.txt", Some(1))).await;
        let report_a = available(&store, &owner, upload_request("report-a.txt", Some(1))).await;
        let summary = available(&store, &owner, upload_request("summary.txt", Some(1))).await;
        // Another person's artifact shared with the owner is theirs to read, not the owner's.
        let colleague = Who::new("sam");
        let shared = available(&store, &colleague, upload_request("shared.txt", Some(1))).await;
        store
            .grant_artifact(
                &colleague.caller(),
                &veoveo_mcp_artifact::contract::GrantArtifactRequest {
                    artifact_id: shared,
                    subject: veoveo_types::AccessSubject::Principal(
                        PrincipalId::parse("owen").unwrap(),
                    ),
                    level: veoveo_types::AccessLevel::Read,
                    expires_at: None,
                },
                now(),
            )
            .await
            .unwrap();
        let mut image = upload_request("chart.png", Some(1));
        image.media_type = MediaType::parse("image/png").unwrap();
        let chart = available(&store, &owner, image).await;
        store
            .set_release_state(&owner.caller(), chart, ReleaseState::Released, now())
            .await
            .unwrap();

        let all = PageSize::<100>::new(100).unwrap();
        let list = |who: Who, filter: ArtifactFilter| {
            let store = &store;
            async move {
                let page = store
                    .artifacts_page(&who.caller(), Some(&filter), None, all, now())
                    .await
                    .unwrap();
                ids(&page)
            }
        };
        let prefix = ArtifactFilter::NamePrefix(NamePrefix::parse("report").unwrap());
        assert_eq!(list(owner.clone(), prefix).await, vec![report_a, report_b]);
        let media = ArtifactFilter::MediaType(MediaType::parse("image/png").unwrap());
        assert_eq!(list(owner.clone(), media).await, vec![chart]);
        let released = ArtifactFilter::ReleaseState(ReleaseState::Released);
        assert_eq!(list(owner.clone(), released).await, vec![chart]);
        // The owner filter lists the owner's own artifacts, not those shared with them.
        let mine = ArtifactFilter::Owner(PrincipalId::parse("owen").unwrap());
        assert_eq!(
            list(owner.clone(), mine).await,
            vec![chart, summary, report_a, report_b]
        );
        let theirs = ArtifactFilter::Owner(PrincipalId::parse("sam").unwrap());
        assert_eq!(list(owner.clone(), theirs).await, vec![shared]);

        // A name page continues by name.
        let one = PageSize::<100>::new(1).unwrap();
        let prefix = ArtifactFilter::NamePrefix(NamePrefix::parse("report-").unwrap());
        let first = store
            .artifacts_page(&owner.caller(), Some(&prefix), None, one, now())
            .await
            .unwrap();
        assert_eq!(ids(&first), vec![report_a]);
        let second = store
            .artifacts_page(
                &owner.caller(),
                Some(&prefix),
                first.next_cursor(),
                one,
                now(),
            )
            .await
            .unwrap();
        assert_eq!(ids(&second), vec![report_b]);
        assert!(second.next_cursor().is_none());

        // A listing applies at most one filter.
        assert!(matches!(
            ArtifactFilter::from_parts(
                Some(PrincipalId::parse("owen").unwrap()),
                Some(MediaType::parse("text/plain").unwrap()),
                None,
                None,
            ),
            Err(ArtifactResourceError::InvalidFilter)
        ));
        assert_eq!(
            ArtifactFilter::from_parts(None, None, None, None).unwrap(),
            None
        );
    })
    .await;
}

#[tokio::test]
async fn upload_pages_hold_the_callers_uploads() {
    with_store(|store| async move {
        let owner = Who::new("owen");
        let first = create(&store, &owner, upload_request("a.txt", Some(1))).await;
        let second = create(&store, &owner, upload_request("b.txt", Some(1))).await;
        create(&store, &Who::new("sam"), upload_request("c.txt", Some(1))).await;
        store
            .abort_upload(&owner.caller(), first.upload.upload_id, now())
            .await
            .unwrap();

        let one = PageSize::<100>::new(1).unwrap();
        let page = store
            .uploads_page(&owner.caller(), None, None, one)
            .await
            .unwrap();
        assert_eq!(page.items()[0].upload_id, second.upload.upload_id);
        let next = page.next_cursor().cloned();
        let page = store
            .uploads_page(&owner.caller(), None, next.as_ref(), one)
            .await
            .unwrap();
        assert_eq!(page.items()[0].upload_id, first.upload.upload_id);
        assert!(page.next_cursor().is_none());

        let open = store
            .uploads_page(
                &owner.caller(),
                Some(UploadState::Open),
                None,
                PageSize::<100>::new(10).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(open.items().len(), 1);
        assert_eq!(open.items()[0].upload_id, second.upload.upload_id);
    })
    .await;
}
