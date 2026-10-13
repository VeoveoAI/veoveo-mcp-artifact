//! Downloads: `create_download` signs a GET, and the client streams the bytes from
//! object storage, whole or one byte range.
use std::{
    future::IntoFuture,
    io,
    ops::RangeInclusive,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt, future::BoxFuture, stream::BoxStream};
use tokio::io::AsyncRead;

use crate::contract::{ArtifactId, ArtifactRecord, CreateDownloadRequest};

use super::{
    ArtifactClient, ArtifactClientError,
    parts::{ATTEMPTS, Attempt, back_off, send},
};

/// The bytes a download asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteRange {
    Between(u64, u64),
    From(u64),
}

impl ByteRange {
    /// The RFC 9110 `Range` header value.
    fn header(self) -> String {
        match self {
            Self::Between(first, last) => format!("bytes={first}-{last}"),
            Self::From(first) => format!("bytes={first}-"),
        }
    }
}

/// A download that runs when it is awaited and resolves once object storage starts
/// sending the bytes.
#[must_use = "a download runs when it is awaited"]
#[derive(Debug)]
pub struct Download {
    client: ArtifactClient,
    artifact_id: ArtifactId,
    range: Option<ByteRange>,
}

impl Download {
    pub(super) fn new(client: ArtifactClient, artifact_id: ArtifactId) -> Self {
        Self {
            client,
            artifact_id,
            range: None,
        }
    }

    /// Only the bytes from offset `first` through `last`, both included.
    pub fn range(mut self, range: RangeInclusive<u64>) -> Self {
        self.range = Some(ByteRange::Between(*range.start(), *range.end()));
        self
    }

    /// Only the bytes from offset `first` to the end.
    pub fn range_from(mut self, first: u64) -> Self {
        self.range = Some(ByteRange::From(first));
        self
    }

    async fn run(self) -> Result<Downloaded, ArtifactClientError> {
        let signed = self
            .client
            .calls
            .call(CreateDownloadRequest {
                artifact_id: self.artifact_id,
            })
            .await?;
        let range = self.range.map(ByteRange::header);
        let mut attempt = 1;
        let response = loop {
            match send(&self.client.http, &signed.get, None, range.as_deref()).await {
                Attempt::Accepted(response) => break response,
                Attempt::Transient(_) if attempt < ATTEMPTS => {
                    back_off(attempt).await;
                    attempt += 1;
                }
                Attempt::Transient(error)
                | Attempt::SignatureRefused(error)
                | Attempt::Refused(error) => return Err(error),
            }
        };
        let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if range.is_some() != partial {
            return Err(ArtifactClientError::Protocol(
                "object storage answered another range than the one requested",
            ));
        }
        Ok(Downloaded {
            artifact: signed.artifact,
            length: response.content_length(),
            body: DownloadStream(
                response
                    .bytes_stream()
                    .map_err(ArtifactClientError::unreachable)
                    .boxed(),
            ),
        })
    }
}

impl IntoFuture for Download {
    type Output = Result<Downloaded, ArtifactClientError>;
    type IntoFuture = BoxFuture<'static, Self::Output>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.run())
    }
}

/// An artifact and its bytes as object storage sends them.
#[derive(Debug)]
pub struct Downloaded {
    pub artifact: ArtifactRecord,
    /// The number of bytes the body holds, when object storage states it.
    pub length: Option<u64>,
    pub body: DownloadStream,
}

/// The bytes of a download, chunk by chunk.
pub struct DownloadStream(BoxStream<'static, Result<Bytes, ArtifactClientError>>);

impl DownloadStream {
    /// The bytes as a reader.
    pub fn into_reader(self) -> impl AsyncRead + Send + Unpin {
        tokio_util::io::StreamReader::new(self.0.map_err(io::Error::other))
    }

    /// Every byte of the body in memory, for a body the caller knows is small.
    pub async fn collect(mut self) -> Result<Vec<u8>, ArtifactClientError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = self.0.next().await {
            bytes.extend_from_slice(&chunk?);
        }
        Ok(bytes)
    }
}

impl Stream for DownloadStream {
    type Item = Result<Bytes, ArtifactClientError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.as_mut().poll_next(context)
    }
}

impl std::fmt::Debug for DownloadStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DownloadStream")
    }
}
