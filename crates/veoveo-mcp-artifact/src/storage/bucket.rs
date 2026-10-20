//! Bucket preparation at startup: the bucket itself, the CORS rule browsers need to send
//! signed requests, and the lifecycle rule that removes abandoned multipart uploads.
use aws_sdk_s3::{
    error::ProvideErrorMetadata,
    types::{
        AbortIncompleteMultipartUpload, BucketLifecycleConfiguration, CorsConfiguration, CorsRule,
        ExpirationStatus, LifecycleRule, LifecycleRuleFilter,
    },
};

use super::{ABANDONED_UPLOAD_DAYS, ObjectStorage, StorageError};

/// The headers a signed request binds, which a browser must be allowed to send.
const SIGNED_HEADERS: &[&str] = &["content-type", "if-none-match"];

impl ObjectStorage {
    /// Create the bucket when it does not exist and set its CORS and lifecycle rules.
    /// Repeating it changes nothing.
    pub async fn prepare(&self) -> Result<(), StorageError> {
        match self
            .internal
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await
        {
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.code(),
                    Some("BucketAlreadyOwnedByYou" | "BucketAlreadyExists")
                ) => {}
            Err(error) => return Err(StorageError::call("CreateBucket", error)),
        }
        if !self.cors_origins.is_empty() {
            let rule = CorsRule::builder()
                .set_allowed_origins(Some(self.cors_origins.clone()))
                .allowed_methods("PUT")
                .allowed_methods("GET")
                .allowed_methods("HEAD")
                .set_allowed_headers(Some(
                    SIGNED_HEADERS
                        .iter()
                        .map(|header| (*header).to_owned())
                        .collect(),
                ))
                .expose_headers("etag")
                .max_age_seconds(3600)
                .build()
                .map_err(|_| StorageError::Config("invalid CORS rule"))?;
            self.internal
                .put_bucket_cors()
                .bucket(&self.bucket)
                .cors_configuration(
                    CorsConfiguration::builder()
                        .cors_rules(rule)
                        .build()
                        .map_err(|_| StorageError::Config("invalid CORS configuration"))?,
                )
                .send()
                .await
                .map_err(|error| StorageError::call("PutBucketCors", error))?;
        }
        let abandoned = LifecycleRule::builder()
            .id("abort-abandoned-multipart-uploads")
            .status(ExpirationStatus::Enabled)
            .filter(LifecycleRuleFilter::builder().prefix("").build())
            .abort_incomplete_multipart_upload(
                AbortIncompleteMultipartUpload::builder()
                    .days_after_initiation(ABANDONED_UPLOAD_DAYS)
                    .build(),
            )
            .build()
            .map_err(|_| StorageError::Config("invalid lifecycle rule"))?;
        self.internal
            .put_bucket_lifecycle_configuration()
            .bucket(&self.bucket)
            .lifecycle_configuration(
                BucketLifecycleConfiguration::builder()
                    .rules(abandoned)
                    .build()
                    .map_err(|_| StorageError::Config("invalid lifecycle configuration"))?,
            )
            .send()
            .await
            .map_err(|error| StorageError::call("PutBucketLifecycleConfiguration", error))?;
        Ok(())
    }

    /// Prepare the bucket, trying again while object storage refuses for at most
    /// `within`. A store that reports healthy can still refuse its first calls while it
    /// starts.
    pub async fn prepare_within(&self, within: std::time::Duration) -> Result<(), StorageError> {
        let deadline = tokio::time::Instant::now() + within;
        let mut pause = std::time::Duration::from_millis(100);
        loop {
            match self.prepare().await {
                Ok(()) => return Ok(()),
                Err(error) if tokio::time::Instant::now() + pause < deadline => {
                    tracing::debug!(%error, "object storage is not ready; preparing again");
                    tokio::time::sleep(pause).await;
                    pause = (pause * 2).min(std::time::Duration::from_secs(5));
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Whether the bucket answers, for readiness.
    pub async fn ready(&self) -> bool {
        self.internal
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_ok()
    }
}
