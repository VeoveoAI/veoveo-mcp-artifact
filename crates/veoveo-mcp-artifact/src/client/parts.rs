//! Signed requests to object storage: one request sent exactly as Artifact signed it,
//! its retries, and the signatures of a multipart upload's parts, which Artifact signs
//! in batches as the upload reaches them.
use std::{collections::BTreeMap, time::Duration};

use bytes::Bytes;
use chrono::{TimeDelta, Utc};
use veoveo_hub_sdk::client::HubClient;

use crate::contract::{
    MAX_PARTS_PER_SIGNING, PartNumber, PartRequest, SignUploadPartsRequest, SignedPart,
    SignedTransfer, TransferMethod, UploadId,
};

use super::ArtifactClientError;

/// How many times one signed request is sent before the transfer fails.
pub(super) const ATTEMPTS: u32 = 4;
/// The wait before the first retry, doubled before each later one.
const FIRST_BACKOFF: Duration = Duration::from_millis(250);
/// A signature that expires sooner than this is signed again before it is sent.
const FRESHNESS: TimeDelta = TimeDelta::seconds(60);
/// The most bytes of an object-storage error body the client reads for its code.
const ERROR_BODY_BYTES: usize = 4096;

/// How one signed request ended.
pub(super) enum Attempt {
    /// Object storage accepted it.
    Accepted(reqwest::Response),
    /// Object storage refused the signature, as it refuses an expired one.
    SignatureRefused(ArtifactClientError),
    /// Sending it again may succeed: the network failed, or storage was busy or failed.
    Transient(ArtifactClientError),
    /// Sending it again cannot succeed.
    Refused(ArtifactClientError),
}

/// Send `transfer` with exactly the method, URL and headers Artifact signed. The HTTP
/// client sets the content length the signature binds from `body`.
pub(super) async fn send(
    http: &reqwest::Client,
    transfer: &SignedTransfer,
    body: Option<Bytes>,
    range: Option<&str>,
) -> Attempt {
    let method = match transfer.method {
        TransferMethod::Put => reqwest::Method::PUT,
        TransferMethod::Get => reqwest::Method::GET,
    };
    let mut request = http.request(method, transfer.url.as_str());
    for (name, value) in &transfer.headers {
        if !name.eq_ignore_ascii_case("content-length") {
            request = request.header(name, value);
        }
    }
    if let Some(range) = range {
        request = request.header(reqwest::header::RANGE, range);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => return Attempt::Transient(ArtifactClientError::unreachable(error)),
    };
    let status = response.status();
    if status.is_success() {
        return Attempt::Accepted(response);
    }
    let error = ArtifactClientError::Storage {
        status: Some(status.as_u16()),
        message: error_code(response).await,
    };
    match status.as_u16() {
        403 => Attempt::SignatureRefused(error),
        408 | 429 | 500..=599 => Attempt::Transient(error),
        _ => Attempt::Refused(error),
    }
}

/// The S3 error code of a refusal, such as `SignatureDoesNotMatch`, or the status's
/// reason when the body names none.
async fn error_code(mut response: reqwest::Response) -> String {
    let reason = response
        .status()
        .canonical_reason()
        .unwrap_or("refused")
        .to_owned();
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    let text = String::from_utf8_lossy(&body);
    text.split_once("<Code>")
        .and_then(|(_, rest)| rest.split_once("</Code>"))
        .map(|(code, _)| code.trim().to_owned())
        .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or(reason)
}

/// Wait before retry `attempt`, which counts from 1 for the first retry.
pub(super) async fn back_off(attempt: u32) {
    tokio::time::sleep(FIRST_BACKOFF * 2u32.saturating_pow(attempt.saturating_sub(1))).await;
}

/// Send the one PUT of a single upload, retrying transient failures. The PUT is
/// create-only, so a retry that finds the object already stored means an earlier
/// attempt stored it; completing the upload then checks its size and SHA-256.
pub(super) async fn put_single(
    http: &reqwest::Client,
    put: &SignedTransfer,
    body: Bytes,
) -> Result<(), ArtifactClientError> {
    let mut attempt = 1;
    loop {
        match send(http, put, Some(body.clone()), None).await {
            Attempt::Accepted(_) => return Ok(()),
            Attempt::Refused(ArtifactClientError::Storage {
                status: Some(412), ..
            }) if attempt > 1 => return Ok(()),
            Attempt::Transient(_) if attempt < ATTEMPTS => {
                back_off(attempt).await;
                attempt += 1;
            }
            Attempt::Transient(error)
            | Attempt::SignatureRefused(error)
            | Attempt::Refused(error) => return Err(error),
        }
    }
}

/// The signatures of one multipart upload's parts. A part is signed together with the
/// parts after it, up to 100 at once, when the upload first needs it or when its
/// signature is about to expire.
pub(super) struct PartSigner {
    calls: HubClient,
    upload_id: UploadId,
    /// The highest part number the upload's bound allows.
    last_part: u16,
    signed: tokio::sync::Mutex<BTreeMap<PartNumber, SignedTransfer>>,
}

impl PartSigner {
    pub(super) fn new(
        calls: HubClient,
        upload_id: UploadId,
        last_part: u16,
        initial: Vec<SignedPart>,
    ) -> Self {
        Self {
            calls,
            upload_id,
            last_part,
            signed: tokio::sync::Mutex::new(
                initial
                    .into_iter()
                    .map(|part| (part.part_number, part.transfer))
                    .collect(),
            ),
        }
    }

    /// A signature of `part` that stays valid for at least a minute.
    async fn signature(&self, part: PartNumber) -> Result<SignedTransfer, ArtifactClientError> {
        let mut signed = self.signed.lock().await;
        if let Some(transfer) = signed.get(&part)
            && transfer.expires_at - Utc::now() > FRESHNESS
        {
            return Ok(transfer.clone());
        }
        let batch = u16::try_from(MAX_PARTS_PER_SIGNING).expect("100 parts fit a part number");
        let last = part
            .get()
            .saturating_add(batch - 1)
            .min(self.last_part)
            .max(part.get());
        let parts = (part.get()..=last)
            .map(|number| {
                PartNumber::new(number)
                    .map(|part_number| PartRequest {
                        part_number,
                        size: None,
                        sha256: None,
                    })
                    .map_err(|_| ArtifactClientError::Protocol("a part number beyond 10000"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let answer = self
            .calls
            .call(SignUploadPartsRequest {
                upload_id: self.upload_id,
                parts,
            })
            .await?;
        for part in answer.parts {
            signed.insert(part.part_number, part.transfer);
        }
        signed
            .get(&part)
            .cloned()
            .ok_or(ArtifactClientError::Protocol(
                "the signed parts lack the part the upload asked for",
            ))
    }

    /// Sign `part` again after object storage refused its signature.
    async fn sign_again(&self, part: PartNumber) -> Result<SignedTransfer, ArtifactClientError> {
        self.signed.lock().await.remove(&part);
        self.signature(part).await
    }

    /// Forget the signature of a part object storage holds.
    async fn sent(&self, part: PartNumber) {
        self.signed.lock().await.remove(&part);
    }

    /// Send one part, retrying transient failures and signing it again when object
    /// storage refuses its signature.
    pub(super) async fn send_part(
        &self,
        http: &reqwest::Client,
        part: PartNumber,
        body: Bytes,
    ) -> Result<(), ArtifactClientError> {
        let mut transfer = self.signature(part).await?;
        let mut attempt = 1;
        loop {
            match send(http, &transfer, Some(body.clone()), None).await {
                Attempt::Accepted(_) => {
                    self.sent(part).await;
                    return Ok(());
                }
                Attempt::SignatureRefused(_) if attempt < ATTEMPTS => {
                    transfer = self.sign_again(part).await?;
                    attempt += 1;
                }
                Attempt::Transient(_) if attempt < ATTEMPTS => {
                    back_off(attempt).await;
                    attempt += 1;
                }
                Attempt::SignatureRefused(error)
                | Attempt::Transient(error)
                | Attempt::Refused(error) => return Err(error),
            }
        }
    }
}
