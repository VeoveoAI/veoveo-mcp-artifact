//! Uploads from a byte stream of known or unknown size. A body of at most one part is
//! read into memory first, so its size and SHA-256 travel with `create_upload` and
//! object storage checks them during one PUT. A larger body streams in parts of the
//! upload's part size; memory holds at most `parallelism` parts at once.
use std::{future::IntoFuture, io, num::NonZeroUsize, pin::Pin, sync::Arc, time::Duration};

use bytes::{Bytes, BytesMut};
use futures::{Stream, future::BoxFuture};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};
use veoveo_types::Sha256Digest;

use crate::contract::{
    AbortUploadRequest, ArtifactLabels, ArtifactMetadataMap, ArtifactName, ArtifactRecord,
    ArtifactState, CompleteUploadRequest, CreateUploadRequest, DerivedFrom, IdempotencyKey,
    MediaType, ObjectSize, PartNumber, PartSize, ReadArtifact, RetentionDays, UploadId,
    UploadTransfer,
};

use super::{
    ArtifactClient, ArtifactClientError,
    parts::{PartSigner, put_single},
};

/// The largest body sent in one PUT, which the client holds in memory to declare its
/// size and SHA-256: one default part, 16 MiB.
pub const SINGLE_PUT_BYTES: u64 = PartSize::DEFAULT;
/// The part bytes a multipart upload holds in memory unless its parallelism is set:
/// four default parts, or one part when a part is larger.
pub const DEFAULT_IN_FLIGHT_BYTES: u64 = 4 * PartSize::DEFAULT;

/// The bytes of an upload: a reader, a stream of chunks, or bytes in memory.
pub struct UploadBody {
    reader: Pin<Box<dyn AsyncRead + Send>>,
}

impl UploadBody {
    /// The bytes `reader` yields until its end.
    pub fn reader(reader: impl AsyncRead + Send + 'static) -> Self {
        Self {
            reader: Box::pin(reader),
        }
    }

    /// The chunks `stream` yields until its end, such as a response's body stream.
    pub fn stream(stream: impl Stream<Item = io::Result<Bytes>> + Send + 'static) -> Self {
        Self::reader(tokio_util::io::StreamReader::new(stream))
    }

    /// Bytes already in memory.
    pub fn bytes(bytes: impl Into<Bytes>) -> Self {
        Self::reader(io::Cursor::new(bytes.into()))
    }
}

impl std::fmt::Debug for UploadBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UploadBody")
    }
}

/// An upload that runs when it is awaited and resolves with the artifact: available,
/// or verifying while Artifact computes the SHA-256 of a large object, unless the upload
/// waits for the verification with [`Upload::until_verified`]. A failure after the upload
/// was created aborts it, which releases its reserved bytes.
#[must_use = "an upload runs when it is awaited"]
#[derive(Debug)]
pub struct Upload {
    client: ArtifactClient,
    request: CreateUploadRequest,
    body: UploadBody,
    parallelism: Option<NonZeroUsize>,
    verification: Option<Duration>,
}

impl Upload {
    pub(super) fn new(
        client: ArtifactClient,
        name: ArtifactName,
        media_type: MediaType,
        body: UploadBody,
    ) -> Self {
        Self {
            client,
            request: CreateUploadRequest {
                name,
                media_type,
                size: None,
                sha256: None,
                max_size: None,
                labels: ArtifactLabels::default(),
                retention_days: None,
                metadata: ArtifactMetadataMap::default(),
                derived_from: DerivedFrom::default(),
                idempotency_key: None,
            },
            body,
            parallelism: None,
            verification: None,
        }
    }

    /// The body's exact size, when the caller knows it. A body of another size fails
    /// the upload.
    pub fn size(mut self, size: ObjectSize) -> Self {
        self.request.size = Some(size);
        self
    }

    /// The most bytes the body holds, when the caller does not know its size. Artifact
    /// reserves this bound; without it, the tenant's largest object is reserved.
    pub fn max_size(mut self, max_size: ObjectSize) -> Self {
        self.request.max_size = Some(max_size);
        self
    }

    /// The body's SHA-256, when the caller knows it. Artifact computes it otherwise.
    pub fn sha256(mut self, sha256: Sha256Digest) -> Self {
        self.request.sha256 = Some(sha256);
        self
    }

    pub fn labels(mut self, labels: ArtifactLabels) -> Self {
        self.request.labels = labels;
        self
    }

    pub fn retention(mut self, days: RetentionDays) -> Self {
        self.request.retention_days = Some(days);
        self
    }

    pub fn metadata(mut self, metadata: ArtifactMetadataMap) -> Self {
        self.request.metadata = metadata;
        self
    }

    pub fn derived_from(mut self, sources: DerivedFrom) -> Self {
        self.request.derived_from = sources;
        self
    }

    /// A key that makes a repeated upload return the first one's artifact.
    pub fn idempotency_key(mut self, key: IdempotencyKey) -> Self {
        self.request.idempotency_key = Some(key);
        self
    }

    /// How many parts of a multipart upload are in memory and in flight at once.
    pub fn parallelism(mut self, parts: NonZeroUsize) -> Self {
        self.parallelism = Some(parts);
        self
    }

    /// Waits up to `timeout` for Artifact to verify a large object, so the upload
    /// resolves with an available artifact or [`ArtifactClientError::VerificationFailed`]
    /// instead of a verifying one. The wait follows the artifact's resource updates
    /// rather than polling, which needs the caller's `artifact.artifacts.read`; a wait
    /// that ends first fails with a timeout, and the artifact keeps verifying.
    pub fn until_verified(mut self, timeout: Duration) -> Self {
        self.verification = Some(timeout);
        self
    }

    async fn run(self) -> Result<ArtifactRecord, ArtifactClientError> {
        let verification = self.verification;
        let client = self.client.clone();
        let artifact = self.transfer().await?;
        match verification {
            Some(timeout) if artifact.state == ArtifactState::Verifying => {
                verified(&client, artifact, timeout).await
            }
            _ => Ok(artifact),
        }
    }

    async fn transfer(self) -> Result<ArtifactRecord, ArtifactClientError> {
        let Upload {
            client,
            request,
            body,
            parallelism,
            verification: _,
        } = self;
        let mut reader = body.reader;
        // One byte past the single-PUT bound tells whether the body fits one PUT.
        let probe = match request.size {
            Some(size) if size.bytes() <= SINGLE_PUT_BYTES => size.bytes(),
            _ => SINGLE_PUT_BYTES,
        } + 1;
        let mut first = BytesMut::new();
        if read_up_to(&mut reader, &mut first, probe).await? {
            return single(&client, request, first.freeze()).await;
        }
        if let Some(size) = request.size
            && size.bytes() < probe
        {
            return Err(ArtifactClientError::TooLarge {
                limit: size.bytes(),
            });
        }
        multipart(&client, request, first, reader, parallelism).await
    }
}

impl IntoFuture for Upload {
    type Output = Result<ArtifactRecord, ArtifactClientError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// The artifact once Artifact has verified it: available, or a failure when the bytes
/// did not verify.
async fn verified(
    client: &ArtifactClient,
    artifact: ArtifactRecord,
    timeout: Duration,
) -> Result<ArtifactRecord, ArtifactClientError> {
    let entry = client
        .calls
        .read(ReadArtifact {
            artifact: artifact.artifact_id,
        })
        .until(|entry| entry.artifact.state != ArtifactState::Verifying)
        .timeout(timeout)
        .await?;
    match entry.artifact.state {
        ArtifactState::Available => Ok(entry.artifact),
        _ => Err(ArtifactClientError::VerificationFailed {
            artifact_id: entry.artifact.artifact_id,
        }),
    }
}

/// Read from `reader` into `buffer` until it holds `limit` bytes or the reader ends.
/// True when the reader ended.
async fn read_up_to(
    reader: &mut Pin<Box<dyn AsyncRead + Send>>,
    buffer: &mut BytesMut,
    limit: u64,
) -> Result<bool, ArtifactClientError> {
    let limit = usize::try_from(limit).map_err(|_| ArtifactClientError::TooLarge {
        limit: usize::MAX as u64,
    })?;
    while buffer.len() < limit {
        let wanted = limit - buffer.len();
        buffer.reserve(wanted);
        let read = (&mut *reader)
            .take(wanted as u64)
            .read_buf(buffer)
            .await
            .map_err(ArtifactClientError::Body)?;
        if read == 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sha256(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

/// A body that fits one PUT: its size and SHA-256 travel with `create_upload`, and
/// object storage refuses other bytes.
async fn single(
    client: &ArtifactClient,
    mut request: CreateUploadRequest,
    body: Bytes,
) -> Result<ArtifactRecord, ArtifactClientError> {
    let read = body.len() as u64;
    if let Some(declared) = request.size
        && declared.bytes() != read
    {
        return Err(ArtifactClientError::SizeMismatch {
            declared: declared.bytes(),
            read,
        });
    }
    if let Some(max_size) = request.max_size
        && read > max_size.bytes()
    {
        return Err(ArtifactClientError::TooLarge {
            limit: max_size.bytes(),
        });
    }
    let computed = sha256(&body);
    if let Some(declared) = &request.sha256
        && declared != &computed
    {
        return Err(ArtifactClientError::DigestMismatch {
            declared: declared.clone(),
            computed,
        });
    }
    request.size = Some(
        ObjectSize::new(read)
            .map_err(|_| ArtifactClientError::Protocol("a single body exceeds an object"))?,
    );
    request.sha256 = Some(computed);
    let created = client.calls.call(request).await?.metadata().clone();
    let upload_id = created.upload.upload_id;
    let Some(transfer) = created.transfer else {
        // A repeated idempotency key found the upload ended; the artifact holds its outcome.
        return Ok(created.artifact);
    };
    let UploadTransfer::Single { put } = transfer else {
        abort(client, upload_id).await;
        return Err(ArtifactClientError::Protocol(
            "a body of one PUT received a multipart upload",
        ));
    };
    let outcome = async {
        put_single(&client.http, &put, body).await?;
        Ok(client
            .calls
            .call(CompleteUploadRequest { upload_id })
            .await?
            .metadata()
            .artifact
            .clone())
    }
    .await;
    if outcome.is_err() {
        abort(client, upload_id).await;
    }
    outcome
}

/// A body larger than one PUT, streamed in the upload's parts. A known size becomes the
/// upload's bound, so every part can be signed and sent again on its own.
async fn multipart(
    client: &ArtifactClient,
    mut request: CreateUploadRequest,
    first: BytesMut,
    reader: Pin<Box<dyn AsyncRead + Send>>,
    parallelism: Option<NonZeroUsize>,
) -> Result<ArtifactRecord, ArtifactClientError> {
    let declared = request.size.take();
    if let Some(size) = declared {
        if let Some(max_size) = request.max_size
            && size > max_size
        {
            return Err(ArtifactClientError::TooLarge {
                limit: max_size.bytes(),
            });
        }
        request.max_size = Some(size);
    }
    let created = client.calls.call(request).await?.metadata().clone();
    let upload_id = created.upload.upload_id;
    let Some(transfer) = created.transfer else {
        return Ok(created.artifact);
    };
    let UploadTransfer::Multipart { part_size, parts } = transfer else {
        abort(client, upload_id).await;
        return Err(ArtifactClientError::Protocol(
            "a body beyond one PUT received a single upload",
        ));
    };
    let bound = created.upload.reserved.bytes();
    let outcome = async {
        let parts = PartSigner::new(
            client.calls.clone(),
            upload_id,
            last_part(bound, part_size),
            parts,
        );
        let read = send_parts(
            client,
            Arc::new(parts),
            part_size,
            bound,
            first,
            reader,
            parallelism,
        )
        .await?;
        if let Some(declared) = declared
            && declared.bytes() != read
        {
            return Err(ArtifactClientError::SizeMismatch {
                declared: declared.bytes(),
                read,
            });
        }
        Ok(client
            .calls
            .call(CompleteUploadRequest { upload_id })
            .await?
            .metadata()
            .artifact
            .clone())
    }
    .await;
    if outcome.is_err() {
        abort(client, upload_id).await;
    }
    outcome
}

/// The highest part number an upload bounded by `bound` bytes needs.
fn last_part(bound: u64, part_size: PartSize) -> u16 {
    let parts = bound
        .div_ceil(part_size.bytes())
        .clamp(1, u64::from(PartNumber::MAX));
    u16::try_from(parts).expect("at most 10000 parts")
}

/// Read the body part by part and send each part while the next one is read. A part
/// takes a permit before it is read, so at most `parallelism` parts are in memory, and a
/// failed part stops the reading. Resolves with the number of bytes sent.
async fn send_parts(
    client: &ArtifactClient,
    signer: Arc<PartSigner>,
    part_size: PartSize,
    bound: u64,
    mut carry: BytesMut,
    mut reader: Pin<Box<dyn AsyncRead + Send>>,
    parallelism: Option<NonZeroUsize>,
) -> Result<u64, ArtifactClientError> {
    let size = usize::try_from(part_size.bytes())
        .map_err(|_| ArtifactClientError::Protocol("a part larger than memory addresses"))?;
    let parallelism = parallelism.map_or_else(
        || usize::try_from((DEFAULT_IN_FLIGHT_BYTES / part_size.bytes()).max(1)).unwrap_or(1),
        NonZeroUsize::get,
    );
    let permits = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let mut sending = tokio::task::JoinSet::new();
    let mut sent = 0u64;
    let mut number = 1u16;
    loop {
        while let Some(finished) = sending.try_join_next() {
            joined(finished)?;
        }
        let permit = permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ArtifactClientError::Protocol("the part permits closed"))?;
        let mut part = if carry.len() >= size {
            let rest = carry.split_off(size);
            std::mem::replace(&mut carry, rest)
        } else {
            std::mem::take(&mut carry)
        };
        let ended = part.len() < size && read_up_to(&mut reader, &mut part, size as u64).await?;
        if part.is_empty() {
            break;
        }
        sent += part.len() as u64;
        if sent > bound {
            return Err(ArtifactClientError::TooLarge { limit: bound });
        }
        let part_number =
            PartNumber::new(number).map_err(|_| ArtifactClientError::TooLarge { limit: bound })?;
        let signer = signer.clone();
        let http = client.http.clone();
        let body = part.freeze();
        sending.spawn(async move {
            let _permit = permit;
            signer.send_part(&http, part_number, body).await
        });
        if ended {
            break;
        }
        number = number
            .checked_add(1)
            .ok_or(ArtifactClientError::TooLarge { limit: bound })?;
    }
    while let Some(finished) = sending.join_next().await {
        joined(finished)?;
    }
    Ok(sent)
}

fn joined(
    finished: Result<Result<(), ArtifactClientError>, tokio::task::JoinError>,
) -> Result<(), ArtifactClientError> {
    finished.map_err(|_| ArtifactClientError::Protocol("a part transfer stopped"))?
}

/// Abort an upload that cannot finish, so its reserved bytes are released at once. A
/// failed abort leaves the upload to expire.
async fn abort(client: &ArtifactClient, upload_id: UploadId) {
    let _ = client.calls.call(AbortUploadRequest { upload_id }).await;
}
