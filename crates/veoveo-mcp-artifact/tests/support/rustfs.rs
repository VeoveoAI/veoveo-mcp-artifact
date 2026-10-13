//! A private RustFS server for one test process, started from the binary the dev image
//! copies out of the pinned RustFS image, with a fresh bucket for each storage handle.
//! Requires `rustfs` on `PATH`.
#![allow(dead_code)]

use std::{
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use url::Url;
use veoveo_mcp_artifact::storage::{ObjectStorage, StorageConfig};

const ACCESS_KEY: &str = "artifact-test-access";
const SECRET_KEY: &str = "artifact-test-secret-key";

/// One RustFS process and its data directory, removed on drop.
pub struct TestRustFs {
    child: Child,
    dir: PathBuf,
    endpoint: Url,
}

impl TestRustFs {
    /// Start RustFS on a free loopback port and wait until it answers, for at most 30
    /// seconds.
    pub async fn start() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free loopback port")
            .port();
        let dir = std::env::temp_dir().join(format!("artifact-rustfs-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(dir.join("data")).expect("RustFS data directory");
        std::fs::create_dir_all(dir.join("logs")).expect("RustFS log directory");
        let child = Command::new("rustfs")
            .arg("server")
            .arg(dir.join("data"))
            .env("RUSTFS_ADDRESS", format!("127.0.0.1:{port}"))
            .env("RUSTFS_ACCESS_KEY", ACCESS_KEY)
            .env("RUSTFS_SECRET_KEY", SECRET_KEY)
            .env("RUSTFS_OBS_LOG_DIRECTORY", dir.join("logs"))
            .env("RUSTFS_OBS_LOGGER_LEVEL", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start rustfs; the dev image puts it on PATH");
        let endpoint = Url::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let server = Self {
            child,
            dir,
            endpoint,
        };
        server.wait_until_ready().await;
        server
    }

    async fn wait_until_ready(&self) {
        let client = reqwest::Client::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if client.get(self.endpoint.as_str()).send().await.is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "RustFS did not answer within 30 seconds"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// The configuration of a fresh bucket on this server.
    pub fn config(&self) -> StorageConfig {
        StorageConfig {
            internal_endpoint: self.endpoint.clone(),
            signing_endpoint: self.endpoint.clone(),
            region: "us-east-1".into(),
            bucket: format!("t{}", uuid::Uuid::now_v7().simple()),
            access_key: ACCESS_KEY.into(),
            secret_key: SECRET_KEY.into(),
            cors_origins: vec!["https://console.veoveo.test".into()],
        }
    }

    /// A prepared fresh bucket.
    pub async fn storage(&self) -> ObjectStorage {
        let storage = ObjectStorage::new(&self.config()).expect("test storage configuration");
        storage.prepare().await.expect("prepare the test bucket");
        storage
    }
}

impl Drop for TestRustFs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Send a signed transfer the way a client does: the signed method, URL and headers,
/// except `content-length`, which the HTTP client derives from the body it sends.
pub async fn send(
    transfer: &veoveo_mcp_artifact::contract::SignedTransfer,
    body: Vec<u8>,
    extra: &[(&str, &str)],
) -> reqwest::Response {
    let client = reqwest::Client::new();
    let method = reqwest::Method::from_bytes(transfer.method.as_str().as_bytes()).unwrap();
    let mut request = client.request(method, transfer.url.as_str());
    for (name, value) in &transfer.headers {
        if name != "content-length" {
            request = request.header(name, value);
        }
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    request
        .body(body)
        .send()
        .await
        .expect("send a signed request")
}
