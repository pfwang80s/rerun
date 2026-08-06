//! Controlled HTTP origins used by Chrome Range/CORS tests.
//!
//! This is deliberately test infrastructure, not a general-purpose HTTP server.
//! It binds two loopback-only random ports so tests can exercise real same-origin and
//! cross-origin behavior while retaining deterministic, bounded control over responses.

use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, OriginalUri, Path, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, LOCATION,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use parking_lot::{Mutex, MutexGuard};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, oneshot, watch};

const ROUTE_PREFIX: &str = "/__mcap_range_fixture/v1";
const PROTOCOL_VERSION: &str = "mcap-range-fixture-v1";
const MAX_SCENARIOS: usize = 128;
const MAX_CONTROL_BODY_BYTES: usize = 64 * 1024;
const MAX_BROWSER_EVENT_BYTES: usize = 1024;
const MAX_EVENTS_PER_SCENARIO: usize = 512;
const MAX_EVENT_BYTES_PER_SCENARIO: usize = 256 * 1024;
const MAX_HEADER_VALUE_BYTES: usize = 512;
const MAX_OBJECT_BYTES: u32 = 8 * 1024 * 1024;
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BODY_CHUNKS: usize = 256;
const MAX_RESPONSE_REVISIONS: usize = 16;
const MAX_ACTIVE_BODIES: usize = 64;
const MAX_ACTIVE_BODIES_PER_SCENARIO: usize = 8;
const MAX_CHUNK_BYTES: u32 = 64 * 1024;
const MAX_DELAY_MS: u32 = 10_000;
const MAX_GATE_ID: u16 = 31;
const GATE_SLOTS: usize = MAX_GATE_ID as usize + 1;
const SERVER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

const CONTROLLED_PAGE_JS: &str = include_str!("mcap_range_server/controlled_page.js");
const SERVICE_WORKER_JS: &str = include_str!("mcap_range_server/service_worker.js");

/// The role of one of the fixture's two HTTP origins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginRole {
    Page,
    Object,
}

/// Stable identifier for one isolated response script.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScenarioId(pub u64);

/// Bounded representation of a request header observed by the fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "state",
    content = "value",
    deny_unknown_fields
)]
pub enum ObservedHeader {
    Absent,
    Ascii(String),
    NonAscii,
    Oversized,
}

/// Secret-safe classification of the request `Origin` header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOrigin {
    Absent,
    PageOrigin,
    ObjectOrigin,
    Other,
    NonAscii,
    Oversized,
}

/// Request header matching performed before the scripted response starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "mode",
    content = "value",
    deny_unknown_fields
)]
pub enum HeaderRequirement {
    #[default]
    Any,
    Absent,
    Present,
    Exact(String),
}

/// Request-side controls for one scenario.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestSpec {
    #[serde(default)]
    pub range: HeaderRequirement,
    #[serde(default)]
    pub if_match: HeaderRequirement,
    #[serde(default)]
    pub head: HeadBehavior,
}

impl Default for RequestSpec {
    fn default() -> Self {
        Self {
            range: HeaderRequirement::Any,
            if_match: HeaderRequirement::Any,
            head: HeadBehavior::MirrorGet,
        }
    }
}

/// How a `HEAD` request is handled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadBehavior {
    #[default]
    MirrorGet,
    MethodNotAllowed,
    NotImplemented,
}

/// CORS response policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsSpec {
    pub allow_origin: CorsAllowOrigin,
    pub preflight: PreflightBehavior,
    pub expose: ExposeHeaders,
}

impl Default for CorsSpec {
    fn default() -> Self {
        Self {
            allow_origin: CorsAllowOrigin::PageOrigin,
            preflight: PreflightBehavior::AllowRequiredHeaders,
            expose: ExposeHeaders::RequiredRangeHeaders,
        }
    }
}

/// Value emitted in `Access-Control-Allow-Origin`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorsAllowOrigin {
    #[default]
    PageOrigin,
    Any,
    Omit,
    Mismatched,
}

/// How an actual `OPTIONS` preflight is answered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightBehavior {
    #[default]
    AllowRequiredHeaders,
    OmitIfMatch,
    OmitAllowHeaders,
    Reject,
}

/// Response headers made visible to browser code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposeHeaders {
    #[default]
    RequiredRangeHeaders,
    RequiredRangeHeadersAndContentEncoding,
    ContentRangeOnly,
    EtagOnly,
    None,
}

/// CSP `connect-src` applied to the controlled page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CspConnectPolicy {
    #[default]
    SelfAndObjectOrigin,
    SelfOnly,
    BlockAll,
}

/// Redirect behavior for the first object request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedirectSpec {
    #[default]
    None,
    SameOriginTemporary,
    CrossOriginTemporary,
    SameOriginPermanent,
    CrossOriginPermanent,
}

/// How `Content-Range` is constructed.
// Empty struct variants make Serde reject extra wire fields; unit variants silently accept them.
#[expect(
    clippy::empty_enum_variants_with_brackets,
    reason = "unit variants silently accept unknown Serde wire fields"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum ContentRangeSpec {
    DerivedFromRequest {},
    Fixed { start: u64, end: u64, total: u64 },
    Unsatisfied { total: u64 },
    Omit {},
}

impl Default for ContentRangeSpec {
    fn default() -> Self {
        Self::DerivedFromRequest {}
    }
}

/// How `Content-Length` is emitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "type",
    content = "value",
    deny_unknown_fields
)]
pub enum ContentLengthSpec {
    #[default]
    BodyLength,
    Fixed(u64),
    Omit,
}

/// Raw `ETag` shapes needed by validator tests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "type",
    content = "value",
    deny_unknown_fields
)]
pub enum EtagSpec {
    Absent,
    Strong(String),
    Weak(String),
    Raw(String),
    Duplicate(Vec<String>),
    List(Vec<String>),
}

impl Default for EtagSpec {
    fn default() -> Self {
        Self::Strong("v1".to_owned())
    }
}

/// Optional `Content-Encoding` header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentEncodingSpec {
    #[default]
    Omit,
    Identity,
    Gzip,
    Br,
}

/// One deterministic HTTP response-body chunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyChunkSpec {
    pub length: u32,
    #[serde(default)]
    pub delay_ms: u32,
    #[serde(default)]
    pub wait_for_gate: Option<u16>,
}

/// Script for the native HTTP response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum BodySpec {
    Finite { chunks: Vec<BodyChunkSpec> },
    StallAfter { chunks: Vec<BodyChunkSpec> },
    Infinite { chunk: BodyChunkSpec },
}

impl BodySpec {
    fn finite_length(&self) -> Option<u64> {
        match self {
            Self::Finite { chunks } => {
                Some(chunks.iter().map(|chunk| u64::from(chunk.length)).sum())
            }
            Self::StallAfter { .. } | Self::Infinite { .. } => None,
        }
    }

    fn chunks(&self) -> &[BodyChunkSpec] {
        match self {
            Self::Finite { chunks } | Self::StallAfter { chunks } => chunks,
            Self::Infinite { chunk } => std::slice::from_ref(chunk),
        }
    }
}

/// One byte chunk synthesized by the service worker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticChunkSpec {
    pub length: u32,
    pub fill: u8,
    #[serde(default)]
    pub delay_ms: u32,
}

/// Service-worker-only body behaviors that cannot be represented reliably by TCP chunking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum SyntheticBodySpec {
    Finite {
        chunks: Vec<SyntheticChunkSpec>,
    },
    Infinite {
        chunk_bytes: u32,
        fill: u8,
        delay_ms: u32,
    },
    ZeroProgressThenStall {
        pulls: u16,
        delay_ms: u32,
    },
}

/// Service worker behavior for same-origin object requests.
// Empty struct variants make Serde reject extra wire fields; unit variants silently accept them.
#[expect(
    clippy::empty_enum_variants_with_brackets,
    reason = "unit variants silently accept unknown Serde wire fields"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum ServiceWorkerSpec {
    Disabled {},
    ForwardNoStore {},
    Synthetic {
        status: u16,
        headers: BTreeMap<String, String>,
        body: SyntheticBodySpec,
    },
}

impl Default for ServiceWorkerSpec {
    fn default() -> Self {
        Self::Disabled {}
    }
}

/// Full, immutable response script for one test case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSpec {
    pub object_length: u32,
    #[serde(default)]
    pub object_seed: u8,
    #[serde(default)]
    pub request: RequestSpec,
    #[serde(default = "default_partial_content_status")]
    pub status: u16,
    #[serde(default)]
    pub content_range: ContentRangeSpec,
    #[serde(default)]
    pub content_length: ContentLengthSpec,
    #[serde(default)]
    pub content_encoding: ContentEncodingSpec,
    #[serde(default)]
    pub etag: EtagSpec,
    #[serde(default)]
    pub cors: CorsSpec,
    #[serde(default)]
    pub redirect: RedirectSpec,
    #[serde(default)]
    pub csp_connect: CspConnectPolicy,
    pub body: BodySpec,
    #[serde(default)]
    pub service_worker: ServiceWorkerSpec,
    /// Per-request overrides.
    ///
    /// Revision zero applies to the first non-redirect object response.
    /// Once the list is exhausted, its final revision remains in effect.
    #[serde(default)]
    pub response_revisions: Vec<ResponseRevision>,
}

/// Bounded override used to model validator, length, status and body changes in one session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseRevision {
    #[serde(default)]
    pub object_length: Option<u32>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub content_range: Option<ContentRangeSpec>,
    #[serde(default)]
    pub content_length: Option<ContentLengthSpec>,
    #[serde(default)]
    pub content_encoding: Option<ContentEncodingSpec>,
    #[serde(default)]
    pub etag: Option<EtagSpec>,
    #[serde(default)]
    pub body: Option<BodySpec>,
}

const fn default_partial_content_status() -> u16 {
    206
}

impl ScenarioSpec {
    /// A legal single-range `206` with a strong validator.
    pub fn exact_range(object_length: u32, body_length: u32) -> Self {
        Self {
            object_length,
            object_seed: 0,
            request: RequestSpec::default(),
            status: 206,
            content_range: ContentRangeSpec::DerivedFromRequest {},
            content_length: ContentLengthSpec::BodyLength,
            content_encoding: ContentEncodingSpec::Omit,
            etag: EtagSpec::default(),
            cors: CorsSpec::default(),
            redirect: RedirectSpec::None,
            csp_connect: CspConnectPolicy::SelfAndObjectOrigin,
            body: BodySpec::Finite {
                chunks: vec![BodyChunkSpec {
                    length: body_length,
                    delay_ms: 0,
                    wait_for_gate: None,
                }],
            },
            service_worker: ServiceWorkerSpec::Disabled {},
            response_revisions: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), ProtocolError> {
        self.validate_effective_response()?;
        if self.response_revisions.len() > MAX_RESPONSE_REVISIONS {
            return Err(ProtocolError::invalid("response_revisions"));
        }
        for revision in &self.response_revisions {
            self.with_revision(revision).validate_effective_response()?;
        }
        Ok(())
    }

    fn validate_effective_response(&self) -> Result<(), ProtocolError> {
        if self.object_length == 0 || self.object_length > MAX_OBJECT_BYTES {
            return Err(ProtocolError::invalid("object_length"));
        }
        if !(200..=599).contains(&self.status) {
            return Err(ProtocolError::invalid("status"));
        }
        validate_requirement(&self.request.range, "request.range")?;
        validate_requirement(&self.request.if_match, "request.if_match")?;
        validate_etag(&self.etag)?;
        validate_body(&self.body)?;
        validate_service_worker(&self.service_worker)?;
        if let ContentLengthSpec::Fixed(length) = self.content_length
            && length > MAX_BODY_BYTES
        {
            return Err(ProtocolError::invalid("content_length"));
        }
        Ok(())
    }

    fn with_revision(&self, revision: &ResponseRevision) -> Self {
        let mut effective = self.clone();
        effective.response_revisions.clear();
        if let Some(object_length) = revision.object_length {
            effective.object_length = object_length;
        }
        if let Some(status) = revision.status {
            effective.status = status;
        }
        if let Some(content_range) = &revision.content_range {
            effective.content_range = content_range.clone();
        }
        if let Some(content_length) = &revision.content_length {
            effective.content_length = content_length.clone();
        }
        if let Some(content_encoding) = revision.content_encoding {
            effective.content_encoding = content_encoding;
        }
        if let Some(etag) = &revision.etag {
            effective.etag = etag.clone();
        }
        if let Some(body) = &revision.body {
            effective.body = body.clone();
        }
        effective
    }
}

/// Browser-originated observations accepted by the bounded event sink.
// Empty struct variants make Serde reject extra wire fields; unit variants silently accept them.
#[expect(
    clippy::empty_enum_variants_with_brackets,
    reason = "unit variants silently accept unknown Serde wire fields"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum BrowserEvent {
    ApplicationBytes {
        count: u32,
        total: u64,
    },
    ReaderCancelled {
        visible_bytes: u64,
    },
    AbortController {
        visible_bytes: u64,
    },
    ArrayBufferCalled {},
    ByobRead {
        requested_bytes: u32,
        returned_bytes: u32,
        done: bool,
        rebuilt_view: bool,
    },
    Heartbeat {
        count: u64,
    },
    ServiceWorkerForwarded {},
    ServiceWorkerSynthetic {},
    ServiceWorkerZeroProgress {
        count: u16,
    },
    ServiceWorkerCancelled {},
}

/// Sanitized events recorded by the fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum FixtureEvent {
    Request {
        origin: OriginRole,
        method: String,
        range: ObservedHeader,
        if_match: ObservedHeader,
        request_origin: ObservedOrigin,
        query_present: bool,
        redirected_target: bool,
    },
    RequestRejected {
        reason: RequestRejection,
    },
    Preflight {
        behavior: PreflightBehavior,
    },
    ResponseStarted {
        status: u16,
        ordinal: u16,
    },
    BodyStarted,
    WaitingForGate {
        gate: u16,
    },
    GateReleased {
        gate: u16,
    },
    BodyFinished {
        bytes: u64,
    },
    BodyCancelled {
        bytes: u64,
        reason: BodyEndReason,
    },
    Browser {
        event: BrowserEvent,
    },
}

/// Typed request mismatch; no raw request value is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestRejection {
    Range,
    IfMatch,
}

/// Why an unfinished response stream ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyEndReason {
    ClientDisconnected,
    ScenarioRemoved,
    ServerShutdown,
}

/// Bounded snapshot returned by the control protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLogSnapshot {
    pub events: Vec<FixtureEvent>,
    pub retained_bytes: usize,
    pub overflowed: bool,
}

/// Public endpoints for one registered scenario.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioDescriptor {
    pub id: ScenarioId,
    pub page_url: String,
    pub same_origin_object_url: String,
    pub cross_origin_object_url: String,
}

/// Non-secret discovery payload used by Wasm browser tests.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureBootstrap {
    pub protocol: String,
    pub page_origin: String,
    pub object_origin: String,
    pub control_root: String,
}

#[derive(Clone, Debug, Serialize)]
struct BrowserScenarioConfig {
    service_worker: ServiceWorkerSpec,
}

#[derive(Debug, Default)]
struct EventLog {
    events: VecDeque<FixtureEvent>,
    retained_bytes: usize,
    overflowed: bool,
}

struct ScenarioState {
    spec: ScenarioSpec,
    events: Mutex<EventLog>,
    event_notify: Notify,
    gates: [Arc<Notify>; GATE_SLOTS],
    cancel_tx: watch::Sender<bool>,
    next_response: AtomicUsize,
    active_bodies: AtomicUsize,
}

impl ScenarioState {
    fn new(spec: ScenarioSpec) -> Self {
        let (cancel_tx, _) = watch::channel(false);
        Self {
            spec,
            events: Mutex::new(EventLog::default()),
            event_notify: Notify::new(),
            gates: std::array::from_fn(|_| Arc::new(Notify::new())),
            cancel_tx,
            next_response: AtomicUsize::new(0),
            active_bodies: AtomicUsize::new(0),
        }
    }

    fn next_effective_spec(&self) -> (u16, ScenarioSpec) {
        let ordinal = self.next_response.fetch_add(1, Ordering::Relaxed);
        let effective = if self.spec.response_revisions.is_empty() {
            let mut effective = self.spec.clone();
            effective.response_revisions.clear();
            effective
        } else {
            let index = ordinal.min(self.spec.response_revisions.len() - 1);
            self.spec
                .with_revision(&self.spec.response_revisions[index])
        };
        (u16::try_from(ordinal).unwrap_or(u16::MAX), effective)
    }

    fn record(&self, event: FixtureEvent) -> Result<(), ()> {
        let event_bytes = serde_json::to_vec(&event)
            .expect("fixture event serialization is infallible")
            .len();
        let mut events = lock(&self.events);
        if events.events.len() == MAX_EVENTS_PER_SCENARIO
            || events.retained_bytes.saturating_add(event_bytes) > MAX_EVENT_BYTES_PER_SCENARIO
        {
            events.overflowed = true;
            self.event_notify.notify_waiters();
            return Err(());
        }
        events.retained_bytes += event_bytes;
        events.events.push_back(event);
        drop(events);
        self.event_notify.notify_waiters();
        Ok(())
    }

    fn snapshot(&self) -> EventLogSnapshot {
        let events = lock(&self.events);
        EventLogSnapshot {
            events: events.events.iter().cloned().collect(),
            retained_bytes: events.retained_bytes,
            overflowed: events.overflowed,
        }
    }
}

struct SharedState {
    nonce: String,
    page_addr: SocketAddr,
    object_addr: SocketAddr,
    scenarios: Mutex<BTreeMap<ScenarioId, Arc<ScenarioState>>>,
    next_scenario: AtomicU64,
    server_shutdown: watch::Sender<bool>,
    active_bodies: AtomicUsize,
}

impl SharedState {
    fn origin(&self, role: OriginRole) -> String {
        let addr = match role {
            OriginRole::Page => self.page_addr,
            OriginRole::Object => self.object_addr,
        };
        format!("http://{addr}")
    }

    fn scenario(&self, id: ScenarioId) -> Result<Arc<ScenarioState>, ProtocolError> {
        lock(&self.scenarios)
            .get(&id)
            .cloned()
            .ok_or_else(|| ProtocolError::not_found("scenario"))
    }

    fn descriptor(&self, id: ScenarioId) -> ScenarioDescriptor {
        let root = format!("{ROUTE_PREFIX}/{}", self.nonce);
        ScenarioDescriptor {
            id,
            page_url: format!("{}{root}/page/{}", self.origin(OriginRole::Page), id.0),
            same_origin_object_url: format!(
                "{}{root}/object/{}",
                self.origin(OriginRole::Page),
                id.0
            ),
            cross_origin_object_url: format!(
                "{}{root}/object/{}",
                self.origin(OriginRole::Object),
                id.0
            ),
        }
    }
}

#[derive(Clone)]
struct AppState {
    shared: Arc<SharedState>,
    role: OriginRole,
}

/// Running pair of loopback-only fixture origins.
pub struct McapRangeTestServer {
    shared: Arc<SharedState>,
    page_shutdown: Option<oneshot::Sender<()>>,
    object_shutdown: Option<oneshot::Sender<()>>,
    page_join: Option<tokio::task::JoinHandle<()>>,
    object_join: Option<tokio::task::JoinHandle<()>>,
}

impl McapRangeTestServer {
    /// Bind two OS-assigned loopback ports and start both origins.
    pub async fn spawn() -> Result<Self, std::io::Error> {
        let page_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let object_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let page_addr = page_listener.local_addr()?;
        let object_addr = object_listener.local_addr()?;

        let mut nonce_bytes = [0_u8; 16];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = nonce_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let (server_shutdown, _) = watch::channel(false);
        let shared = Arc::new(SharedState {
            nonce,
            page_addr,
            object_addr,
            scenarios: Mutex::new(BTreeMap::new()),
            next_scenario: AtomicU64::new(1),
            server_shutdown,
            active_bodies: AtomicUsize::new(0),
        });

        let (page_shutdown_tx, page_shutdown_rx) = oneshot::channel();
        let (object_shutdown_tx, object_shutdown_rx) = oneshot::channel();
        let page_app = router(AppState {
            shared: shared.clone(),
            role: OriginRole::Page,
        });
        let object_app = router(AppState {
            shared: shared.clone(),
            role: OriginRole::Object,
        });
        let page_join = tokio::spawn(async move {
            _ = axum::serve(page_listener, page_app)
                .with_graceful_shutdown(async {
                    _ = page_shutdown_rx.await;
                })
                .await;
        });
        let object_join = tokio::spawn(async move {
            _ = axum::serve(object_listener, object_app)
                .with_graceful_shutdown(async {
                    _ = object_shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            shared,
            page_shutdown: Some(page_shutdown_tx),
            object_shutdown: Some(object_shutdown_tx),
            page_join: Some(page_join),
            object_join: Some(object_join),
        })
    }

    pub fn page_addr(&self) -> SocketAddr {
        self.shared.page_addr
    }

    pub fn object_addr(&self) -> SocketAddr {
        self.shared.object_addr
    }

    pub fn page_origin(&self) -> String {
        self.shared.origin(OriginRole::Page)
    }

    pub fn object_origin(&self) -> String {
        self.shared.origin(OriginRole::Object)
    }

    pub fn active_response_bodies(&self) -> usize {
        self.shared.active_bodies.load(Ordering::Relaxed)
    }

    pub fn bootstrap_url(&self) -> String {
        format!("{}{ROUTE_PREFIX}/bootstrap", self.page_origin())
    }

    /// Register a validated scenario without crossing HTTP.
    pub fn register_scenario(
        &self,
        spec: ScenarioSpec,
    ) -> Result<ScenarioDescriptor, ProtocolError> {
        register_scenario(&self.shared, spec)
    }

    pub fn snapshot(&self, id: ScenarioId) -> Result<EventLogSnapshot, ProtocolError> {
        Ok(self.shared.scenario(id)?.snapshot())
    }

    /// Wait until an event matching `predicate` is present, without consuming it.
    pub async fn wait_for_event(
        &self,
        id: ScenarioId,
        timeout: Duration,
        predicate: impl Fn(&FixtureEvent) -> bool,
    ) -> Result<EventLogSnapshot, WaitForEventError> {
        let scenario = self
            .shared
            .scenario(id)
            .map_err(WaitForEventError::Protocol)?;
        let wait = async {
            loop {
                let notified = scenario.event_notify.notified();
                let snapshot = scenario.snapshot();
                if snapshot.events.iter().any(&predicate) {
                    return snapshot;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_elapsed| WaitForEventError::Timeout(scenario.snapshot()))
    }

    /// Cancel all response bodies, close both listeners and await both tasks.
    pub async fn shutdown(mut self) {
        self.signal_shutdown();
        let page_join = self.page_join.take();
        let object_join = self.object_join.take();
        if let Some(join) = page_join {
            await_server_task(join).await;
        }
        if let Some(join) = object_join {
            await_server_task(join).await;
        }
    }

    fn signal_shutdown(&mut self) {
        _ = self.shared.server_shutdown.send(true);
        for scenario in lock(&self.shared.scenarios).values() {
            _ = scenario.cancel_tx.send(true);
        }
        if let Some(shutdown) = self.page_shutdown.take() {
            _ = shutdown.send(());
        }
        if let Some(shutdown) = self.object_shutdown.take() {
            _ = shutdown.send(());
        }
    }
}

impl Drop for McapRangeTestServer {
    fn drop(&mut self) {
        self.signal_shutdown();
        if let Some(join) = self.page_join.take() {
            join.abort();
        }
        if let Some(join) = self.object_join.take() {
            join.abort();
        }
    }
}

async fn await_server_task(mut join: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(SERVER_SHUTDOWN_TIMEOUT, &mut join)
        .await
        .is_err()
    {
        join.abort();
        _ = join.await;
    }
}

/// Failure while waiting for a recorded event.
#[derive(Debug)]
pub enum WaitForEventError {
    Protocol(ProtocolError),
    Timeout(EventLogSnapshot),
}

impl std::fmt::Display for WaitForEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(err) => err.fmt(f),
            Self::Timeout(snapshot) => write!(
                f,
                "timed out waiting for fixture event; {} event(s) retained",
                snapshot.events.len()
            ),
        }
    }
}

impl std::error::Error for WaitForEventError {}

/// Sanitized control-protocol error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolError {
    status: StatusCode,
    code: &'static str,
    field: &'static str,
}

impl ProtocolError {
    fn invalid(field: &'static str) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_fixture_spec",
            field,
        }
    }

    fn not_found(field: &'static str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "fixture_not_found",
            field,
        }
    }

    fn capacity(field: &'static str) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "fixture_capacity_exceeded",
            field,
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.field)
    }
}

impl std::error::Error for ProtocolError {}

impl IntoResponse for ProtocolError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
            field: &'static str,
        }
        (
            self.status,
            Json(ErrorBody {
                code: self.code,
                field: self.field,
            }),
        )
            .into_response()
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route(&format!("{ROUTE_PREFIX}/bootstrap"), get(bootstrap))
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/control/scenarios"),
            post(register_scenario_http).options(control_preflight),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/control/scenarios/{{id}}"),
            get(event_snapshot)
                .delete(remove_scenario)
                .options(control_preflight),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/control/scenarios/{{id}}/browser-config"),
            get(browser_config).options(control_preflight),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/control/scenarios/{{id}}/browser-events"),
            post(browser_event).options(control_preflight),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/control/scenarios/{{id}}/gates/{{gate}}"),
            post(release_gate).options(control_preflight),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/page/{{id}}"),
            get(controlled_page),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/controlled-page.js"),
            get(controlled_page_js),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/service-worker.js"),
            get(service_worker_js),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/object/{{id}}"),
            any(object_response),
        )
        .route(
            &format!("{ROUTE_PREFIX}/{{nonce}}/object/{{id}}/redirected"),
            any(object_response_redirected),
        )
        .layer(DefaultBodyLimit::max(MAX_CONTROL_BODY_BYTES))
        .with_state(state)
}

async fn bootstrap(State(state): State<AppState>) -> Response {
    let mut response = Json(FixtureBootstrap {
        protocol: PROTOCOL_VERSION.to_owned(),
        page_origin: state.shared.origin(OriginRole::Page),
        object_origin: state.shared.origin(OriginRole::Object),
        control_root: format!("{ROUTE_PREFIX}/{}", state.shared.nonce),
    })
    .into_response();
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn register_scenario_http(
    State(state): State<AppState>,
    Path(nonce): Path<String>,
    body: Bytes,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let spec: ScenarioSpec =
        serde_json::from_slice(&body).map_err(|_err| ProtocolError::invalid("json"))?;
    Ok(control_cors(
        Json(register_scenario(&state.shared, spec)?).into_response(),
    ))
}

fn register_scenario(
    shared: &Arc<SharedState>,
    spec: ScenarioSpec,
) -> Result<ScenarioDescriptor, ProtocolError> {
    spec.validate()?;
    let encoded = serde_json::to_vec(&spec).map_err(|_err| ProtocolError::invalid("json"))?;
    if encoded.len() > MAX_CONTROL_BODY_BYTES {
        return Err(ProtocolError::invalid("json_size"));
    }
    let mut scenarios = lock(&shared.scenarios);
    if scenarios.len() == MAX_SCENARIOS {
        return Err(ProtocolError::capacity("scenarios"));
    }
    let raw_id = shared
        .next_scenario
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .map_err(|_previous| ProtocolError::capacity("scenario_ids"))?;
    let id = ScenarioId(raw_id);
    scenarios.insert(id, Arc::new(ScenarioState::new(spec)));
    drop(scenarios);
    Ok(shared.descriptor(id))
}

async fn event_snapshot(
    State(state): State<AppState>,
    Path((nonce, id)): Path<(String, u64)>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    Ok(control_cors(
        Json(state.shared.scenario(ScenarioId(id))?.snapshot()).into_response(),
    ))
}

async fn remove_scenario(
    State(state): State<AppState>,
    Path((nonce, id)): Path<(String, u64)>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let scenario = lock(&state.shared.scenarios)
        .remove(&ScenarioId(id))
        .ok_or_else(|| ProtocolError::not_found("scenario"))?;
    _ = scenario.cancel_tx.send(true);
    Ok(control_cors(StatusCode::NO_CONTENT.into_response()))
}

async fn browser_config(
    State(state): State<AppState>,
    Path((nonce, id)): Path<(String, u64)>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let scenario = state.shared.scenario(ScenarioId(id))?;
    let mut response = Json(BrowserScenarioConfig {
        service_worker: scenario.spec.service_worker.clone(),
    })
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(control_cors(response))
}

async fn browser_event(
    State(state): State<AppState>,
    Path((nonce, id)): Path<(String, u64)>,
    body: Bytes,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    if body.len() > MAX_BROWSER_EVENT_BYTES {
        return Err(ProtocolError::invalid("browser_event_size"));
    }
    let event = serde_json::from_slice::<BrowserEvent>(&body)
        .map_err(|_err| ProtocolError::invalid("browser_event"))?;
    state
        .shared
        .scenario(ScenarioId(id))?
        .record(FixtureEvent::Browser { event })
        .map_err(|()| ProtocolError::capacity("event_log"))?;
    Ok(control_cors(StatusCode::NO_CONTENT.into_response()))
}

async fn release_gate(
    State(state): State<AppState>,
    Path((nonce, id, gate)): Path<(String, u64, u16)>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let scenario = state.shared.scenario(ScenarioId(id))?;
    let notify = scenario
        .gates
        .get(usize::from(gate))
        .ok_or_else(|| ProtocolError::not_found("gate"))?;
    notify.notify_one();
    _ = scenario.record(FixtureEvent::GateReleased { gate });
    Ok(control_cors(StatusCode::NO_CONTENT.into_response()))
}

async fn control_preflight() -> Response {
    control_cors(StatusCode::NO_CONTENT.into_response())
}

fn control_cors(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
    );
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn controlled_page(
    State(state): State<AppState>,
    Path((nonce, id)): Path<(String, u64)>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let scenario = state.shared.scenario(ScenarioId(id))?;
    let descriptor = state.shared.descriptor(ScenarioId(id));
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>MCAP Range fixture</title></head><body data-cross-origin-object-url=\"{}\"><script src=\"{ROUTE_PREFIX}/{nonce}/controlled-page.js\"></script></body></html>",
        descriptor.cross_origin_object_url
    );
    let mut response = Html(html).into_response();
    response.headers_mut().insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_str(&csp_value(&state.shared, scenario.spec.csp_connect))
            .expect("fixture CSP is valid"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn controlled_page_js(
    State(state): State<AppState>,
    Path(nonce): Path<String>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    Ok(javascript_response(CONTROLLED_PAGE_JS))
}

async fn service_worker_js(
    State(state): State<AppState>,
    Path(nonce): Path<String>,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let mut response = javascript_response(SERVICE_WORKER_JS);
    response.headers_mut().insert(
        HeaderName::from_static("service-worker-allowed"),
        HeaderValue::from_str(&format!("{ROUTE_PREFIX}/{nonce}/")).expect("valid worker scope"),
    );
    Ok(response)
}

fn javascript_response(source: &'static str) -> Response {
    let mut response = source.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn object_response(
    state: State<AppState>,
    path: Path<(String, u64)>,
    uri: OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ProtocolError> {
    object_response_inner(
        &state.0,
        path.0,
        RequestMetadata {
            query_present: uri.0.query().is_some(),
            redirected_target: false,
        },
        &method,
        &headers,
    )
}

async fn object_response_redirected(
    state: State<AppState>,
    path: Path<(String, u64)>,
    uri: OriginalUri,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, ProtocolError> {
    object_response_inner(
        &state.0,
        path.0,
        RequestMetadata {
            query_present: uri.0.query().is_some(),
            redirected_target: true,
        },
        &method,
        &headers,
    )
}

#[derive(Clone, Copy)]
struct RequestMetadata {
    query_present: bool,
    redirected_target: bool,
}

fn object_response_inner(
    state: &AppState,
    (nonce, raw_id): (String, u64),
    metadata: RequestMetadata,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response, ProtocolError> {
    require_nonce(&state.shared, &nonce)?;
    let id = ScenarioId(raw_id);
    let scenario = state.shared.scenario(id)?;
    scenario
        .record(FixtureEvent::Request {
            origin: state.role,
            method: method.as_str().to_owned(),
            range: observed_header(headers, axum::http::header::RANGE),
            if_match: observed_header(headers, axum::http::header::IF_MATCH),
            request_origin: observed_origin(&state.shared, headers),
            // Only presence is retained; query bytes are never retained or formatted.
            query_present: metadata.query_present,
            redirected_target: metadata.redirected_target,
        })
        .map_err(|()| ProtocolError::capacity("event_log"))?;

    if *method == Method::OPTIONS {
        return Ok(preflight_response(&state.shared, &scenario));
    }
    if *method == Method::HEAD {
        match scenario.spec.request.head {
            HeadBehavior::MethodNotAllowed => {
                return Ok(cors_response(
                    &state.shared,
                    &scenario.spec.cors,
                    StatusCode::METHOD_NOT_ALLOWED.into_response(),
                ));
            }
            HeadBehavior::NotImplemented => {
                return Ok(cors_response(
                    &state.shared,
                    &scenario.spec.cors,
                    StatusCode::NOT_IMPLEMENTED.into_response(),
                ));
            }
            HeadBehavior::MirrorGet => {}
        }
    } else if *method != Method::GET {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }

    if !matches_requirement(
        &scenario.spec.request.range,
        headers.get(axum::http::header::RANGE),
    ) {
        _ = scenario.record(FixtureEvent::RequestRejected {
            reason: RequestRejection::Range,
        });
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    if !matches_requirement(
        &scenario.spec.request.if_match,
        headers.get(axum::http::header::IF_MATCH),
    ) {
        _ = scenario.record(FixtureEvent::RequestRejected {
            reason: RequestRejection::IfMatch,
        });
        return Ok(StatusCode::PRECONDITION_FAILED.into_response());
    }

    if !metadata.redirected_target && scenario.spec.redirect != RedirectSpec::None {
        return Ok(cors_response(
            &state.shared,
            &scenario.spec.cors,
            redirect_response(&state.shared, state.role, id, scenario.spec.redirect),
        ));
    }

    let body_permit = (*method != Method::HEAD)
        .then(|| BodyPermit::try_acquire(state.shared.clone(), scenario.clone()))
        .transpose()?;
    let (response_ordinal, effective_spec) = scenario.next_effective_spec();
    let status = StatusCode::from_u16(effective_spec.status)
        .map_err(|_err| ProtocolError::invalid("status"))?;
    let mut response = if *method == Method::HEAD {
        status.into_response()
    } else {
        let body_offset = response_body_offset(&effective_spec.content_range, headers);
        let body = response_body(
            state.shared.clone(),
            scenario.clone(),
            body_permit.expect("GET response has a body permit"),
            effective_spec.body.clone(),
            effective_spec.object_seed,
            body_offset,
        );
        Response::builder()
            .status(status)
            .body(body)
            .expect("fixture response builder is valid")
    };
    apply_response_headers(&effective_spec, headers, response.headers_mut())?;
    response = cors_response(&state.shared, &scenario.spec.cors, response);
    _ = scenario.record(FixtureEvent::ResponseStarted {
        status: status.as_u16(),
        ordinal: response_ordinal,
    });
    Ok(response)
}

fn preflight_response(shared: &SharedState, scenario: &ScenarioState) -> Response {
    _ = scenario.record(FixtureEvent::Preflight {
        behavior: scenario.spec.cors.preflight,
    });
    let status = match scenario.spec.cors.preflight {
        PreflightBehavior::Reject => StatusCode::FORBIDDEN,
        _ => StatusCode::NO_CONTENT,
    };
    let mut response = status.into_response();
    match scenario.spec.cors.preflight {
        PreflightBehavior::AllowRequiredHeaders => {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("range, if-match"),
            );
        }
        PreflightBehavior::OmitIfMatch => {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("range"),
            );
        }
        PreflightBehavior::OmitAllowHeaders | PreflightBehavior::Reject => {}
    }
    response.headers_mut().insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    cors_response(shared, &scenario.spec.cors, response)
}

fn redirect_response(
    shared: &SharedState,
    request_origin: OriginRole,
    id: ScenarioId,
    redirect: RedirectSpec,
) -> Response {
    let (role, status) = match redirect {
        RedirectSpec::SameOriginTemporary => (request_origin, StatusCode::TEMPORARY_REDIRECT),
        RedirectSpec::CrossOriginTemporary => (
            opposite_origin(request_origin),
            StatusCode::TEMPORARY_REDIRECT,
        ),
        RedirectSpec::SameOriginPermanent => (request_origin, StatusCode::PERMANENT_REDIRECT),
        RedirectSpec::CrossOriginPermanent => (
            opposite_origin(request_origin),
            StatusCode::PERMANENT_REDIRECT,
        ),
        RedirectSpec::None => unreachable!("redirect response requires redirect mode"),
    };
    let location = format!(
        "{}{ROUTE_PREFIX}/{}/object/{}/redirected",
        shared.origin(role),
        shared.nonce,
        id.0
    );
    let mut response = status.into_response();
    response.headers_mut().insert(
        LOCATION,
        HeaderValue::from_str(&location).expect("fixture redirect URL is valid"),
    );
    response
}

const fn opposite_origin(origin: OriginRole) -> OriginRole {
    match origin {
        OriginRole::Page => OriginRole::Object,
        OriginRole::Object => OriginRole::Page,
    }
}

fn apply_response_headers(
    spec: &ScenarioSpec,
    request_headers: &HeaderMap,
    headers: &mut HeaderMap,
) -> Result<(), ProtocolError> {
    match spec.content_range {
        ContentRangeSpec::DerivedFromRequest {} => {
            let range = request_headers
                .get(axum::http::header::RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_single_range)
                .ok_or_else(|| ProtocolError::invalid("range_header"))?;
            headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!(
                    "bytes {}-{}/{}",
                    range.0, range.1, spec.object_length
                ))
                .expect("derived Content-Range is valid"),
            );
        }
        ContentRangeSpec::Fixed { start, end, total } => {
            headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                    .expect("fixed Content-Range is valid"),
            );
        }
        ContentRangeSpec::Unsatisfied { total } => {
            headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{total}"))
                    .expect("unsatisfied Content-Range is valid"),
            );
        }
        ContentRangeSpec::Omit {} => {}
    }
    match spec.content_length {
        ContentLengthSpec::BodyLength => {
            if let Some(length) = spec.body.finite_length() {
                headers.insert(
                    CONTENT_LENGTH,
                    HeaderValue::from_str(&length.to_string()).expect("body length is valid"),
                );
            }
        }
        ContentLengthSpec::Fixed(length) => {
            headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&length.to_string()).expect("fixed body length is valid"),
            );
        }
        ContentLengthSpec::Omit => {}
    }
    match spec.content_encoding {
        ContentEncodingSpec::Omit => {}
        ContentEncodingSpec::Identity => {
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
        }
        ContentEncodingSpec::Gzip => {
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        }
        ContentEncodingSpec::Br => {
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static("br"));
        }
    }
    append_etag_headers(headers, &spec.etag)?;
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(())
}

fn response_body_offset(content_range: &ContentRangeSpec, headers: &HeaderMap) -> u64 {
    match content_range {
        ContentRangeSpec::DerivedFromRequest {} => headers
            .get(axum::http::header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_single_range)
            .map_or(0, |range| range.0),
        ContentRangeSpec::Fixed { start, .. } => *start,
        ContentRangeSpec::Unsatisfied { .. } | ContentRangeSpec::Omit {} => 0,
    }
}

fn response_body(
    shared: Arc<SharedState>,
    scenario: Arc<ScenarioState>,
    permit: BodyPermit,
    body_spec: BodySpec,
    seed: u8,
    body_offset: u64,
) -> Body {
    let mut server_shutdown = shared.server_shutdown.subscribe();
    let mut scenario_cancel = scenario.cancel_tx.subscribe();
    let stream = async_stream::stream! {
        let mut guard = BodyGuard::new(shared.clone(), scenario.clone(), permit);
        _ = scenario.record(FixtureEvent::BodyStarted);
        match body_spec {
            BodySpec::Finite { chunks } => {
                for chunk in chunks {
                    if !prepare_chunk(&scenario, &chunk, &mut server_shutdown, &mut scenario_cancel).await {
                        return;
                    }
                    let bytes = pattern_bytes(
                        seed,
                        body_offset.wrapping_add(guard.sent),
                        chunk.length,
                    );
                    guard.sent = guard.sent.wrapping_add(u64::from(chunk.length));
                    yield Ok::<Bytes, Infallible>(bytes);
                }
                guard.finish();
            }
            BodySpec::StallAfter { chunks } => {
                for chunk in chunks {
                    if !prepare_chunk(&scenario, &chunk, &mut server_shutdown, &mut scenario_cancel).await {
                        return;
                    }
                    let bytes = pattern_bytes(
                        seed,
                        body_offset.wrapping_add(guard.sent),
                        chunk.length,
                    );
                    guard.sent = guard.sent.wrapping_add(u64::from(chunk.length));
                    yield Ok::<Bytes, Infallible>(bytes);
                }
                wait_for_cancel(&mut server_shutdown, &mut scenario_cancel).await;
            }
            BodySpec::Infinite { chunk } => loop {
                if !prepare_chunk(&scenario, &chunk, &mut server_shutdown, &mut scenario_cancel).await {
                    return;
                }
                let bytes = pattern_bytes(
                    seed,
                    body_offset.wrapping_add(guard.sent),
                    chunk.length,
                );
                guard.sent = guard.sent.wrapping_add(u64::from(chunk.length));
                yield Ok::<Bytes, Infallible>(bytes);
            },
        }
    };
    Body::from_stream(stream)
}

async fn prepare_chunk(
    scenario: &ScenarioState,
    chunk: &BodyChunkSpec,
    server_shutdown: &mut watch::Receiver<bool>,
    scenario_cancel: &mut watch::Receiver<bool>,
) -> bool {
    if let Some(gate) = chunk.wait_for_gate {
        let Some(notify) = scenario.gates.get(usize::from(gate)) else {
            return false;
        };
        _ = scenario.record(FixtureEvent::WaitingForGate { gate });
        tokio::select! {
            () = notify.notified() => {}
            _ = server_shutdown.changed() => return false,
            _ = scenario_cancel.changed() => return false,
        }
    }
    if chunk.delay_ms > 0 {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(u64::from(chunk.delay_ms))) => {}
            _ = server_shutdown.changed() => return false,
            _ = scenario_cancel.changed() => return false,
        }
    }
    true
}

async fn wait_for_cancel(
    server_shutdown: &mut watch::Receiver<bool>,
    scenario_cancel: &mut watch::Receiver<bool>,
) {
    tokio::select! {
        _ = server_shutdown.changed() => {}
        _ = scenario_cancel.changed() => {}
    }
}

struct BodyGuard {
    shared: Arc<SharedState>,
    scenario: Arc<ScenarioState>,
    _permit: BodyPermit,
    sent: u64,
    finished: bool,
}

impl BodyGuard {
    fn new(shared: Arc<SharedState>, scenario: Arc<ScenarioState>, permit: BodyPermit) -> Self {
        Self {
            shared,
            scenario,
            _permit: permit,
            sent: 0,
            finished: false,
        }
    }

    fn finish(&mut self) {
        self.finished = true;
        _ = self
            .scenario
            .record(FixtureEvent::BodyFinished { bytes: self.sent });
    }
}

impl Drop for BodyGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let reason = if *self.shared.server_shutdown.borrow() {
            BodyEndReason::ServerShutdown
        } else if *self.scenario.cancel_tx.borrow() {
            BodyEndReason::ScenarioRemoved
        } else {
            BodyEndReason::ClientDisconnected
        };
        _ = self.scenario.record(FixtureEvent::BodyCancelled {
            bytes: self.sent,
            reason,
        });
    }
}

struct BodyPermit {
    shared: Arc<SharedState>,
    scenario: Arc<ScenarioState>,
}

impl BodyPermit {
    fn try_acquire(
        shared: Arc<SharedState>,
        scenario: Arc<ScenarioState>,
    ) -> Result<Self, ProtocolError> {
        shared
            .active_bodies
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < MAX_ACTIVE_BODIES).then_some(active + 1)
            })
            .map_err(|_active| ProtocolError::capacity("active_response_bodies"))?;
        if scenario
            .active_bodies
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < MAX_ACTIVE_BODIES_PER_SCENARIO).then_some(active + 1)
            })
            .is_err()
        {
            shared.active_bodies.fetch_sub(1, Ordering::Relaxed);
            return Err(ProtocolError::capacity("scenario_response_bodies"));
        }
        Ok(Self { shared, scenario })
    }
}

impl Drop for BodyPermit {
    fn drop(&mut self) {
        self.scenario.active_bodies.fetch_sub(1, Ordering::Relaxed);
        self.shared.active_bodies.fetch_sub(1, Ordering::Relaxed);
    }
}

fn pattern_bytes(seed: u8, offset: u64, length: u32) -> Bytes {
    let bytes = (0..length)
        .map(|index| seed.wrapping_add(offset.wrapping_add(u64::from(index)) as u8))
        .collect::<Vec<_>>();
    Bytes::from(bytes)
}

fn cors_response(shared: &SharedState, cors: &CorsSpec, mut response: Response) -> Response {
    match cors.allow_origin {
        CorsAllowOrigin::PageOrigin => {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_str(&shared.origin(OriginRole::Page))
                    .expect("fixture page origin is valid"),
            );
        }
        CorsAllowOrigin::Any => {
            response
                .headers_mut()
                .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        }
        CorsAllowOrigin::Omit => {}
        CorsAllowOrigin::Mismatched => {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("https://mismatched.invalid"),
            );
        }
    }
    let exposed = match cors.expose {
        ExposeHeaders::RequiredRangeHeaders => Some("content-range, content-length, etag"),
        ExposeHeaders::RequiredRangeHeadersAndContentEncoding => {
            Some("content-range, content-length, content-encoding, etag")
        }
        ExposeHeaders::ContentRangeOnly => Some("content-range"),
        ExposeHeaders::EtagOnly => Some("etag"),
        ExposeHeaders::None => None,
    };
    if let Some(exposed) = exposed {
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(exposed),
        );
    }
    response
}

fn csp_value(shared: &SharedState, policy: CspConnectPolicy) -> String {
    let connect_src = match policy {
        CspConnectPolicy::SelfAndObjectOrigin => {
            format!("'self' {}", shared.origin(OriginRole::Object))
        }
        CspConnectPolicy::SelfOnly => "'self'".to_owned(),
        CspConnectPolicy::BlockAll => "'none'".to_owned(),
    };
    format!(
        "default-src 'none'; script-src 'self'; worker-src 'self'; connect-src {connect_src}; base-uri 'none'; object-src 'none'"
    )
}

fn append_etag_headers(headers: &mut HeaderMap, etag: &EtagSpec) -> Result<(), ProtocolError> {
    let mut append = |value: &str| -> Result<(), ProtocolError> {
        let value = HeaderValue::from_str(value).map_err(|_err| ProtocolError::invalid("etag"))?;
        headers.append(ETAG, value);
        Ok(())
    };
    match etag {
        EtagSpec::Absent => {}
        EtagSpec::Strong(value) => append(&format!("\"{value}\""))?,
        EtagSpec::Weak(value) => append(&format!("W/\"{value}\""))?,
        EtagSpec::Raw(value) => append(value)?,
        EtagSpec::Duplicate(values) => {
            for value in values {
                append(value)?;
            }
        }
        EtagSpec::List(values) => append(&checked_etag_list(values)?)?,
    }
    Ok(())
}

fn observed_header(headers: &HeaderMap, name: HeaderName) -> ObservedHeader {
    let Some(value) = headers.get(name) else {
        return ObservedHeader::Absent;
    };
    if value.as_bytes().len() > MAX_HEADER_VALUE_BYTES {
        return ObservedHeader::Oversized;
    }
    match value.to_str() {
        Ok(value) => ObservedHeader::Ascii(value.to_owned()),
        Err(_) => ObservedHeader::NonAscii,
    }
}

fn observed_origin(shared: &SharedState, headers: &HeaderMap) -> ObservedOrigin {
    let Some(value) = headers.get(axum::http::header::ORIGIN) else {
        return ObservedOrigin::Absent;
    };
    if value.as_bytes().len() > MAX_HEADER_VALUE_BYTES {
        return ObservedOrigin::Oversized;
    }
    let Ok(value) = value.to_str() else {
        return ObservedOrigin::NonAscii;
    };
    if value == shared.origin(OriginRole::Page) {
        ObservedOrigin::PageOrigin
    } else if value == shared.origin(OriginRole::Object) {
        ObservedOrigin::ObjectOrigin
    } else {
        ObservedOrigin::Other
    }
}

fn matches_requirement(requirement: &HeaderRequirement, actual: Option<&HeaderValue>) -> bool {
    match requirement {
        HeaderRequirement::Any => true,
        HeaderRequirement::Absent => actual.is_none(),
        HeaderRequirement::Present => actual.is_some(),
        HeaderRequirement::Exact(expected) => {
            actual.is_some_and(|actual| actual.as_bytes() == expected.as_bytes())
        }
    }
}

fn parse_single_range(value: &str) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    (start <= end).then_some((start, end))
}

fn require_nonce(shared: &SharedState, nonce: &str) -> Result<(), ProtocolError> {
    if constant_time_eq(shared.nonce.as_bytes(), nonce.as_bytes()) {
        Ok(())
    } else {
        Err(ProtocolError::not_found("fixture"))
    }
}

fn constant_time_eq(expected: &[u8], actual: &[u8]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .iter()
        .zip(actual)
        .fold(0_u8, |difference, (lhs, rhs)| difference | (lhs ^ rhs))
        == 0
}

fn validate_requirement(
    requirement: &HeaderRequirement,
    field: &'static str,
) -> Result<(), ProtocolError> {
    if let HeaderRequirement::Exact(value) = requirement {
        validate_header_text(value, field)?;
    }
    Ok(())
}

fn validate_etag(etag: &EtagSpec) -> Result<(), ProtocolError> {
    let values: Vec<&String> = match etag {
        EtagSpec::Absent => Vec::new(),
        EtagSpec::Strong(value) | EtagSpec::Weak(value) | EtagSpec::Raw(value) => vec![value],
        EtagSpec::Duplicate(values) | EtagSpec::List(values) => values.iter().collect(),
    };
    if values.len() > 8 {
        return Err(ProtocolError::invalid("etag_count"));
    }
    if matches!(etag, EtagSpec::Duplicate(_) | EtagSpec::List(_)) && values.len() < 2 {
        return Err(ProtocolError::invalid("etag_count"));
    }
    for value in values {
        validate_header_text(value, "etag")?;
        if matches!(etag, EtagSpec::Strong(_) | EtagSpec::Weak(_))
            && (value.contains('"') || value.chars().any(char::is_control))
        {
            return Err(ProtocolError::invalid("etag"));
        }
    }
    if let EtagSpec::List(values) = etag {
        _ = checked_etag_list(values)?;
    }
    Ok(())
}

fn checked_etag_list(values: &[String]) -> Result<String, ProtocolError> {
    let separator_bytes = values
        .len()
        .saturating_sub(1)
        .checked_mul(2)
        .ok_or_else(|| ProtocolError::invalid("etag_list_bytes"))?;
    let total_bytes = values.iter().try_fold(separator_bytes, |total, value| {
        total.checked_add(value.len())
    });
    let total_bytes = total_bytes.ok_or_else(|| ProtocolError::invalid("etag_list_bytes"))?;
    if total_bytes > MAX_HEADER_VALUE_BYTES {
        return Err(ProtocolError::invalid("etag_list_bytes"));
    }
    let mut joined = String::with_capacity(total_bytes);
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            joined.push_str(", ");
        }
        joined.push_str(value);
    }
    Ok(joined)
}

fn validate_header_text(value: &str, field: &'static str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || value.len() > MAX_HEADER_VALUE_BYTES
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(ProtocolError::invalid(field));
    }
    Ok(())
}

fn validate_body(body: &BodySpec) -> Result<(), ProtocolError> {
    let chunks = body.chunks();
    if chunks.len() > MAX_BODY_CHUNKS
        || matches!(body, BodySpec::Infinite { .. }) && chunks.is_empty()
    {
        return Err(ProtocolError::invalid("body.chunks"));
    }
    let mut total = 0_u64;
    for chunk in chunks {
        if chunk.length == 0 || chunk.length > MAX_CHUNK_BYTES {
            return Err(ProtocolError::invalid("body.chunk.length"));
        }
        if chunk.delay_ms > MAX_DELAY_MS {
            return Err(ProtocolError::invalid("body.chunk.delay_ms"));
        }
        if chunk.wait_for_gate.is_some_and(|gate| gate > MAX_GATE_ID) {
            return Err(ProtocolError::invalid("body.chunk.wait_for_gate"));
        }
        total = total
            .checked_add(u64::from(chunk.length))
            .ok_or_else(|| ProtocolError::invalid("body.length"))?;
        if total > MAX_BODY_BYTES {
            return Err(ProtocolError::invalid("body.length"));
        }
    }
    Ok(())
}

fn validate_service_worker(service_worker: &ServiceWorkerSpec) -> Result<(), ProtocolError> {
    let ServiceWorkerSpec::Synthetic {
        status,
        headers,
        body,
    } = service_worker
    else {
        return Ok(());
    };
    if !(200..=599).contains(status) {
        return Err(ProtocolError::invalid("service_worker.status"));
    }
    if headers.len() > 16 {
        return Err(ProtocolError::invalid("service_worker.headers"));
    }
    for (name, value) in headers {
        if name.is_empty()
            || name.len() > 64
            || HeaderName::from_bytes(name.as_bytes()).is_err()
            || value.len() > MAX_HEADER_VALUE_BYTES
            || HeaderValue::from_str(value).is_err()
        {
            return Err(ProtocolError::invalid("service_worker.headers"));
        }
    }
    match body {
        SyntheticBodySpec::Finite { chunks } => {
            if chunks.is_empty() || chunks.len() > MAX_BODY_CHUNKS {
                return Err(ProtocolError::invalid("service_worker.body.chunks"));
            }
            let mut total = 0_u64;
            for chunk in chunks {
                validate_synthetic_chunk(chunk)?;
                total = total
                    .checked_add(u64::from(chunk.length))
                    .ok_or_else(|| ProtocolError::invalid("service_worker.body.length"))?;
                if total > MAX_BODY_BYTES {
                    return Err(ProtocolError::invalid("service_worker.body.length"));
                }
            }
        }
        SyntheticBodySpec::Infinite {
            chunk_bytes,
            delay_ms,
            ..
        } => {
            if *chunk_bytes == 0 || *chunk_bytes > MAX_CHUNK_BYTES || *delay_ms > MAX_DELAY_MS {
                return Err(ProtocolError::invalid("service_worker.body.infinite"));
            }
        }
        SyntheticBodySpec::ZeroProgressThenStall { pulls, delay_ms } => {
            if *pulls == 0 || *pulls > 32 || *delay_ms > MAX_DELAY_MS {
                return Err(ProtocolError::invalid("service_worker.body.zero_progress"));
            }
        }
    }
    Ok(())
}

fn validate_synthetic_chunk(chunk: &SyntheticChunkSpec) -> Result<(), ProtocolError> {
    if chunk.length == 0 || chunk.length > MAX_CHUNK_BYTES || chunk.delay_ms > MAX_DELAY_MS {
        return Err(ProtocolError::invalid("service_worker.body.chunk"));
    }
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reqwest::redirect::Policy;

    use super::*;

    fn two_chunk_range() -> ScenarioSpec {
        let mut spec = ScenarioSpec::exact_range(64, 4);
        spec.object_seed = 10;
        spec.request.range = HeaderRequirement::Exact("bytes=2-5".to_owned());
        spec.request.if_match = HeaderRequirement::Exact("\"v1\"".to_owned());
        spec.body = BodySpec::Finite {
            chunks: vec![
                BodyChunkSpec {
                    length: 2,
                    delay_ms: 0,
                    wait_for_gate: None,
                },
                BodyChunkSpec {
                    length: 2,
                    delay_ms: 1,
                    wait_for_gate: None,
                },
            ],
        };
        spec
    }

    async fn bootstrap(client: &reqwest::Client, server: &McapRangeTestServer) -> FixtureBootstrap {
        client
            .get(server.bootstrap_url())
            .send()
            .await
            .expect("bootstrap request")
            .error_for_status()
            .expect("bootstrap status")
            .json()
            .await
            .expect("bootstrap JSON")
    }

    async fn register_over_http(
        client: &reqwest::Client,
        bootstrap: &FixtureBootstrap,
        spec: &ScenarioSpec,
    ) -> ScenarioDescriptor {
        client
            .post(format!(
                "{}{}/control/scenarios",
                bootstrap.page_origin, bootstrap.control_root
            ))
            .json(spec)
            .send()
            .await
            .expect("register request")
            .error_for_status()
            .expect("register status")
            .json()
            .await
            .expect("register JSON")
    }

    fn control_url(bootstrap: &FixtureBootstrap, id: ScenarioId, suffix: &str) -> String {
        format!(
            "{}{}/control/scenarios/{}{}",
            bootstrap.page_origin, bootstrap.control_root, id.0, suffix
        )
    }

    #[tokio::test]
    async fn control_protocol_serves_real_origins_and_redacts_queries() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        assert!(server.page_addr().ip().is_loopback());
        assert!(server.object_addr().ip().is_loopback());
        assert_ne!(server.page_addr(), server.object_addr());
        let client = reqwest::Client::new();
        let bootstrap = bootstrap(&client, &server).await;
        assert_eq!(bootstrap.protocol, PROTOCOL_VERSION);
        assert_eq!(bootstrap.page_origin, server.page_origin());
        assert_eq!(bootstrap.object_origin, server.object_origin());
        assert_ne!(bootstrap.page_origin, bootstrap.object_origin);

        let descriptor = register_over_http(&client, &bootstrap, &two_chunk_range()).await;
        assert!(descriptor.page_url.starts_with(&bootstrap.page_origin));
        assert!(
            descriptor
                .same_origin_object_url
                .starts_with(&bootstrap.page_origin)
        );
        assert!(
            descriptor
                .cross_origin_object_url
                .starts_with(&bootstrap.object_origin)
        );

        let page = client
            .get(&descriptor.page_url)
            .send()
            .await
            .expect("controlled page");
        let csp = page
            .headers()
            .get("content-security-policy")
            .expect("CSP")
            .to_str()
            .expect("ASCII CSP")
            .to_owned();
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains(&bootstrap.object_origin));
        let page_html = page.text().await.expect("page HTML");
        assert!(page_html.contains("controlled-page.js"));
        assert!(!page_html.contains("<script>"));

        let response = client
            .get(format!(
                "{}?credential=must-not-enter-events",
                descriptor.cross_origin_object_url
            ))
            .header("origin", &bootstrap.page_origin)
            .header("range", "bytes=2-5")
            .header("if-match", "\"v1\"")
            .header("referer", "https://host.invalid/?secret=also-redacted")
            .send()
            .await
            .expect("range response");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_RANGE], "bytes 2-5/64");
        assert_eq!(response.headers()[CONTENT_LENGTH], "4");
        assert_eq!(response.headers()[ETAG], "\"v1\"");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            bootstrap.page_origin
        );
        assert_eq!(
            response.bytes().await.expect("range body").as_ref(),
            &[12, 13, 14, 15]
        );

        let preflight = client
            .request(
                Method::OPTIONS,
                format!(
                    "{}?preflight_secret=must-also-be-redacted",
                    descriptor.cross_origin_object_url
                ),
            )
            .header("origin", "https://host.invalid/?origin_secret=redacted")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "range")
            .send()
            .await
            .expect("redaction preflight");
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);

        let event_response = client
            .post(control_url(&bootstrap, descriptor.id, "/browser-events"))
            .json(&BrowserEvent::ArrayBufferCalled {})
            .send()
            .await
            .expect("browser event");
        assert_eq!(event_response.status(), StatusCode::NO_CONTENT);

        let snapshot: EventLogSnapshot = client
            .get(control_url(&bootstrap, descriptor.id, ""))
            .send()
            .await
            .expect("event snapshot")
            .error_for_status()
            .expect("event snapshot status")
            .json()
            .await
            .expect("event snapshot JSON");
        assert!(snapshot.events.iter().any(|event| matches!(
            event,
            FixtureEvent::Request {
                query_present: true,
                range: ObservedHeader::Ascii(range),
                if_match: ObservedHeader::Ascii(if_match),
                ..
            } if range == "bytes=2-5" && if_match == "\"v1\""
        )));
        assert!(snapshot.events.iter().any(|event| matches!(
            event,
            FixtureEvent::Browser {
                event: BrowserEvent::ArrayBufferCalled {}
            }
        )));
        let encoded = serde_json::to_string(&snapshot).expect("serialize event snapshot");
        assert!(!encoded.contains("must-not-enter-events"));
        assert!(!encoded.contains("also-redacted"));
        assert!(!encoded.contains("must-also-be-redacted"));
        assert!(!encoded.contains("origin_secret"));

        let removed = client
            .delete(control_url(&bootstrap, descriptor.id, ""))
            .send()
            .await
            .expect("remove scenario");
        assert_eq!(removed.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            client
                .get(descriptor.cross_origin_object_url)
                .send()
                .await
                .expect("removed scenario request")
                .status(),
            StatusCode::NOT_FOUND
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn header_redirect_csp_and_service_worker_matrix_is_scriptable() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .expect("client");
        let bootstrap = bootstrap(&client, &server).await;

        let mut spec = ScenarioSpec::exact_range(16, 1);
        spec.request.head = HeadBehavior::MethodNotAllowed;
        spec.cors.preflight = PreflightBehavior::AllowRequiredHeaders;
        spec.cors.expose = ExposeHeaders::RequiredRangeHeadersAndContentEncoding;
        spec.etag = EtagSpec::Duplicate(vec!["\"v1\"".to_owned(), "W/\"v1\"".to_owned()]);
        spec.content_encoding = ContentEncodingSpec::Gzip;
        spec.content_length = ContentLengthSpec::Omit;
        spec.redirect = RedirectSpec::CrossOriginTemporary;
        spec.csp_connect = CspConnectPolicy::SelfOnly;
        spec.service_worker = ServiceWorkerSpec::Synthetic {
            status: 206,
            headers: BTreeMap::from([
                ("content-range".to_owned(), "bytes 0-0/16".to_owned()),
                ("etag".to_owned(), "\"synthetic\"".to_owned()),
            ]),
            body: SyntheticBodySpec::ZeroProgressThenStall {
                pulls: 3,
                delay_ms: 1,
            },
        };
        let descriptor = register_over_http(&client, &bootstrap, &spec).await;

        let preflight = client
            .request(Method::OPTIONS, &descriptor.cross_origin_object_url)
            .header("origin", &bootstrap.page_origin)
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "range, if-match")
            .send()
            .await
            .expect("preflight");
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "range, if-match"
        );

        let head = client
            .head(&descriptor.cross_origin_object_url)
            .send()
            .await
            .expect("HEAD");
        assert_eq!(head.status(), StatusCode::METHOD_NOT_ALLOWED);

        let redirect = client
            .get(&descriptor.same_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("redirect");
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            redirect.headers()[ACCESS_CONTROL_EXPOSE_HEADERS],
            "content-range, content-length, content-encoding, etag"
        );
        assert!(
            redirect.headers()[LOCATION]
                .to_str()
                .expect("redirect location")
                .starts_with(&bootstrap.object_origin)
        );

        let redirected_url = format!("{}/redirected", descriptor.cross_origin_object_url);
        let redirected = client
            .get(redirected_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("redirect target");
        assert_eq!(redirected.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(redirected.headers()[CONTENT_ENCODING], "gzip");
        assert!(!redirected.headers().contains_key(CONTENT_LENGTH));
        assert_eq!(redirected.headers().get_all(ETAG).iter().count(), 2);

        let page = client
            .get(&descriptor.page_url)
            .send()
            .await
            .expect("controlled page");
        let csp = page.headers()["content-security-policy"]
            .to_str()
            .expect("CSP");
        assert!(csp.contains("connect-src 'self'"));
        assert!(!csp.contains(&bootstrap.object_origin));

        let browser_config: serde_json::Value = client
            .get(control_url(&bootstrap, descriptor.id, "/browser-config"))
            .send()
            .await
            .expect("browser config")
            .error_for_status()
            .expect("browser config status")
            .json()
            .await
            .expect("browser config JSON");
        assert_eq!(browser_config["service_worker"]["type"], "synthetic");
        assert_eq!(
            browser_config["service_worker"]["body"]["type"],
            "zero_progress_then_stall"
        );

        for script in ["controlled-page.js", "service-worker.js"] {
            let response = client
                .get(format!(
                    "{}{}/{script}",
                    bootstrap.page_origin, bootstrap.control_root
                ))
                .send()
                .await
                .expect("fixture script");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[CONTENT_TYPE],
                "text/javascript; charset=utf-8"
            );
            assert!(!response.text().await.expect("script source").is_empty());
        }
        server.shutdown().await;
    }

    #[tokio::test]
    async fn etag_body_and_response_revision_matrix_is_exact() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        let client = reqwest::Client::new();

        let etag_cases = [
            (EtagSpec::Absent, Vec::<&str>::new()),
            (EtagSpec::Strong("v1".to_owned()), vec!["\"v1\""]),
            (EtagSpec::Weak("v1".to_owned()), vec!["W/\"v1\""]),
            (
                EtagSpec::Raw("w/\"malformed\"".to_owned()),
                vec!["w/\"malformed\""],
            ),
            (
                EtagSpec::Duplicate(vec!["\"v1\"".to_owned(), "\"V1\"".to_owned()]),
                vec!["\"v1\"", "\"V1\""],
            ),
            (
                EtagSpec::List(vec!["\"v1\"".to_owned(), "W/\"v2\"".to_owned()]),
                vec!["\"v1\", W/\"v2\""],
            ),
            (EtagSpec::Raw("\"a,b\"".to_owned()), vec!["\"a,b\""]),
        ];
        for (etag, expected) in etag_cases {
            let mut spec = ScenarioSpec::exact_range(8, 1);
            spec.etag = etag;
            let descriptor = server.register_scenario(spec).expect("register ETag case");
            let response = client
                .get(&descriptor.cross_origin_object_url)
                .header("range", "bytes=0-0")
                .send()
                .await
                .expect("ETag response");
            let actual = response
                .headers()
                .get_all(ETAG)
                .iter()
                .map(|value| value.to_str().expect("ASCII ETag"))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
            _ = response.bytes().await.expect("ETag response body");
        }

        for (length, expected) in [(1, 1_usize), (0, 0), (2, 2)] {
            let mut spec = ScenarioSpec::exact_range(8, 1);
            spec.content_length = ContentLengthSpec::Omit;
            spec.body = BodySpec::Finite {
                chunks: (length > 0)
                    .then_some(BodyChunkSpec {
                        length,
                        delay_ms: 0,
                        wait_for_gate: None,
                    })
                    .into_iter()
                    .collect(),
            };
            let descriptor = server.register_scenario(spec).expect("register body case");
            let response = client
                .get(&descriptor.cross_origin_object_url)
                .header("range", "bytes=0-0")
                .send()
                .await
                .expect("body case response");
            assert!(!response.headers().contains_key(CONTENT_LENGTH));
            assert_eq!(
                response.bytes().await.expect("body case bytes").len(),
                expected
            );
        }

        let mut changing = ScenarioSpec::exact_range(16, 1);
        changing.response_revisions = vec![
            ResponseRevision {
                etag: Some(EtagSpec::Strong("v1".to_owned())),
                ..Default::default()
            },
            ResponseRevision {
                etag: Some(EtagSpec::Strong("V1".to_owned())),
                ..Default::default()
            },
            ResponseRevision {
                etag: Some(EtagSpec::Weak("v1".to_owned())),
                ..Default::default()
            },
            ResponseRevision {
                object_length: Some(17),
                status: Some(412),
                content_range: Some(ContentRangeSpec::Omit {}),
                content_length: Some(ContentLengthSpec::BodyLength),
                etag: Some(EtagSpec::Absent),
                body: Some(BodySpec::Finite { chunks: Vec::new() }),
                ..Default::default()
            },
        ];
        let changing = server
            .register_scenario(changing)
            .expect("register response sequence");
        for (status, etag) in [
            (206, Some("\"v1\"")),
            (206, Some("\"V1\"")),
            (206, Some("W/\"v1\"")),
            (412, None),
            // The final revision remains active after the sequence is exhausted.
            (412, None),
        ] {
            let response = client
                .get(&changing.cross_origin_object_url)
                .header("range", "bytes=0-0")
                .send()
                .await
                .expect("revision response");
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok()),
                etag
            );
            _ = response.bytes().await.expect("revision body");
        }
        let snapshot = server.snapshot(changing.id).expect("revision events");
        let ordinals = snapshot
            .events
            .iter()
            .filter_map(|event| match event {
                FixtureEvent::ResponseStarted { ordinal, .. } => Some(*ordinal),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(ordinals, [0, 1, 2, 3, 4]);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn gates_cancellation_and_shutdown_are_deterministic() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        let client = reqwest::Client::new();
        let bootstrap = bootstrap(&client, &server).await;

        let mut gated_spec = ScenarioSpec::exact_range(8, 2);
        gated_spec.content_length = ContentLengthSpec::Omit;
        gated_spec.body = BodySpec::Finite {
            chunks: vec![BodyChunkSpec {
                length: 2,
                delay_ms: 0,
                wait_for_gate: Some(7),
            }],
        };
        let gated = register_over_http(&client, &bootstrap, &gated_spec).await;
        let gated_response = client
            .get(&gated.cross_origin_object_url)
            .header("range", "bytes=0-1")
            .send()
            .await
            .expect("gated response headers");
        server
            .wait_for_event(gated.id, Duration::from_secs(1), |event| {
                matches!(event, FixtureEvent::WaitingForGate { gate: 7 })
            })
            .await
            .expect("wait for gate event");
        let release = client
            .post(control_url(&bootstrap, gated.id, "/gates/7"))
            .send()
            .await
            .expect("release gate");
        assert_eq!(release.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            gated_response.bytes().await.expect("gated body").as_ref(),
            &[0, 1]
        );

        let mut stalled_spec = ScenarioSpec::exact_range(8, 1);
        stalled_spec.content_length = ContentLengthSpec::Omit;
        stalled_spec.body = BodySpec::StallAfter { chunks: Vec::new() };
        let stalled = register_over_http(&client, &bootstrap, &stalled_spec).await;
        let mut stalled_response = client
            .get(&stalled.cross_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("stalled response headers");
        let mut pending_chunk = Box::pin(stalled_response.chunk());
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut pending_chunk)
                .await
                .is_err()
        );
        drop(pending_chunk);
        drop(stalled_response);
        server
            .wait_for_event(stalled.id, Duration::from_secs(2), |event| {
                matches!(
                    event,
                    FixtureEvent::BodyCancelled {
                        reason: BodyEndReason::ClientDisconnected,
                        bytes: 0,
                    }
                )
            })
            .await
            .expect("stalled body cancellation");

        let mut infinite_spec = ScenarioSpec::exact_range(1024, 1);
        infinite_spec.content_length = ContentLengthSpec::Omit;
        infinite_spec.body = BodySpec::Infinite {
            chunk: BodyChunkSpec {
                length: MAX_CHUNK_BYTES,
                delay_ms: 1,
                wait_for_gate: None,
            },
        };
        let infinite = register_over_http(&client, &bootstrap, &infinite_spec).await;
        let mut response = client
            .get(&infinite.cross_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("infinite response");
        assert!(
            response
                .chunk()
                .await
                .expect("first infinite chunk")
                .is_some()
        );
        assert_eq!(server.active_response_bodies(), 1);
        drop(response);
        server
            .wait_for_event(infinite.id, Duration::from_secs(2), |event| {
                matches!(
                    event,
                    FixtureEvent::BodyCancelled {
                        reason: BodyEndReason::ClientDisconnected,
                        ..
                    }
                )
            })
            .await
            .expect("client disconnect event");

        let page_addr = server.page_addr();
        tokio::time::timeout(Duration::from_secs(2), server.shutdown())
            .await
            .expect("fixture shutdown completed");
        assert!(
            reqwest::get(format!("http://{page_addr}{ROUTE_PREFIX}/bootstrap"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn revision_only_gates_release_cancel_and_shutdown_without_panics() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        let client = reqwest::Client::new();
        let bootstrap = bootstrap(&client, &server).await;

        let mut revision_gates = ScenarioSpec::exact_range(8, 1);
        assert!(
            revision_gates
                .body
                .chunks()
                .iter()
                .all(|chunk| chunk.wait_for_gate.is_none())
        );
        revision_gates.content_length = ContentLengthSpec::Omit;
        revision_gates.response_revisions = vec![
            ResponseRevision {
                body: Some(BodySpec::Finite {
                    chunks: vec![BodyChunkSpec {
                        length: 1,
                        delay_ms: 0,
                        wait_for_gate: Some(5),
                    }],
                }),
                ..Default::default()
            },
            ResponseRevision {
                body: Some(BodySpec::StallAfter {
                    chunks: vec![BodyChunkSpec {
                        length: 1,
                        delay_ms: 0,
                        wait_for_gate: Some(6),
                    }],
                }),
                ..Default::default()
            },
        ];
        let revision_gates = register_over_http(&client, &bootstrap, &revision_gates).await;

        let released_response = client
            .get(&revision_gates.cross_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("revision-gated response");
        server
            .wait_for_event(revision_gates.id, Duration::from_secs(1), |event| {
                matches!(event, FixtureEvent::WaitingForGate { gate: 5 })
            })
            .await
            .expect("revision gate 5 wait");
        assert_eq!(
            client
                .post(control_url(&bootstrap, revision_gates.id, "/gates/5"))
                .send()
                .await
                .expect("release revision gate 5")
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            released_response
                .bytes()
                .await
                .expect("released revision body")
                .as_ref(),
            &[0]
        );

        let mut cancelled_response = client
            .get(&revision_gates.cross_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("cancelled revision response");
        server
            .wait_for_event(revision_gates.id, Duration::from_secs(1), |event| {
                matches!(event, FixtureEvent::WaitingForGate { gate: 6 })
            })
            .await
            .expect("revision gate 6 wait");
        client
            .post(control_url(&bootstrap, revision_gates.id, "/gates/6"))
            .send()
            .await
            .expect("release revision gate 6")
            .error_for_status()
            .expect("release gate 6 status");
        assert_eq!(
            cancelled_response
                .chunk()
                .await
                .expect("revision body chunk")
                .expect("revision body byte")
                .as_ref(),
            &[0]
        );
        let cancelled_state = server
            .shared
            .scenario(revision_gates.id)
            .expect("revision state before delete");
        let removed = client
            .delete(control_url(&bootstrap, revision_gates.id, ""))
            .send()
            .await
            .expect("delete revision scenario");
        assert_eq!(removed.status(), StatusCode::NO_CONTENT);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), cancelled_response.chunk())
                .await
                .expect("cancelled body converged")
                .expect("cancelled body read")
                .is_none()
        );
        assert!(
            cancelled_state
                .snapshot()
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    FixtureEvent::BodyCancelled {
                        reason: BodyEndReason::ScenarioRemoved,
                        ..
                    }
                ))
        );

        let mut shutdown_gate = ScenarioSpec::exact_range(8, 1);
        shutdown_gate.content_length = ContentLengthSpec::Omit;
        shutdown_gate.response_revisions = vec![ResponseRevision {
            body: Some(BodySpec::StallAfter {
                chunks: vec![BodyChunkSpec {
                    length: 1,
                    delay_ms: 0,
                    wait_for_gate: Some(7),
                }],
            }),
            ..Default::default()
        }];
        let shutdown_gate = register_over_http(&client, &bootstrap, &shutdown_gate).await;
        let shutdown_state = server
            .shared
            .scenario(shutdown_gate.id)
            .expect("shutdown revision state");
        let mut shutdown_response = client
            .get(&shutdown_gate.cross_origin_object_url)
            .header("range", "bytes=0-0")
            .send()
            .await
            .expect("shutdown revision response");
        server
            .wait_for_event(shutdown_gate.id, Duration::from_secs(1), |event| {
                matches!(event, FixtureEvent::WaitingForGate { gate: 7 })
            })
            .await
            .expect("shutdown revision gate wait");
        let pending_chunk = tokio::spawn(async move { shutdown_response.chunk().await });
        server.shutdown().await;
        _ = pending_chunk.await.expect("shutdown body task");
        assert!(
            shutdown_state
                .snapshot()
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    FixtureEvent::BodyCancelled {
                        reason: BodyEndReason::ServerShutdown,
                        ..
                    }
                ))
        );
    }

    #[tokio::test]
    async fn protocol_caps_bound_specs_and_event_retention() {
        let server = McapRangeTestServer::spawn().await.expect("spawn fixture");
        let client = reqwest::Client::new();
        let bootstrap = bootstrap(&client, &server).await;

        let mut invalid = ScenarioSpec::exact_range(1, 1);
        invalid.object_length = MAX_OBJECT_BYTES + 1;
        assert_eq!(
            server
                .register_scenario(invalid)
                .expect_err("invalid spec")
                .field,
            "object_length"
        );

        let oversized = client
            .post(format!(
                "{}{}/control/scenarios",
                bootstrap.page_origin, bootstrap.control_root
            ))
            .body(vec![b' '; MAX_CONTROL_BODY_BYTES + 1])
            .send()
            .await
            .expect("oversized control request");
        assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let mut tagged_unknown =
            serde_json::to_value(ScenarioSpec::exact_range(8, 1)).expect("scenario JSON");
        tagged_unknown["body"]["unknown_wire_field"] = serde_json::json!(true);
        let unknown_response = client
            .post(format!(
                "{}{}/control/scenarios",
                bootstrap.page_origin, bootstrap.control_root
            ))
            .json(&tagged_unknown)
            .send()
            .await
            .expect("unknown tagged field response");
        assert_eq!(unknown_response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let descriptor = server
            .register_scenario(ScenarioSpec::exact_range(8, 1))
            .expect("browser event scenario");
        let unknown_browser_event = client
            .post(format!(
                "{}{}/control/scenarios/{}/browser-events",
                bootstrap.page_origin, bootstrap.control_root, descriptor.id.0
            ))
            .json(&serde_json::json!({
                "type": "array_buffer_called",
                "unknown_wire_field": true,
            }))
            .send()
            .await
            .expect("unknown browser event field response");
        assert_eq!(
            unknown_browser_event.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let mut oversized_etag_list = ScenarioSpec::exact_range(8, 1);
        oversized_etag_list.etag = EtagSpec::List(vec!["a".repeat(300), "b".repeat(300)]);
        assert_eq!(
            server
                .register_scenario(oversized_etag_list)
                .expect_err("oversized ETag list")
                .field,
            "etag_list_bytes"
        );

        let mut maximum_offsets = ScenarioSpec::exact_range(8, 2);
        maximum_offsets.content_range = ContentRangeSpec::Fixed {
            start: u64::MAX,
            end: u64::MAX,
            total: u64::MAX,
        };
        maximum_offsets.body = BodySpec::Finite {
            chunks: vec![BodyChunkSpec {
                length: 2,
                delay_ms: 0,
                wait_for_gate: None,
            }],
        };
        let maximum_offsets = server
            .register_scenario(maximum_offsets)
            .expect("u64::MAX response offsets");
        let maximum_response = client
            .get(&maximum_offsets.cross_origin_object_url)
            .send()
            .await
            .expect("u64::MAX response");
        assert_eq!(
            maximum_response.headers()[CONTENT_RANGE],
            "bytes 18446744073709551615-18446744073709551615/18446744073709551615"
        );
        assert_eq!(
            maximum_response
                .bytes()
                .await
                .expect("wrapped pattern")
                .as_ref(),
            &[u8::MAX, 0]
        );

        let descriptor =
            register_over_http(&client, &bootstrap, &ScenarioSpec::exact_range(8, 1)).await;
        for _ in 0..MAX_EVENTS_PER_SCENARIO {
            let response = client
                .post(control_url(&bootstrap, descriptor.id, "/browser-events"))
                .json(&BrowserEvent::ArrayBufferCalled {})
                .send()
                .await
                .expect("bounded browser event");
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
        let overflow = client
            .post(control_url(&bootstrap, descriptor.id, "/browser-events"))
            .json(&BrowserEvent::ArrayBufferCalled {})
            .send()
            .await
            .expect("overflow browser event");
        assert_eq!(overflow.status(), StatusCode::TOO_MANY_REQUESTS);
        let snapshot = server.snapshot(descriptor.id).expect("snapshot");
        assert_eq!(snapshot.events.len(), MAX_EVENTS_PER_SCENARIO);
        assert!(snapshot.retained_bytes <= MAX_EVENT_BYTES_PER_SCENARIO);
        assert!(snapshot.overflowed);

        let mut infinite_spec = ScenarioSpec::exact_range(8, 1);
        infinite_spec.content_length = ContentLengthSpec::Omit;
        infinite_spec.body = BodySpec::Infinite {
            chunk: BodyChunkSpec {
                length: 1,
                delay_ms: 100,
                wait_for_gate: None,
            },
        };
        let infinite = server
            .register_scenario(infinite_spec)
            .expect("register active-body cap scenario");
        let mut held_responses = Vec::new();
        for _ in 0..MAX_ACTIVE_BODIES_PER_SCENARIO {
            held_responses.push(
                client
                    .get(&infinite.cross_origin_object_url)
                    .header("range", "bytes=0-0")
                    .send()
                    .await
                    .expect("held infinite response"),
            );
        }
        assert_eq!(
            server.active_response_bodies(),
            MAX_ACTIVE_BODIES_PER_SCENARIO
        );
        assert_eq!(
            client
                .get(&infinite.cross_origin_object_url)
                .header("range", "bytes=0-0")
                .send()
                .await
                .expect("active-body overflow response")
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(held_responses);
        server.shutdown().await;
    }
}
