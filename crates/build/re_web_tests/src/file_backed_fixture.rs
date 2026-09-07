//! File-backed, allowlisted HTTP fixture for remote-MCAP transport tests.
//!
//! This server is test infrastructure only. It exposes registered regular files by numeric IDs,
//! serves exact single-byte ranges, and never accepts a caller-controlled filesystem path.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{OriginalUri, Path as AxumPath, State};
use axum::http::header::{
    ACCEPT_RANGES, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS,
    ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_EXPOSE_HEADERS, CONTENT_LENGTH, CONTENT_RANGE,
    ETAG, RANGE,
};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::routing::any;
use bytes::Bytes;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::sync::oneshot;

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 64;

/// Deterministic size (bytes) of the browser-harness file-backed fixture.
const FILE_BACKED_CHROME_FIXTURE_LEN: usize = 8192;

/// Immutable manifest entry for one allowlisted file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBackedFixtureEntry {
    /// Numeric fixture identifier used in the URL.
    pub id: u64,
    /// File size in bytes.
    pub length: u64,
    /// Lowercase SHA-256 digest of the complete file.
    pub sha256: String,
}

#[derive(Clone)]
struct FileEntry {
    manifest: FileBackedFixtureEntry,
    path: PathBuf,
}

#[derive(Clone)]
struct FileState {
    root: Arc<PathBuf>,
    files: Arc<Mutex<BTreeMap<u64, FileEntry>>>,
}

/// Running file-backed fixture server.
pub struct FileBackedFixtureServer {
    addr: SocketAddr,
    state: FileState,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    /// Keeps an automatically created fixture directory alive for the server's lifetime.
    /// The field is intentionally never read; dropping it removes the temporary fixture root.
    _temp_dir: Option<tempfile::TempDir>,
}

impl FileBackedFixtureServer {
    /// Starts a loopback server rooted at `root`.
    pub async fn spawn(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::spawn_with_temp_dir(root, None).await
    }

    async fn spawn_with_temp_dir(
        root: impl AsRef<Path>,
        temp_dir: Option<tempfile::TempDir>,
    ) -> anyhow::Result<Self> {
        let root = Arc::new(std::fs::canonicalize(root.as_ref())?);
        anyhow::ensure!(root.is_dir(), "fixture root is not a directory");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = FileState {
            root,
            files: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let app = Router::new()
            .route("/__file_fixture_v1/object/{id}", any(file_response))
            .with_state(state.clone());
        let (shutdown, signal) = oneshot::channel();
        let join = tokio::spawn(async move {
            _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    _ = signal.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            state,
            shutdown: Some(shutdown),
            join: Some(join),
            _temp_dir: temp_dir,
        })
    }

    /// Registers one regular file beneath the server root.
    pub fn register_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> anyhow::Result<FileBackedFixtureEntry> {
        let relative_path = relative_path.as_ref();
        anyhow::ensure!(
            !relative_path.is_absolute(),
            "fixture path must be relative"
        );
        anyhow::ensure!(
            relative_path
                .components()
                .all(|component| { matches!(component, std::path::Component::Normal(_)) }),
            "fixture path contains traversal or non-normal components"
        );
        let path = std::fs::canonicalize(self.state.root.join(relative_path))?;
        anyhow::ensure!(
            path.starts_with(self.state.root.as_ref()),
            "fixture path escapes root"
        );
        let metadata = std::fs::metadata(&path)?;
        anyhow::ensure!(metadata.is_file(), "fixture path is not a regular file");
        anyhow::ensure!(
            metadata.len() <= MAX_FILE_BYTES,
            "fixture exceeds size limit"
        );
        let bytes = std::fs::read(&path)?;
        let digest = Sha256::digest(&bytes);
        let sha256 = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut id_bytes = [0_u8; 8];
        id_bytes.copy_from_slice(&digest[..8]);
        let mut id = u64::from_le_bytes(id_bytes) | 1;
        let mut files = self.state.files.lock();
        anyhow::ensure!(files.len() < MAX_FILES, "fixture file limit exceeded");
        while files.contains_key(&id) {
            id = id.wrapping_add(2);
        }
        let manifest = FileBackedFixtureEntry {
            id,
            length: metadata.len(),
            sha256,
        };
        files.insert(
            id,
            FileEntry {
                manifest: manifest.clone(),
                path,
            },
        );
        Ok(manifest)
    }

    /// Returns the server URL for a registered file.
    pub fn object_url(&self, entry: &FileBackedFixtureEntry) -> String {
        format!("http://{}/__file_fixture_v1/object/{}", self.addr, entry.id)
    }

    /// Returns the immutable manifest for all registered files.
    pub fn manifest(&self) -> Vec<FileBackedFixtureEntry> {
        self.state
            .files
            .lock()
            .values()
            .map(|entry| entry.manifest.clone())
            .collect()
    }

    /// Returns the object-origin socket address.
    pub fn object_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops the server and waits for its task.
    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            _ = join.await;
        }
    }

    /// Spawns a file-backed fixture rooted at a fresh unique directory under the system temp
    /// directory and registers one deterministic byte fixture, so browser harnesses can perform
    /// real cross-origin Range fetches against an on-disk regular file.
    ///
    /// The fixture bytes are deterministic: `bytes[i] = (i * 131 + 17) & 0xff` for
    /// `FILE_BACKED_CHROME_FIXTURE_LEN` bytes. The returned entry carries the registered id,
    /// length, and SHA-256 digest; the object URL is available via [`Self::object_url`].
    pub async fn spawn_chrome_fixture_v1() -> anyhow::Result<(Self, FileBackedFixtureEntry)> {
        let temp_dir = tempfile::Builder::new()
            .prefix("rerun-mcap114-file-backed-chrome-")
            .tempdir()?;
        let root_path = temp_dir.path().to_path_buf();
        let bytes = (0..FILE_BACKED_CHROME_FIXTURE_LEN)
            .map(|index| ((index * 131 + 17) & 0xff) as u8)
            .collect::<Vec<_>>();
        std::fs::write(root_path.join("sample.mcap"), &bytes)?;
        let server = Self::spawn_with_temp_dir(&root_path, Some(temp_dir)).await?;
        let entry = server.register_file("sample.mcap")?;
        Ok((server, entry))
    }
}

impl Drop for FileBackedFixtureServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

/// Minimal CORS surface shared by every response so the browser can observe status codes and
/// headers on success and error responses alike (cross-origin Range tests rely on this).
fn cors_headers() -> Vec<(axum::http::header::HeaderName, axum::http::HeaderValue)> {
    vec![
        (
            ACCESS_CONTROL_ALLOW_ORIGIN,
            axum::http::HeaderValue::from_static("*"),
        ),
        (
            ACCESS_CONTROL_ALLOW_METHODS,
            axum::http::HeaderValue::from_static("GET, HEAD, OPTIONS"),
        ),
        (
            ACCESS_CONTROL_ALLOW_HEADERS,
            axum::http::HeaderValue::from_static("range, if-match"),
        ),
        (
            axum::http::HeaderName::from_static("access-control-allow-private-network"),
            axum::http::HeaderValue::from_static("true"),
        ),
    ]
}

async fn file_response(
    State(state): State<FileState>,
    AxumPath(id): AxumPath<u64>,
    uri: OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let entry = state.files.lock().get(&id).cloned();
    let Some(entry) = entry else {
        return with_cors(
            Response::builder().status(StatusCode::NOT_FOUND),
            cors_headers(),
        )
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()));
    };
    if uri.0.query().is_some() {
        return with_cors(
            Response::builder().status(StatusCode::BAD_REQUEST),
            cors_headers(),
        )
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()));
    }
    if method == Method::OPTIONS {
        return with_cors(
            Response::builder().status(StatusCode::NO_CONTENT),
            cors_headers(),
        )
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()));
    }
    if method != Method::GET && method != Method::HEAD {
        return with_cors(
            Response::builder().status(StatusCode::METHOD_NOT_ALLOWED),
            cors_headers(),
        )
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()));
    }
    let range = headers.get(RANGE).and_then(|value| value.to_str().ok());
    let (start, end) = match range {
        Some(value) => match parse_range(value, entry.manifest.length) {
            Some(range) => range,
            None => {
                return with_cors(
                    Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE),
                    cors_headers(),
                )
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty()));
            }
        },
        None => {
            return with_cors(
                Response::builder().status(StatusCode::RANGE_NOT_SATISFIABLE),
                cors_headers(),
            )
            .body(Body::empty())
            .unwrap_or_else(|_| Response::new(Body::empty()));
        }
    };
    let length = end - start + 1;
    let internal_error = || {
        with_cors(
            Response::builder().status(StatusCode::INTERNAL_SERVER_ERROR),
            cors_headers(),
        )
        .body(Body::empty())
        .unwrap_or_else(|_| Response::new(Body::empty()))
    };
    let body = if method == Method::GET {
        let Ok(bytes) = read_validated_file(&state.root, &entry) else {
            return internal_error();
        };
        Bytes::copy_from_slice(bytes.get(start as usize..=end as usize).unwrap_or_default())
    } else {
        if read_validated_file(&state.root, &entry).is_err() {
            return internal_error();
        }
        Bytes::new()
    };
    with_cors(
        Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(ACCEPT_RANGES, "bytes")
            .header(
                CONTENT_RANGE,
                format!("bytes {start}-{end}/{}", entry.manifest.length),
            )
            .header(CONTENT_LENGTH, length)
            .header(ETAG, format!("\"{}\"", entry.manifest.sha256))
            .header(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                "content-range, content-length, etag",
            )
            .header("cache-control", "no-store"),
        cors_headers(),
    )
    .body(Body::from(body))
    .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// Applies the shared CORS header set to a response builder.
fn with_cors(
    builder: axum::http::response::Builder,
    headers: Vec<(axum::http::header::HeaderName, axum::http::HeaderValue)>,
) -> axum::http::response::Builder {
    headers.into_iter().fold(builder, |builder, (name, value)| {
        builder.header(name, value)
    })
}

fn read_validated_file(root: &Path, entry: &FileEntry) -> anyhow::Result<Vec<u8>> {
    let canonical_before = std::fs::canonicalize(&entry.path)?;
    anyhow::ensure!(
        canonical_before.starts_with(root),
        "fixture path escapes root"
    );
    let metadata_before = std::fs::metadata(&canonical_before)?;
    anyhow::ensure!(
        metadata_before.is_file(),
        "fixture path is not a regular file"
    );
    anyhow::ensure!(
        metadata_before.len() <= MAX_FILE_BYTES,
        "fixture exceeds size limit"
    );
    anyhow::ensure!(
        metadata_before.len() == entry.manifest.length,
        "fixture length changed"
    );

    let bytes = std::fs::read(&canonical_before)?;

    let canonical_after = std::fs::canonicalize(&entry.path)?;
    anyhow::ensure!(
        canonical_after == canonical_before,
        "fixture path changed during read"
    );
    let metadata_after = std::fs::metadata(&canonical_after)?;
    anyhow::ensure!(
        metadata_after.is_file(),
        "fixture path is not a regular file"
    );
    anyhow::ensure!(
        metadata_after.len() <= MAX_FILE_BYTES,
        "fixture exceeds size limit"
    );
    anyhow::ensure!(
        metadata_after.len() == entry.manifest.length,
        "fixture length changed"
    );
    anyhow::ensure!(
        bytes.len() as u64 == entry.manifest.length,
        "fixture bytes length changed"
    );

    let digest = Sha256::digest(&bytes);
    let sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    anyhow::ensure!(sha256 == entry.manifest.sha256, "fixture digest changed");
    Ok(bytes)
}

fn parse_range(value: &str, length: u64) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    if start.is_empty() || end.is_empty() || value.contains(',') {
        return None;
    }
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    (start <= end && end < length).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{CONTENT_RANGE, ETAG, RANGE};

    #[tokio::test]
    async fn file_fixture_serves_exact_range_and_manifest_digest() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("sample.mcap"), b"0123456789").expect("fixture");
        let server = FileBackedFixtureServer::spawn(directory.path())
            .await
            .expect("server");
        let entry = server.register_file("sample.mcap").expect("register");
        let response = reqwest::Client::new()
            .get(server.object_url(&entry))
            .header(RANGE, "bytes=2-5")
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(CONTENT_RANGE).unwrap(),
            "bytes 2-5/10"
        );
        assert_eq!(
            response.headers().get(ETAG).unwrap(),
            &format!("\"{}\"", entry.sha256)
        );
        assert_eq!(response.bytes().await.expect("body").as_ref(), b"2345");
        assert_eq!(entry.sha256, sha256_hex(b"0123456789"));
        server.shutdown().await;
    }

    #[tokio::test]
    async fn file_fixture_rejects_traversal_and_unbounded_requests() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("sample.mcap"), b"0123456789").expect("fixture");
        let server = FileBackedFixtureServer::spawn(directory.path())
            .await
            .expect("server");
        assert!(server.register_file("../sample.mcap").is_err());
        let entry = server.register_file("sample.mcap").expect("register");
        let client = reqwest::Client::new();
        let no_range = client
            .get(server.object_url(&entry))
            .send()
            .await
            .expect("request");
        assert_eq!(no_range.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        let query = "?fixture_secret=forbidden";
        let query_response = client
            .get(format!("{}{}", server.object_url(&entry), query))
            .header(RANGE, "bytes=0-1")
            .send()
            .await
            .expect("query request");
        assert_eq!(query_response.status(), StatusCode::BAD_REQUEST);
        let preflight = client
            .request(reqwest::Method::OPTIONS, server.object_url(&entry))
            .send()
            .await
            .expect("preflight request");
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight
                .headers()
                .get(ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
        let multi = client
            .get(server.object_url(&entry))
            .header(RANGE, "bytes=0-1,2-3")
            .send()
            .await
            .expect("request");
        assert_eq!(multi.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn file_fixture_rejects_runtime_path_replacement() {
        let directory = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        std::fs::write(directory.path().join("sample.mcap"), b"0123456789").expect("fixture");
        std::fs::write(outside.path().join("outside.mcap"), b"outside!!!")
            .expect("outside fixture");
        let server = FileBackedFixtureServer::spawn(directory.path())
            .await
            .expect("server");
        let entry = server.register_file("sample.mcap").expect("register");
        std::fs::remove_file(directory.path().join("sample.mcap")).expect("remove fixture");
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("outside.mcap"),
            directory.path().join("sample.mcap"),
        )
        .expect("replace with outside symlink");
        let response = reqwest::Client::new()
            .get(server.object_url(&entry))
            .header(RANGE, "bytes=0-1")
            .send()
            .await
            .expect("request");
        #[cfg(unix)]
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn file_fixture_supports_head_headers_without_body() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("sample.mcap"), b"0123456789").expect("fixture");
        let server = FileBackedFixtureServer::spawn(directory.path())
            .await
            .expect("server");
        let entry = server.register_file("sample.mcap").expect("register");
        let response = reqwest::Client::new()
            .head(server.object_url(&entry))
            .header(RANGE, "bytes=2-5")
            .send()
            .await
            .expect("request");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), "4");
        assert_eq!(response.headers().get(ACCEPT_RANGES).unwrap(), "bytes");
        assert_eq!(response.bytes().await.expect("body").len(), 0);
        server.shutdown().await;
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
