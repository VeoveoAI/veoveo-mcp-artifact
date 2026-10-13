//! `artifact.effective_level` decides each case of the design's access rules through the
//! store's reads.
mod support;

use chrono::Duration;
use support::*;
use veoveo_mcp_artifact::contract::GrantArtifactRequest;
use veoveo_types::{AccessLevel, AccessSubject, GroupId, PrincipalId, WorkContextMembershipLevel};

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
async fn ownership_grants_groups_and_work_context_decide_the_level() {
    with_store(|store| async move {
        let owner = Who::new("owen");
        let artifact = available(&store, &owner, upload_request("plan.txt", Some(5))).await;
        let at = now();

        assert_eq!(
            level(&store, &owner, artifact, at).await,
            Some(AccessLevel::Admin)
        );

        // Outside the artifact's Work Context and without a grant, it does not exist.
        let stranger = Who::new("sam").in_context("research", WorkContextMembershipLevel::Owner);
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
        let eve = Who::new("eve").in_context("research", WorkContextMembershipLevel::Viewer);
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
        let reader = Who::new("rita")
            .in_context("research", WorkContextMembershipLevel::Viewer)
            .group("analysts", AccessLevel::Read);
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
        let lead = Who::new("lee")
            .in_context("research", WorkContextMembershipLevel::Viewer)
            .group("auditors", AccessLevel::Admin);
        assert_eq!(
            level(&store, &lead, artifact, at).await,
            Some(AccessLevel::Read)
        );
        // A group the caller is not in gives nothing.
        let outsider = Who::new("otto")
            .in_context("research", WorkContextMembershipLevel::Viewer)
            .group("other", AccessLevel::Admin);
        assert_eq!(level(&store, &outsider, artifact, at).await, None);

        // Membership in the artifact's Work Context counts while it is the selected one.
        for (membership, expected) in [
            (WorkContextMembershipLevel::Viewer, AccessLevel::Read),
            (WorkContextMembershipLevel::Contributor, AccessLevel::Write),
            (WorkContextMembershipLevel::Custodian, AccessLevel::Admin),
            (WorkContextMembershipLevel::Owner, AccessLevel::Admin),
        ] {
            let member = Who::new("mia").in_context("ops", membership);
            assert_eq!(
                level(&store, &member, artifact, at).await,
                Some(expected),
                "{membership:?}"
            );
        }
        let elsewhere = Who::new("mia").in_context("research", WorkContextMembershipLevel::Owner);
        assert_eq!(level(&store, &elsewhere, artifact, at).await, None);

        // The highest source wins: a write grant over viewer membership.
        let both = Who::new("sam").in_context("ops", WorkContextMembershipLevel::Viewer);
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
async fn administration_needs_admin_and_writing_needs_contributor() {
    with_store(|store| async move {
        let at = now();
        let owner = Who::new("owen");
        let artifact = available(&store, &owner, upload_request("shared.txt", Some(2))).await;
        let viewer = Who::new("vic").in_context("ops", WorkContextMembershipLevel::Viewer);

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

        // A viewer cannot create artifacts in the Work Context.
        let created = store
            .create_upload(
                &viewer.caller(),
                &veoveo_types::Checked::new(upload_request("new.txt", Some(1))).unwrap(),
                at,
            )
            .await;
        assert!(matches!(
            created,
            Err(veoveo_mcp_artifact::store::ArtifactStoreError::WorkContextReadOnly)
        ));

        // A label the creator does not hold is refused, including one the Work Context
        // stamps on every output.
        let stamped = Who::new("owen").output_labels(&["internal"]);
        let created = store
            .create_upload(
                &stamped.caller(),
                &veoveo_types::Checked::new(upload_request("new.txt", Some(1))).unwrap(),
                at,
            )
            .await;
        assert!(matches!(
            created,
            Err(veoveo_mcp_artifact::store::ArtifactStoreError::LabelOutsideClearance)
        ));
        let cleared = Who::new("owen")
            .output_labels(&["internal"])
            .labels(&["internal"]);
        let created = create(&store, &cleared, upload_request("new.txt", Some(1))).await;
        assert_eq!(created.artifact.labels.len(), 1);

        // Grants list for administrators, and revoking removes one.
        let grants = store
            .artifact_grants(&owner.caller(), artifact, at)
            .await
            .unwrap();
        assert!(grants.is_empty());
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
