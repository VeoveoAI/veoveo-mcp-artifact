//! `artifact.effective_level` decides each case of the design's access rules through the
//! store's reads.
mod support;

use chrono::Duration;
use support::*;
use veoveo_mcp_artifact::contract::GrantArtifactRequest;
use veoveo_types::{AccessLevel, AccessSubject, GroupId, PrincipalId};

fn grant(
    artifact: veoveo_mcp_artifact::contract::ArtifactId,
    subject: AccessSubject,
    level: AccessLevel,
) -> GrantArtifactRequest {
    GrantArtifactRequest {
        artifact_id: artifact,
        subject,
        level,
        expires_at: None,
    }
}

fn principal(key: &str) -> AccessSubject {
    AccessSubject::Principal(PrincipalId::parse(key).unwrap())
}

fn group(key: &str) -> AccessSubject {
    AccessSubject::Group(GroupId::parse(key).unwrap())
}

#[tokio::test]
async fn ownership_principal_grants_and_capped_group_grants_decide_the_level() {
    with_store(|store| async move {
        let owner = Who::new("owen");
        let artifact = available(&store, &owner, upload_request("plan.txt", Some(5))).await;
        let at = now();

        assert_eq!(
            level(&store, &owner, artifact, at).await,
            Some(AccessLevel::Admin)
        );

        // Without a grant it does not exist for anyone else in the tenant: an artifact is
        // private to its owner until shared.
        let stranger = Who::new("sam");
        assert_eq!(level(&store, &stranger, artifact, at).await, None);

        // A principal grant gives its level.
        store
            .grant_artifact(
                &owner.caller(),
                &grant(artifact, principal("sam"), AccessLevel::Write),
                at,
            )
            .await
            .unwrap();
        assert_eq!(
            level(&store, &stranger, artifact, at).await,
            Some(AccessLevel::Write)
        );

        // An expired grant gives nothing.
        let mut expiring = grant(artifact, principal("eve"), AccessLevel::Admin);
        expiring.expires_at = Some(at - Duration::minutes(1));
        store
            .grant_artifact(&owner.caller(), &expiring, at)
            .await
            .unwrap();
        let eve = Who::new("eve");
        assert_eq!(level(&store, &eve, artifact, at).await, None);

        // A group grant is capped by the caller's role in the group, and the cap works
        // both ways.
        store
            .grant_artifact(
                &owner.caller(),
                &grant(artifact, group("analysts"), AccessLevel::Admin),
                at,
            )
            .await
            .unwrap();
        let reader = Who::new("rita").group("analysts", AccessLevel::Read);
        assert_eq!(
            level(&store, &reader, artifact, at).await,
            Some(AccessLevel::Read)
        );
        store
            .grant_artifact(
                &owner.caller(),
                &grant(artifact, group("auditors"), AccessLevel::Read),
                at,
            )
            .await
            .unwrap();
        let lead = Who::new("lee").group("auditors", AccessLevel::Admin);
        assert_eq!(
            level(&store, &lead, artifact, at).await,
            Some(AccessLevel::Read)
        );
        // A group the caller is not in gives nothing.
        let outsider = Who::new("otto").group("other", AccessLevel::Admin);
        assert_eq!(level(&store, &outsider, artifact, at).await, None);

        // The highest source wins: a write grant over a read group grant.
        let both = Who::new("sam").group("analysts", AccessLevel::Read);
        assert_eq!(
            level(&store, &both, artifact, at).await,
            Some(AccessLevel::Write)
        );
    })
    .await;
}

#[tokio::test]
async fn labels_tenants_deletion_and_retention_hide_an_artifact_from_everyone() {
    with_store(|store| async move {
        let at = now();
        let cleared = Who::new("owen").labels(&["secret"]);
        let labelled = available(
            &store,
            &cleared,
            with_labels(upload_request("secret.txt", Some(3)), &["secret"]),
        )
        .await;
        assert_eq!(
            level(&store, &cleared, labelled, at).await,
            Some(AccessLevel::Admin)
        );

        // A label outside the caller's labels hides it, even from its owner and a grantee.
        let uncleared_owner = Who::new("owen");
        assert_eq!(level(&store, &uncleared_owner, labelled, at).await, None);
        store
            .grant_artifact(
                &cleared.caller(),
                &GrantArtifactRequest {
                    artifact_id: labelled,
                    subject: principal("sam"),
                    level: AccessLevel::Admin,
                    expires_at: None,
                },
                at,
            )
            .await
            .unwrap();
        assert_eq!(level(&store, &Who::new("sam"), labelled, at).await, None);
        assert_eq!(
            level(&store, &Who::new("sam").labels(&["secret"]), labelled, at).await,
            Some(AccessLevel::Admin)
        );

        // Another tenant's caller of the same name sees nothing.
        let elsewhere = Who::new("owen").tenant("globex").labels(&["secret"]);
        assert_eq!(level(&store, &elsewhere, labelled, at).await, None);

        // Past its retention it does not exist.
        let owner = Who::new("owen");
        let kept = available(
            &store,
            &owner,
            with_retention(upload_request("kept.txt", Some(1)), 1),
        )
        .await;
        assert_eq!(
            level(&store, &owner, kept, at + Duration::hours(23)).await,
            Some(AccessLevel::Admin)
        );
        assert_eq!(
            level(&store, &owner, kept, at + Duration::days(1)).await,
            None
        );

        // Deleted, it stops existing at once.
        let doomed = available(&store, &owner, upload_request("doomed.txt", Some(1))).await;
        store
            .delete_artifact(&owner.caller(), doomed, at)
            .await
            .unwrap();
        assert_eq!(level(&store, &owner, doomed, at).await, None);
    })
    .await;
}

#[tokio::test]
async fn administration_needs_admin_and_new_labels_need_clearance() {
    with_store(|store| async move {
        let at = now();
        let owner = Who::new("owen");
        let artifact = available(&store, &owner, upload_request("shared.txt", Some(2))).await;
        let viewer = Who::new("vic");
        store
            .grant_artifact(
                &owner.caller(),
                &grant(artifact, principal("vic"), AccessLevel::Read),
                at,
            )
            .await
            .unwrap();

        let refused = store
            .grant_artifact(
                &viewer.caller(),
                &GrantArtifactRequest {
                    artifact_id: artifact,
                    subject: principal("vic"),
                    level: AccessLevel::Admin,
                    expires_at: None,
                },
                at,
            )
            .await;
        assert!(matches!(
            refused,
            Err(veoveo_mcp_artifact::store::ArtifactStoreError::Forbidden {
                needed: AccessLevel::Admin
            })
        ));
        assert!(matches!(
            store.delete_artifact(&viewer.caller(), artifact, at).await,
            Err(veoveo_mcp_artifact::store::ArtifactStoreError::Forbidden { .. })
        ));

        // Anyone may create an artifact, which they own; a reader of another's is no
        // creator of theirs.
        let created = create(&store, &viewer, upload_request("new.txt", Some(1))).await;
        assert_eq!(created.artifact.owner.as_str(), "vic");
        assert!(created.artifact.labels.is_empty());

        // A label the creator does not hold is refused; the artifact carries exactly the
        // labels asked for.
        let created = store
            .create_upload(
                &Who::new("owen").caller(),
                &veoveo_types::Checked::new(with_labels(
                    upload_request("new.txt", Some(1)),
                    &["internal"],
                ))
                .unwrap(),
                at,
            )
            .await;
        assert!(matches!(
            created,
            Err(veoveo_mcp_artifact::store::ArtifactStoreError::LabelOutsideClearance)
        ));
        let cleared = Who::new("owen").labels(&["internal", "export"]);
        let created = create(
            &store,
            &cleared,
            with_labels(upload_request("new.txt", Some(1)), &["internal"]),
        )
        .await;
        assert_eq!(created.artifact.labels.len(), 1);

        // Grants list for administrators, a later grant to the same subject replaces its
        // earlier one, and revoking removes it.
        let grants = store
            .artifact_grants(&owner.caller(), artifact, at)
            .await
            .unwrap();
        assert_eq!(grants.len(), 1);
        store
            .grant_artifact(
                &owner.caller(),
                &GrantArtifactRequest {
                    artifact_id: artifact,
                    subject: principal("vic"),
                    level: AccessLevel::Admin,
                    expires_at: None,
                },
                at,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .artifact_grants(&owner.caller(), artifact, at)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .revoke_artifact_grant(&owner.caller(), artifact, &principal("vic"), at)
                .await
                .unwrap()
        );
        assert!(
            !store
                .revoke_artifact_grant(&owner.caller(), artifact, &principal("vic"), at)
                .await
                .unwrap()
        );
    })
    .await;
}
