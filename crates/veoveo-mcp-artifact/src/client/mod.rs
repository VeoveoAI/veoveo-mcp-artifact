//! `ArtifactClient`: uploads and downloads built on Artifact's typed calls. The calls go
//! through the Gateway for the caller a `GatewayClient` is bound to: the request a
//! handler serves (`ctx.client()`), a Task (`ServerCalls::for_task`), or a program with
//! its own access token. The bytes go straight to object storage through the URLs
//! Artifact signs, and the Gateway carries none of them.
//!
//! ```ignore
//! let artifacts = ArtifactClient::new(ctx.client()?, http);
//! let artifact = artifacts
//!     .upload(name, media_type, UploadBody::stream(response.bytes_stream()))
//!     .labels(labels)
//!     .await?;
//! let download = artifacts.download(artifact.artifact_id).range(0..=1023).await?;
//! ```
mod download;
mod parts;
mod upload;

use veoveo_sdk_rust::client::{CallError, GatewayClient};
use veoveo_types::Sha256Digest;

use crate::contract::{ArtifactId, ArtifactName, MediaType};

pub use download::{Download, DownloadStream, Downloaded};
pub use upload::{DEFAULT_IN_FLIGHT_BYTES, SINGLE_PUT_BYTES, Upload, UploadBody};

/// Uploads and downloads of artifacts through one Gateway binding.
#[derive(Clone, Debug)]
pub struct ArtifactClient {
    calls: GatewayClient,
    http: reqwest::Client,
}

impl ArtifactClient {
    /// Artifact through `calls`, moving bytes with `http`. `http` reaches object storage
    /// directly and carries the TLS configuration the program chose.
    pub fn new(calls: GatewayClient, http: reqwest::Client) -> Self {
        Self { calls, http }
    }

    /// The Gateway binding the client calls Artifact through, for Artifact's other
    /// tools, such as `grant_artifact`.
    pub fn calls(&self) -> &GatewayClient {
        &self.calls
    }

    /// Upload `body` as a new artifact named `name` of `media_type`. The upload runs when
    /// it is awaited and resolves with the artifact, available or verifying.
    pub fn upload(&self, name: ArtifactName, media_type: MediaType, body: UploadBody) -> Upload {
        Upload::new(self.clone(), name, media_type, body)
    }

    /// Download an artifact's bytes. The download runs when it is awaited and resolves
    /// with the artifact and a stream of its bytes.
    pub fn download(&self, artifact_id: ArtifactId) -> Download {
        Download::new(self.clone(), artifact_id)
    }
}

/// Why an upload or a download did not finish.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactClientError {
    /// A call to Artifact through the Gateway failed or was refused.
    #[error(transparent)]
    Call(#[from] CallError),
    /// Object storage refused a signed request or could not be reached. The message
    /// never carries the signed URL.
    #[error("object storage {}: {message}", status.map_or_else(|| "was unreachable".to_owned(), |status| format!("answered {status}")))]
    Storage {
        status: Option<u16>,
        message: String,
    },
    /// The upload's body could not be read.
    #[error("reading the upload's body failed: {0}")]
    Body(#[source] std::io::Error),
    /// The body held another number of bytes than its declared size.
    #[error("the body held {read} bytes, not the declared {declared}")]
    SizeMismatch { declared: u64, read: u64 },
    /// The body holds more bytes than the upload may receive.
    #[error("the body exceeds the upload's bound of {limit} bytes")]
    TooLarge { limit: u64 },
    /// The body's SHA-256 differs from the declared one.
    #[error("the body's SHA-256 is {computed}, not the declared {declared}")]
    DigestMismatch {
        declared: Sha256Digest,
        computed: Sha256Digest,
    },
    /// Artifact answered with something its contract does not describe for this request.
    #[error("Artifact answered unexpectedly: {0}")]
    Protocol(&'static str),
}

impl ArtifactClientError {
    /// A transport failure of a signed request, without the URL, which carries the
    /// signature.
    fn unreachable(error: reqwest::Error) -> Self {
        Self::Storage {
            status: error.status().map(|status| status.as_u16()),
            message: error.without_url().to_string(),
        }
    }
}
