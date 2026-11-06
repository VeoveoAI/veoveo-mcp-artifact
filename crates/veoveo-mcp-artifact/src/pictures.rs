//! Small pictures of artifacts this server was handed, kept by artifact, whose bytes never change.
//! This server decides who may read an artifact, so a kept picture needs no call to any other
//! server. It draws nothing, and all of it can be fetched again.
use std::path::PathBuf;

use crate::contract::ArtifactId;

#[derive(Debug, Clone)]
pub struct KeptPictures {
    folder: PathBuf,
}

const MEDIA_TYPE: &str = "image/webp";

impl KeptPictures {
    pub fn new(folder: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&folder)?;
        Ok(Self { folder })
    }

    fn path(&self, artifact: ArtifactId) -> PathBuf {
        self.folder.join(format!("{artifact}.webp"))
    }

    /// The picture's media type and its base64.
    pub async fn get(&self, artifact: ArtifactId) -> Option<(String, String)> {
        let data = tokio::fs::read_to_string(self.path(artifact)).await.ok()?;
        Some((MEDIA_TYPE.to_owned(), data))
    }

    /// Keeps a picture, whole or not at all. A failure costs a later fetch.
    pub async fn keep(&self, artifact: ArtifactId, media_type: &str, data: &str) {
        if media_type != MEDIA_TYPE {
            return;
        }
        let partial = self
            .folder
            .join(format!("{artifact}.partial-{}", std::process::id()));
        if tokio::fs::write(&partial, data).await.is_ok() {
            let _ = tokio::fs::rename(&partial, self.path(artifact)).await;
        }
    }
}
