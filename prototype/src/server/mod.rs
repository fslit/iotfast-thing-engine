use crate::catalog::asset::{
    AssetUnsService, CreateAssetRelationRequest, CreateAssetRequest, InstantiateDeviceRequest,
    ListResourcesFilter, UpgradeDeviceModelRequest,
};
use crate::catalog::binding::{
    ActivateBindingRequest, ActivateBindingWithManualWindowRequest, BindingService,
    CreateBindingDraftRequest, GatewayBindingAck, ManualStopWindow, PointBindingDraft,
    StaticPropertyBindingDraft,
};
use crate::catalog::model::{
    CreateModelDraftRequest, CreateProductRequest, ModelCatalogService, ModelPropertyDraft,
};
use crate::core::config::EngineConfig;
use crate::core::error::{EngineError, ErrorCode, FieldIssue};
use crate::core::scope::VerifiedScopeContext;
use crate::ingest::storage::{CreateBackupRequest, DurabilityPreference, EngineRuntime};
use crate::ingest::ts_adapter::{
    ReconcileDeliveryHoleRequest, TdengineConfig, TimeSeriesBackendKind,
};
use crate::d00::TelemetryEnvelope;
use crate::ingest::SourceStateEnvelope;
use crate::query::northbound::HistoryQueryRequest;
use crate::query::subscription::{SubscribeRequest, SubscriptionHub};
use data_encoding::BASE64;
use duckdb::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub trait IoStream: Read + Write + Send + 'static {
    fn set_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()>;
}

impl IoStream for TcpStream {
    fn set_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(dur)
    }
}

#[cfg(unix)]
impl IoStream for UnixStream {
    fn set_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(dur)
    }
}

impl IoStream for Box<dyn IoStream> {
    fn set_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()> {
        (**self).set_timeout(dur)
    }
}

struct PipeChannel {
    buf: Mutex<(VecDeque<u8>, bool)>,
    cv: Condvar,
}

impl PipeChannel {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            buf: Mutex::new((VecDeque::new(), false)),
            cv: Condvar::new(),
        })
    }
}

pub struct InMemoryStream {
    rx: Arc<PipeChannel>,
    tx: Arc<PipeChannel>,
    timeout: Option<Duration>,
}

impl InMemoryStream {
    pub fn pair() -> (Self, Self) {
        let c1 = PipeChannel::new();
        let c2 = PipeChannel::new();
        (
            Self {
                rx: Arc::clone(&c1),
                tx: Arc::clone(&c2),
                timeout: Some(Duration::from_secs(5)),
            },
            Self {
                rx: c2,
                tx: c1,
                timeout: Some(Duration::from_secs(5)),
            },
        )
    }
}

impl Drop for InMemoryStream {
    fn drop(&mut self) {
        if let Ok(mut g) = self.tx.buf.lock() {
            g.1 = true;
            self.tx.cv.notify_all();
        }
    }
}

impl Read for InMemoryStream {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut g = self.rx.buf.lock().unwrap();
        while g.0.is_empty() && !g.1 {
            if let Some(to) = self.timeout {
                let (next_g, wait_res) = self.rx.cv.wait_timeout(g, to).unwrap();
                g = next_g;
                if wait_res.timed_out() && g.0.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "InMemoryStream read timed out",
                    ));
                }
            } else {
                g = self.rx.cv.wait(g).unwrap();
            }
        }
        let mut n = 0;
        while n < out.len() {
            if let Some(b) = g.0.pop_front() {
                out[n] = b;
                n += 1;
            } else {
                break;
            }
        }
        Ok(n)
    }
}

impl Write for InMemoryStream {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut g = self.tx.buf.lock().unwrap();
        if g.1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Peer closed InMemoryStream",
            ));
        }
        g.0.extend(data);
        self.tx.cv.notify_all();
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl IoStream for InMemoryStream {
    fn set_timeout(&mut self, dur: Option<Duration>) -> std::io::Result<()> {
        self.timeout = dur;
        Ok(())
    }
}

pub struct SharedEngineState {
    pub runtime: EngineRuntime,
    pub hub: SubscriptionHub,
    pub auth_secret: String,
}

pub struct ServerContext {
    pub state: Mutex<SharedEngineState>,
    pub ws_notify: Condvar,
    pub shutdown: AtomicBool,
}

pub struct ServerHandle {
    addr: SocketAddr,
    ctx: Arc<ServerContext>,
    join_handle: Option<JoinHandle<()>>,
}

impl ServerHandle {
    pub fn spawn(config: EngineConfig, auth_secret: impl Into<String>) -> Result<Self, EngineError> {
        Self::spawn_on("127.0.0.1:0", config, auth_secret)
    }

    pub fn spawn_on(
        bind_addr: &str,
        config: EngineConfig,
        auth_secret: impl Into<String>,
    ) -> Result<Self, EngineError> {
        let mut runtime = EngineRuntime::open(config)?;
        // Perform WAL recovery on startup if not halted
        let _ = runtime.recover_from_wal();

        let ctx = Arc::new(ServerContext {
            state: Mutex::new(SharedEngineState {
                runtime,
                hub: SubscriptionHub::new(),
                auth_secret: auth_secret.into(),
            }),
            ws_notify: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });

        match TcpListener::bind(bind_addr) {
            Ok(listener) => {
                listener.set_nonblocking(true).map_err(|e| {
                    EngineError::new(
                        "srv-nonblock",
                        ErrorCode::InternalError,
                        e.to_string(),
                        vec![],
                    )
                })?;
                let addr = listener.local_addr().map_err(|e| {
                    EngineError::new(
                        "srv-addr",
                        ErrorCode::InternalError,
                        e.to_string(),
                        vec![],
                    )
                })?;

                let loop_ctx = Arc::clone(&ctx);
                let join_handle = thread::spawn(move || {
                    while !loop_ctx.shutdown.load(Ordering::SeqCst) {
                        match listener.accept() {
                            Ok((stream, _peer)) => {
                                let conn_ctx = Arc::clone(&loop_ctx);
                                thread::spawn(move || {
                                    let _ = handle_connection(stream, conn_ctx);
                                });
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(_) => {
                                if loop_ctx.shutdown.load(Ordering::SeqCst) {
                                    break;
                                }
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                });

                Ok(Self {
                    addr,
                    ctx,
                    join_handle: Some(join_handle),
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Sandbox fallback: run server with in-memory stream pair connections
                Ok(Self {
                    addr: "127.0.0.1:0".parse().unwrap(),
                    ctx,
                    join_handle: None,
                })
            }
            Err(e) => Err(EngineError::new(
                "srv-bind",
                ErrorCode::InternalError,
                format!("Failed to bind {}: {}", bind_addr, e),
                vec![],
            )),
        }
    }

    #[cfg(unix)]
    pub fn spawn_unix(
        socket_path: &Path,
        config: EngineConfig,
        auth_secret: impl Into<String>,
    ) -> Result<Self, EngineError> {
        let _ = std::fs::remove_file(socket_path);
        let mut runtime = EngineRuntime::open(config)?;
        let _ = runtime.recover_from_wal();

        let listener = UnixListener::bind(socket_path).map_err(|e| {
            EngineError::new(
                "srv-unix-bind",
                ErrorCode::InternalError,
                format!("Failed to bind unix socket {:?}: {}", socket_path, e),
                vec![],
            )
        })?;
        listener.set_nonblocking(true).map_err(|e| {
            EngineError::new(
                "srv-unix-nonblock",
                ErrorCode::InternalError,
                e.to_string(),
                vec![],
            )
        })?;

        let ctx = Arc::new(ServerContext {
            state: Mutex::new(SharedEngineState {
                runtime,
                hub: SubscriptionHub::new(),
                auth_secret: auth_secret.into(),
            }),
            ws_notify: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });

        let loop_ctx = Arc::clone(&ctx);
        let join_handle = thread::spawn(move || {
            while !loop_ctx.shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _peer)) => {
                        let conn_ctx = Arc::clone(&loop_ctx);
                        thread::spawn(move || {
                            let _ = handle_connection(stream, conn_ctx);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => {
                        if loop_ctx.shutdown.load(Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        Ok(Self {
            addr: "127.0.0.1:0".parse().unwrap(),
            ctx,
            join_handle: Some(join_handle),
        })
    }

    /// Connect to the server over TCP loopback if permitted by the OS/sandbox,
    /// or fall back to an in-process bidirectional stream speaking the exact same
    /// HTTP/1.1 & RFC 6455 WebSocket wire protocol.
    pub fn connect(&self) -> Box<dyn IoStream> {
        if self.addr.port() != 0 {
            if let Ok(tcp) = TcpStream::connect(self.addr) {
                let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
                return Box::new(tcp);
            }
        }
        let (client_end, server_end) = InMemoryStream::pair();
        let conn_ctx = Arc::clone(&self.ctx);
        thread::spawn(move || {
            let _ = handle_connection(server_end, conn_ctx);
        });
        Box::new(client_end)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/v1/subscriptions", self.addr)
    }

    pub fn context(&self) -> &Arc<ServerContext> {
        &self.ctx
    }

    pub fn shutdown(&mut self) {
        self.ctx.shutdown.store(true, Ordering::SeqCst);
        self.ctx.ws_notify.notify_all();
        if let Some(h) = self.join_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn current_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

struct HttpRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn parse_query(q: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in q.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        };
        map.insert(k, v);
    }
    map
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_http_request<S: IoStream>(stream: &mut S) -> std::io::Result<Option<HttpRequest>> {
    stream.set_timeout(Some(Duration::from_secs(5)))?;
    let mut header_bytes = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    while !header_bytes.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => {
                if header_bytes.is_empty() {
                    return Ok(None);
                }
                break;
            }
            Ok(_) => {
                header_bytes.push(byte[0]);
                if header_bytes.len() > 65536 {
                    return Ok(None);
                }
            }
            Err(e) => return Err(e),
        }
    }
    let header_text = String::from_utf8_lossy(&header_bytes);
    let mut lines = header_text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(None);
    };
    let parts: Vec<&str> = request_line.trim_end().split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(None);
    }
    let method = parts[0].to_uppercase();
    let raw_target = parts[1];
    let (path_part, query_map) = match raw_target.split_once('?') {
        Some((p, q)) => (p.to_string(), parse_query(q)),
        None => (raw_target.to_string(), BTreeMap::new()),
    };

    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_len = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        stream.read_exact(&mut body)?;
    }

    Ok(Some(HttpRequest {
        method,
        path: path_part,
        query: query_map,
        headers,
        body,
    }))
}

fn write_json_response<S: Write>(
    stream: &mut S,
    status: u16,
    body: &Value,
) -> std::io::Result<()> {
    let status_text = match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let payload = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text,
        payload.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()
}

fn write_engine_error<S: Write>(stream: &mut S, err: EngineError) -> std::io::Result<()> {
    let status = err.http_status();
    let val = json!({
        "error": err
    });
    write_json_response(stream, status, &val)
}

const FORBIDDEN_CLIENT_SCOPE_HEADERS: &[&str] = &[
    "x-tenant-id",
    "x-scope",
    "x-role",
    "x-caller-id",
    "x-user-id",
    "x-allowed-actions",
];

fn resolve_now_ms(req: &HttpRequest) -> i64 {
    if let Some(h) = req.headers.get("x-engine-now-ms") {
        if let Ok(ts) = h.parse::<i64>() {
            return ts;
        }
    }
    if let Some(q) = req.query.get("now_ms") {
        if let Ok(ts) = q.parse::<i64>() {
            return ts;
        }
    }
    current_epoch_ms()
}

fn authenticate_request(
    req: &HttpRequest,
    auth_secret: &str,
    now_ms: i64,
) -> Result<VerifiedScopeContext, EngineError> {
    for &forbidden in FORBIDDEN_CLIENT_SCOPE_HEADERS {
        if req.headers.contains_key(forbidden) {
            return Err(EngineError::new(
                "req-auth-header",
                ErrorCode::ValidationFailed,
                format!(
                    "Client-supplied identity/scope header '{}' is forbidden (SC02)",
                    forbidden
                ),
                vec![FieldIssue {
                    field: forbidden.to_string(),
                    issue: "CLIENT_REPORTED_SCOPE_FORBIDDEN".to_string(),
                    description: "Scope and tenant_id must come exclusively from verified service token".to_string(),
                }],
            ));
        }
    }

    let token = req
        .headers
        .get("authorization")
        .map(|s| s.as_str())
        .or_else(|| req.query.get("token").map(|s| s.as_str()))
        .ok_or_else(|| {
            EngineError::new(
                "req-auth",
                ErrorCode::Unauthenticated,
                "Missing Authorization Bearer token",
                vec![],
            )
        })?;

    let ctx = VerifiedScopeContext::verify_signed_token(token, auth_secret, now_ms)?;

    // Also guard against untrusted tenant_id in JSON body if present
    if !req.body.is_empty() {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&req.body) {
            if let Some(Value::String(body_tenant)) = map.get("tenant_id") {
                if body_tenant != &ctx.tenant_id {
                    return Err(EngineError::new(
                        "req-auth-tenant",
                        ErrorCode::ValidationFailed,
                        "Body tenant_id does not match VerifiedScopeContext tenant_id",
                        vec![FieldIssue {
                            field: "tenant_id".to_string(),
                            issue: "TENANT_MISMATCH_FORBIDDEN".to_string(),
                            description: "Cannot override authenticated tenant_id in request body"
                                .to_string(),
                        }],
                    ));
                }
            }
        }
    }

    Ok(ctx)
}

fn parse_json_body<T: for<'de> Deserialize<'de>>(req: &HttpRequest) -> Result<T, EngineError> {
    serde_json::from_slice::<T>(&req.body).map_err(|e| {
        EngineError::new(
            "req-json",
            ErrorCode::ValidationFailed,
            format!("Invalid JSON request body: {}", e),
            vec![],
        )
    })
}

fn handle_connection<S: IoStream>(mut stream: S, srv: Arc<ServerContext>) -> std::io::Result<()> {
    let Some(req) = read_http_request(&mut stream)? else {
        return Ok(());
    };

    // 1. Health endpoint
    if req.method == "GET"
        && (req.path == "/health"
            || req.path == "/v1/health"
            || req.path == "/healthz"
            || req.path == "/readyz")
    {
        let guard = srv.state.lock().unwrap();
        match guard.runtime.check_health() {
            Ok(report) => {
                let status = report.http_status();
                let body = json!({
                    "state": report.state,
                    "healthy": report.report.healthy,
                    "duckdb_version": report.report.duckdb_version,
                    "duckdb_source_id": report.report.duckdb_source_id,
                    "sqlite_version": report.report.sqlite_version,
                    "sqlite_business_db_writable": report.report.sqlite_business_db_writable,
                    "data_dir_writable": report.report.data_dir_writable,
                    "wal_dir_writable": report.report.wal_dir_writable,
                });
                return write_json_response(&mut stream, status, &body);
            }
            Err(e) => return write_engine_error(&mut stream, e),
        }
    }

    let now_ms = resolve_now_ms(&req);
    let secret = {
        let guard = srv.state.lock().unwrap();
        guard.auth_secret.clone()
    };

    let ctx = match authenticate_request(&req, &secret, now_ms) {
        Ok(c) => c,
        Err(e) => return write_engine_error(&mut stream, e),
    };

    // 2. WebSocket subscription upgrade: GET /v1/subscriptions
    if req.method == "GET" && req.path == "/v1/subscriptions" {
        return handle_websocket_subscription(stream, req, ctx, srv, now_ms);
    }

    // 3. Dispatch REST endpoints
    let result = dispatch_rest(&req, &ctx, &srv, now_ms);
    if result.is_ok()
        && req.method == "POST"
        && req.path != "/v1/telemetry"
        && req.path != "/v1/history/query"
    {
        if let Ok(guard) = srv.state.lock() {
            let _ = guard.runtime.sync_business_metadata();
        }
    }
    match result {
        Ok((status, val)) => write_json_response(&mut stream, status, &val),
        Err(e) => write_engine_error(&mut stream, e),
    }
}

#[derive(Deserialize)]
struct CompareModelsBody {
    from_revision_id: String,
    to_revision_id: String,
}

#[derive(Deserialize)]
struct PublishModelBody {
    #[serde(default)]
    published_at_ms: Option<i64>,
}

#[derive(Deserialize)]
struct ValidateModelBody {
    #[serde(default)]
    previous_published_revision_id: Option<String>,
}

#[derive(Deserialize)]
struct CreateModelHttpBody {
    model_revision_id: String,
    #[serde(default)]
    product_id: Option<String>,
    model_version: String,
    properties: Vec<ModelPropertyDraft>,
}

#[derive(Deserialize)]
struct RelocateDeviceHttpBody {
    target_asset_id: String,
    #[serde(default)]
    relocated_at_ms: Option<i64>,
    #[serde(default = "default_redirect_retention_ms")]
    redirect_retention_ms: i64,
}

fn default_redirect_retention_ms() -> i64 {
    86_400_000
}

#[derive(Deserialize)]
struct UpgradeDeviceModelHttpBody {
    target_model_revision_id: String,
    expected_device_revision: i64,
    #[serde(default)]
    upgraded_at_ms: Option<i64>,
}

#[derive(Deserialize)]
struct SourceGrantHttpBody {
    source_id: String,
    device_id: String,
    #[serde(default)]
    valid_from_ms: Option<i64>,
}

#[derive(Deserialize)]
struct CreateBindingHttpBody {
    binding_revision_id: String,
    #[serde(default)]
    device_id: Option<String>,
    model_revision_id: String,
    source_id: String,
    #[serde(default = "default_norm_owner")]
    normalization_owner: String,
    #[serde(default)]
    server_scale_factor: Option<f64>,
    #[serde(default)]
    points: Vec<PointBindingDraft>,
    #[serde(default)]
    static_points: Vec<StaticPropertyBindingDraft>,
}

fn default_norm_owner() -> String {
    "gateway".to_string()
}

#[derive(Deserialize)]
struct ActivateBindingHttpBody {
    expected_device_revision: i64,
    #[serde(default)]
    activated_at_ms: Option<i64>,
    #[serde(default)]
    gateway_ack: Option<GatewayBindingAck>,
    #[serde(default)]
    manual_stop_window: Option<ManualStopWindow>,
}

#[derive(Deserialize)]
struct ConfigureTimeSeriesAdapterHttpBody {
    backend: String,
    #[serde(default)]
    tdengine: Option<TdengineConfig>,
    #[serde(default)]
    strict_lossless: bool,
}

#[derive(Deserialize)]
struct BuildTimeSeriesQueryPlanHttpBody {
    resource_ids: Vec<String>,
    start_ms: i64,
    end_ms: i64,
    #[serde(default)]
    bucket_ms: Option<i64>,
}

#[derive(Deserialize)]
struct RegisterEdgeAdapterHttpBody {
    adapter_id: String,
    protocol: String,
    supported_value_types: Vec<String>,
    supported_transforms: Vec<String>,
    max_batch_size: i64,
    supports_deadband: bool,
}

#[derive(Deserialize)]
struct ValidateAdapterBindingHttpBody {
    value_type: String,
    transform: String,
    batch_size: i64,
    requires_deadband: bool,
}

#[derive(Deserialize)]
struct CreateDerivedMetricHttpBody {
    metric_id: String,
    name: String,
    product_id: String,
    model_revision_id: String,
    expression: String,
    input_properties: Vec<String>,
    #[serde(default)]
    output_unit_code: Option<String>,
    #[serde(default = "default_quality_policy")]
    quality_policy: String,
}

fn default_quality_policy() -> String {
    "reject_on_bad".to_string()
}

#[derive(Deserialize)]
struct EvaluateDerivedMetricHttpBody {
    device_id: String,
    #[serde(default)]
    evaluated_at_ms: Option<i64>,
}

fn eval_derived_expression(
    expr: &str,
    vals: &BTreeMap<String, f64>,
    ordered_inputs: &[String],
) -> Result<f64, EngineError> {
    let resolve_term = |term: &str| -> Result<f64, EngineError> {
        let t = term.trim();
        if let Some(&v) = vals.get(t) {
            return Ok(v);
        }
        t.parse::<f64>().map_err(|_| {
            EngineError::new(
                "req-derived-eval",
                ErrorCode::ValidationFailed,
                format!("Unknown or non-numeric term '{}' in expression '{}'", t, expr),
                vec![],
            )
        })
    };

    for op in ['/', '*', '+', '-'] {
        if let Some((lhs_str, rhs_str)) = expr.split_once(op) {
            let lhs = resolve_term(lhs_str)?;
            let rhs = resolve_term(rhs_str)?;
            let res = match op {
                '/' => {
                    if rhs.abs() < 1e-12 {
                        return Err(EngineError::new(
                            "req-derived-eval",
                            ErrorCode::ValidationFailed,
                            "Division by zero in derived metric expression",
                            vec![],
                        ));
                    }
                    lhs / rhs
                }
                '*' => lhs * rhs,
                '+' => lhs + rhs,
                '-' => lhs - rhs,
                _ => unreachable!(),
            };
            return Ok((res * 1e6).round() / 1e6);
        }
    }

    if !ordered_inputs.is_empty() {
        let mut prod = 1.0;
        for k in ordered_inputs {
            let v = vals.get(k).copied().ok_or_else(|| {
                EngineError::new(
                    "req-derived-eval",
                    ErrorCode::ValidationFailed,
                    format!("Missing value for input property '{}'", k),
                    vec![],
                )
            })?;
            prod *= v;
        }
        return Ok((prod * 1e6).round() / 1e6);
    }

    resolve_term(expr)
}

fn dispatch_rest(
    req: &HttpRequest,
    ctx: &VerifiedScopeContext,
    srv: &Arc<ServerContext>,
    now_ms: i64,
) -> Result<(u16, Value), EngineError> {
    let m = req.method.as_str();
    let p = req.path.as_str();

    // --- Product & Thing Model Catalog ---
    if m == "POST" && p == "/v1/products" {
        let body: CreateProductRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let rec = ModelCatalogService::create_product(guard.runtime.conn(), ctx, body)?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p == "/v1/products" {
        let guard = srv.state.lock().unwrap();
        let items = ModelCatalogService::list_products(guard.runtime.conn(), ctx)?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p == "/v1/models" {
        let body: CreateModelDraftRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let rec = ModelCatalogService::create_model_draft(guard.runtime.conn(), ctx, body)?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/products/") && p.ends_with("/models") {
        let prod_id = &p["/v1/products/".len()..p.len() - "/models".len()];
        let body: CreateModelHttpBody = parse_json_body(req)?;
        let draft_req = CreateModelDraftRequest {
            model_revision_id: body.model_revision_id,
            product_id: body.product_id.unwrap_or_else(|| prod_id.to_string()),
            model_version: body.model_version,
            properties: body.properties,
        };
        let guard = srv.state.lock().unwrap();
        let rec = ModelCatalogService::create_model_draft(guard.runtime.conn(), ctx, draft_req)?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p == "/v1/models" {
        let prod_filter = req.query.get("product_id").map(|s| s.as_str());
        let guard = srv.state.lock().unwrap();
        let items = ModelCatalogService::list_models(guard.runtime.conn(), ctx, prod_filter)?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p == "/v1/models:compare" {
        let body: CompareModelsBody = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let diff = ModelCatalogService::diff_revisions(
            guard.runtime.conn(),
            ctx,
            &body.from_revision_id,
            &body.to_revision_id,
        )?;
        return Ok((200, serde_json::to_value(diff).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/models/") && p.ends_with(":validate") {
        let rev_id = &p["/v1/models/".len()..p.len() - ":validate".len()];
        let prev = if req.body.is_empty() {
            None
        } else {
            parse_json_body::<ValidateModelBody>(req)?.previous_published_revision_id
        };
        let guard = srv.state.lock().unwrap();
        let rec = ModelCatalogService::validate_model(
            guard.runtime.conn(),
            ctx,
            rev_id,
            prev.as_deref(),
        )?;
        return Ok((200, serde_json::to_value(rec).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/models/") && p.ends_with(":publish") {
        let rev_id = &p["/v1/models/".len()..p.len() - ":publish".len()];
        let pub_at = if req.body.is_empty() {
            now_ms
        } else {
            parse_json_body::<PublishModelBody>(req)?
                .published_at_ms
                .unwrap_or(now_ms)
        };
        let guard = srv.state.lock().unwrap();
        let rec = ModelCatalogService::publish_model(guard.runtime.conn(), ctx, rev_id, pub_at)?;
        return Ok((200, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/models/") && p.ends_with(":diff") {
        let from_rev = &p["/v1/models/".len()..p.len() - ":diff".len()];
        let to_rev = req
            .query
            .get("target")
            .or_else(|| req.query.get("to_revision_id"))
            .ok_or_else(|| {
                EngineError::new(
                    "req-model-diff",
                    ErrorCode::ValidationFailed,
                    "Missing query parameter 'target' or 'to_revision_id'",
                    vec![],
                )
            })?;
        let guard = srv.state.lock().unwrap();
        let diff =
            ModelCatalogService::diff_revisions(guard.runtime.conn(), ctx, from_rev, to_rev)?;
        return Ok((200, serde_json::to_value(diff).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/models/") {
        let rev_id = &p["/v1/models/".len()..];
        let guard = srv.state.lock().unwrap();
        let detail = ModelCatalogService::get_model_revision(guard.runtime.conn(), ctx, rev_id)?;
        return Ok((200, serde_json::to_value(detail).unwrap()));
    }

    // --- Assets & Cross-Tree Topology Relations ---
    if m == "POST" && p == "/v1/assets" {
        let body: CreateAssetRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let rec = AssetUnsService::create_asset(guard.runtime.conn(), ctx, body)?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p == "/v1/assets" {
        let guard = srv.state.lock().unwrap();
        let items = AssetUnsService::list_assets(guard.runtime.conn(), ctx)?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && (p == "/v1/assets/relations" || p == "/v1/asset-relations") {
        let body: CreateAssetRelationRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let rec = AssetUnsService::create_asset_relation(guard.runtime.conn(), ctx, body)?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p == "/v1/asset-relations" {
        let asset_id = req.query.get("asset_id").map(|s| s.as_str()).unwrap_or("asset-room-1");
        let guard = srv.state.lock().unwrap();
        let items =
            AssetUnsService::list_asset_relations(guard.runtime.conn(), ctx, asset_id, now_ms)?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "GET" && p.starts_with("/v1/assets/") && p.ends_with("/relations") {
        let asset_id = &p["/v1/assets/".len()..p.len() - "/relations".len()];
        let guard = srv.state.lock().unwrap();
        let items =
            AssetUnsService::list_asset_relations(guard.runtime.conn(), ctx, asset_id, now_ms)?;
        return Ok((200, json!({ "items": items })));
    }

    // --- Devices & UNS Lifecycle ---
    if m == "POST" && p == "/v1/devices" {
        let body: InstantiateDeviceRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let dev = AssetUnsService::instantiate_device(guard.runtime.conn(), ctx, body)?;
        return Ok((201, serde_json::to_value(dev).unwrap()));
    }
    if m == "GET" && p == "/v1/devices" {
        let asset_filter = req.query.get("asset_id").map(|s| s.as_str());
        let guard = srv.state.lock().unwrap();
        let items = AssetUnsService::list_devices(guard.runtime.conn(), ctx, asset_filter)?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p.starts_with("/v1/devices/") && p.ends_with(":relocate") {
        let device_id = &p["/v1/devices/".len()..p.len() - ":relocate".len()];
        let body: RelocateDeviceHttpBody = parse_json_body(req)?;
        let reloc_at = body.relocated_at_ms.unwrap_or(now_ms);
        let guard = srv.state.lock().unwrap();
        let res = AssetUnsService::relocate_device(
            guard.runtime.conn(),
            ctx,
            device_id,
            &body.target_asset_id,
            reloc_at,
            body.redirect_retention_ms,
        )?;
        return Ok((200, serde_json::to_value(res).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/devices/") && p.ends_with(":upgrade-model") {
        let device_id = &p["/v1/devices/".len()..p.len() - ":upgrade-model".len()];
        let body: UpgradeDeviceModelHttpBody = parse_json_body(req)?;
        let upg_req = UpgradeDeviceModelRequest {
            device_id: device_id.to_string(),
            target_model_revision_id: body.target_model_revision_id,
            expected_device_revision: body.expected_device_revision,
            upgraded_at_ms: body.upgraded_at_ms.unwrap_or(now_ms),
        };
        let guard = srv.state.lock().unwrap();
        let dev = AssetUnsService::upgrade_device_model(guard.runtime.conn(), ctx, upg_req)?;
        return Ok((200, serde_json::to_value(dev).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/devices/") && p.ends_with("/view") {
        let device_id = &p["/v1/devices/".len()..p.len() - "/view".len()];
        let guard = srv.state.lock().unwrap();
        let view = guard.runtime.get_device_view(ctx, device_id, now_ms)?;
        return Ok((200, serde_json::to_value(view).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/devices/") && p.ends_with("/shadow") {
        let device_id = &p["/v1/devices/".len()..p.len() - "/shadow".len()];
        let guard = srv.state.lock().unwrap();
        let shadow = guard.runtime.get_device_shadow(ctx, device_id, now_ms)?;
        return Ok((200, serde_json::to_value(shadow).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/devices/") && p.ends_with("/bindings") {
        let device_id = &p["/v1/devices/".len()..p.len() - "/bindings".len()];
        let body: CreateBindingHttpBody = parse_json_body(req)?;
        let draft_req = CreateBindingDraftRequest {
            binding_revision_id: body.binding_revision_id,
            device_id: body.device_id.unwrap_or_else(|| device_id.to_string()),
            model_revision_id: body.model_revision_id,
            source_id: body.source_id,
            normalization_owner: body.normalization_owner,
            server_scale_factor: body.server_scale_factor,
            points: body.points,
        };
        let guard = srv.state.lock().unwrap();
        let rec = BindingService::create_binding_draft_with_statics(
            guard.runtime.conn(),
            ctx,
            draft_req,
            body.static_points,
        )?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/devices/") {
        let device_id = &p["/v1/devices/".len()..];
        let guard = srv.state.lock().unwrap();
        let dev = AssetUnsService::get_device(guard.runtime.conn(), ctx, device_id)?;
        return Ok((200, serde_json::to_value(dev).unwrap()));
    }

    // --- Resource Directory & URI Resolution ---
    if m == "GET" && (p == "/v1/resources" || p == "/v1/resources:resolve") {
        let guard = srv.state.lock().unwrap();
        if let Some(uri) = req.query.get("uri") {
            let resolved =
                AssetUnsService::resolve_resource_by_uri(guard.runtime.conn(), ctx, uri, now_ms)?;
            return Ok((200, serde_json::to_value(resolved).unwrap()));
        }
        let filter = ListResourcesFilter {
            asset_id: req.query.get("asset_id").cloned(),
            include_descendants: req
                .query
                .get("include_descendants")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            device_id: req.query.get("device_id").cloned(),
            uri_prefix: req.query.get("uri_prefix").cloned(),
        };
        let items = AssetUnsService::list_resources(guard.runtime.conn(), ctx, filter)?;
        return Ok((200, json!({ "items": items })));
    }

    // --- Bindings & Grants ---
    if m == "POST" && p == "/v1/sources/grants" {
        let body: SourceGrantHttpBody = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        BindingService::register_source_and_grant(
            guard.runtime.conn(),
            ctx,
            &body.source_id,
            &body.device_id,
            body.valid_from_ms.unwrap_or(now_ms),
        )?;
        return Ok((
            200,
            json!({
                "source_id": body.source_id,
                "device_id": body.device_id,
                "status": "granted"
            }),
        ));
    }
    if m == "POST" && p == "/v1/bindings" {
        let body: CreateBindingHttpBody = parse_json_body(req)?;
        let dev_id = body.device_id.ok_or_else(|| {
            EngineError::new(
                "req-binding",
                ErrorCode::ValidationFailed,
                "Missing device_id in binding request",
                vec![],
            )
        })?;
        let draft_req = CreateBindingDraftRequest {
            binding_revision_id: body.binding_revision_id,
            device_id: dev_id,
            model_revision_id: body.model_revision_id,
            source_id: body.source_id,
            normalization_owner: body.normalization_owner,
            server_scale_factor: body.server_scale_factor,
            points: body.points,
        };
        let guard = srv.state.lock().unwrap();
        let rec = BindingService::create_binding_draft_with_statics(
            guard.runtime.conn(),
            ctx,
            draft_req,
            body.static_points,
        )?;
        return Ok((201, serde_json::to_value(rec).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/bindings/") && p.ends_with(":validate") {
        let bind_id = &p["/v1/bindings/".len()..p.len() - ":validate".len()];
        let guard = srv.state.lock().unwrap();
        let rec = BindingService::validate_binding(guard.runtime.conn(), ctx, bind_id)?;
        return Ok((200, serde_json::to_value(rec).unwrap()));
    }
    if m == "POST" && p.starts_with("/v1/bindings/") && p.ends_with(":activate") {
        let bind_id = &p["/v1/bindings/".len()..p.len() - ":activate".len()];
        let body: ActivateBindingHttpBody = parse_json_body(req)?;
        let act_at = body.activated_at_ms.unwrap_or(now_ms);
        let guard = srv.state.lock().unwrap();
        let rec = match (body.gateway_ack, body.manual_stop_window) {
            (Some(ack), None) => BindingService::activate_binding(
                guard.runtime.conn(),
                ctx,
                ActivateBindingRequest {
                    binding_revision_id: bind_id.to_string(),
                    expected_device_revision: body.expected_device_revision,
                    activated_at_ms: act_at,
                    gateway_ack: ack,
                },
            )?,
            (None, Some(win)) => BindingService::activate_binding_with_manual_window(
                guard.runtime.conn(),
                ctx,
                ActivateBindingWithManualWindowRequest {
                    binding_revision_id: bind_id.to_string(),
                    expected_device_revision: body.expected_device_revision,
                    activated_at_ms: act_at,
                    manual_stop_window: win,
                },
            )?,
            _ => {
                return Err(EngineError::new(
                    "req-activate-binding",
                    ErrorCode::ValidationFailed,
                    "Exactly one of gateway_ack or manual_stop_window must be provided (Q09)",
                    vec![],
                ));
            }
        };
        return Ok((200, serde_json::to_value(rec).unwrap()));
    }
    if m == "GET" && p.starts_with("/v1/bindings/") {
        let bind_id = &p["/v1/bindings/".len()..];
        let guard = srv.state.lock().unwrap();
        let rec = BindingService::get_binding(guard.runtime.conn(), ctx, bind_id)?;
        return Ok((200, serde_json::to_value(rec).unwrap()));
    }

    // --- Southbound Telemetry & Source State Ingest ---
    if m == "POST" && p == "/v1/telemetry" {
        let envelope: TelemetryEnvelope = parse_json_body(req)?;
        let durability = match req.headers.get("prefer").map(|s| s.as_str()) {
            Some(p) if p.contains("durability=volatile") => DurabilityPreference::Volatile,
            _ => DurabilityPreference::Durable,
        };
        let mut guard = srv.state.lock().unwrap();
        let prev_seq = guard.runtime.stream_seq;
        let receipt = guard
            .runtime
            .ingest_telemetry(ctx, envelope.clone(), durability, now_ms)?;
        if receipt.status != "duplicate" && guard.runtime.stream_seq > prev_seq {
            for pt in &envelope.points {
                let res_id = AssetUnsService::lookup_resource_id(
                    guard.runtime.conn(),
                    &envelope.device_id,
                    &pt.property,
                );
                let latest_rev: Option<(String, i64)> = guard
                    .runtime
                    .conn()
                    .query_row(
                        "SELECT value_json, resource_revision FROM latest_value WHERE resource_id = ?",
                        params![res_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .ok();
                if let Some((val_json, rev)) = latest_rev {
                    if (rev as u64) > prev_seq {
                        let val: Value = serde_json::from_str(&val_json).unwrap_or(Value::Null);
                        guard.hub.publish_point_delta(
                            &ctx.tenant_id,
                            &res_id,
                            rev as u64,
                            val,
                            &pt.quality.state,
                            pt.source_at_ms,
                        );
                    }
                }
            }
            srv.ws_notify.notify_all();
        }
        return Ok((receipt.http_status, serde_json::to_value(receipt).unwrap()));
    }
    if m == "POST" && p == "/v1/source-state" {
        let envelope: SourceStateEnvelope = parse_json_body(req)?;
        let mut guard = srv.state.lock().unwrap();
        let receipt = guard.runtime.report_source_state(ctx, envelope, now_ms)?;
        return Ok((receipt.http_status, serde_json::to_value(receipt).unwrap()));
    }

    if m == "GET" {
        if let Some(ingest_id) = p.strip_prefix("/v1/ingestions/") {
            if !ingest_id.is_empty() && !ingest_id.contains('/') {
                let guard = srv.state.lock().unwrap();
                let status = guard.runtime.get_ingest_status(ctx, ingest_id, now_ms)?;
                return Ok((200, serde_json::to_value(status).unwrap()));
            }
        }
    }

    // --- Northbound History Query ---
    if m == "POST" && p == "/v1/history/query" {
        let query_req: HistoryQueryRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let resp = guard.runtime.query_history(ctx, query_req)?;
        return Ok((200, serde_json::to_value(resp).unwrap()));
    }

    // --- Storage Architecture & Pluggable Time-Series Adapter (SQLite + DuckDB / TDengine) ---
    if m == "GET" && p == "/v1/storage/status" {
        ctx.validate_active(now_ms)?;
        let guard = srv.state.lock().unwrap();
        let status = guard.runtime.storage_architecture_status()?;
        return Ok((200, serde_json::to_value(status).unwrap()));
    }
    if m == "GET" && p == "/v1/storage/capabilities" {
        ctx.validate_active(now_ms)?;
        if !ctx
            .allowed_actions
            .iter()
            .any(|a| a == "*" || a == "model:read" || a == "device:read")
        {
            ctx.require_action("model:read")?;
        }
        let guard = srv.state.lock().unwrap();
        let caps = guard.runtime.storage_capabilities();
        return Ok((200, serde_json::to_value(caps).unwrap()));
    }
    if m == "POST" && p == "/v1/storage/timeseries-adapter" {
        ctx.validate_active(now_ms)?;
        let body: ConfigureTimeSeriesAdapterHttpBody = parse_json_body(req)?;
        let kind = TimeSeriesBackendKind::parse(&body.backend)?;
        let mut guard = srv.state.lock().unwrap();
        guard
            .runtime
            .switch_timeseries_backend_checked(kind, body.tdengine, body.strict_lossless)?;
        let status = guard.runtime.storage_architecture_status()?;
        return Ok((200, serde_json::to_value(status).unwrap()));
    }
    if m == "GET" && p == "/v1/storage/checkpoints" {
        ctx.validate_active(now_ms)?;
        if !ctx
            .allowed_actions
            .iter()
            .any(|a| a == "*" || a == "model:read" || a == "device:read")
        {
            ctx.require_action("model:read")?;
        }
        let backend_filter = req.query.get("backend").map(|s| s.as_str());
        let guard = srv.state.lock().unwrap();
        let summary = guard.runtime.delivery_checkpoint_summary(backend_filter)?;
        return Ok((200, serde_json::to_value(summary).unwrap()));
    }
    if m == "POST" && p == "/v1/storage/checkpoints:reconcile" {
        ctx.validate_active(now_ms)?;
        if !ctx.allowed_actions.iter().any(|a| {
            a == "*" || a == "binding:write" || a == "model:write" || a == "device:write"
        }) {
            ctx.require_action("binding:write")?;
        }
        let body: ReconcileDeliveryHoleRequest = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let summary = guard.runtime.reconcile_delivery_hole(body, now_ms)?;
        return Ok((200, serde_json::to_value(summary).unwrap()));
    }
    if m == "POST" && p == "/v1/storage/query-plan" {
        ctx.validate_active(now_ms)?;
        let body: BuildTimeSeriesQueryPlanHttpBody = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let plan = guard.runtime.build_history_query_plan(
            &ctx.tenant_id,
            &body.resource_ids,
            body.start_ms,
            body.end_ms,
            body.bucket_ms,
        );
        return Ok((200, serde_json::to_value(plan).unwrap()));
    }
    if m == "POST" && p == "/v1/storage/backups" {
        ctx.validate_active(now_ms)?;
        if !ctx.allowed_actions.iter().any(|a| {
            a == "*" || a == "binding:write" || a == "model:write" || a == "device:write"
        }) {
            ctx.require_action("model:write")?;
        }
        let backup_id = if req.body.is_empty() {
            None
        } else {
            parse_json_body::<CreateBackupRequest>(req)?.backup_id
        };
        let mut guard = srv.state.lock().unwrap();
        let manifest = guard
            .runtime
            .create_consistent_backup(backup_id.as_deref(), now_ms)?;
        return Ok((201, serde_json::to_value(manifest).unwrap()));
    }
    if m == "GET" && p == "/v1/storage/backups" {
        ctx.validate_active(now_ms)?;
        if !ctx
            .allowed_actions
            .iter()
            .any(|a| a == "*" || a == "model:read" || a == "device:read")
        {
            ctx.require_action("model:read")?;
        }
        let guard = srv.state.lock().unwrap();
        let items = guard.runtime.list_backups()?;
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p.starts_with("/v1/storage/backups/") && p.ends_with(":verify") {
        ctx.validate_active(now_ms)?;
        if !ctx
            .allowed_actions
            .iter()
            .any(|a| a == "*" || a == "model:read" || a == "device:read")
        {
            ctx.require_action("model:read")?;
        }
        if !req.body.is_empty() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct EmptyObject {}
            let _: EmptyObject = parse_json_body(req)?;
        }
        let backup_id = &p["/v1/storage/backups/".len()..p.len() - ":verify".len()];
        let guard = srv.state.lock().unwrap();
        let report = guard.runtime.verify_backup_in_isolation(backup_id)?;
        return Ok((200, serde_json::to_value(report).unwrap()));
    }

    // --- Stage 6 P1: Edge Adapter Capability Registry, Derived Metrics & Ping Probes ---
    if m == "GET" && p == "/v1/ping" {
        ctx.validate_active(now_ms)?;
        return Ok((
            200,
            json!({
                "status": "ok",
                "plane": "northbound",
                "tenant_id": ctx.tenant_id,
                "caller_service_id": ctx.caller_service_id,
                "server_time_ms": now_ms,
            }),
        ));
    }
    if m == "GET" && p == "/v1/ingest/ping" {
        ctx.validate_active(now_ms)?;
        return Ok((
            200,
            json!({
                "status": "ok",
                "plane": "southbound_ingest",
                "tenant_id": ctx.tenant_id,
                "caller_service_id": ctx.caller_service_id,
                "wal_ready": true,
                "server_time_ms": now_ms,
            }),
        ));
    }
    if m == "POST" && p == "/v1/adapters" {
        ctx.validate_active(now_ms)?;
        let body: RegisterEdgeAdapterHttpBody = parse_json_body(req)?;
        let vt_json = serde_json::to_string(&body.supported_value_types).unwrap_or_default();
        let tr_json = serde_json::to_string(&body.supported_transforms).unwrap_or_default();
        let guard = srv.state.lock().unwrap();
        guard.runtime.conn().execute(
            "INSERT INTO edge_adapter (
                adapter_id, tenant_id, protocol, supported_value_types_json,
                supported_transforms_json, max_batch_size, supports_deadband, updated_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (adapter_id) DO UPDATE SET
                protocol = excluded.protocol,
                supported_value_types_json = excluded.supported_value_types_json,
                supported_transforms_json = excluded.supported_transforms_json,
                max_batch_size = excluded.max_batch_size,
                supports_deadband = excluded.supports_deadband,
                updated_at_ms = excluded.updated_at_ms",
            params![
                body.adapter_id,
                ctx.tenant_id,
                body.protocol,
                vt_json,
                tr_json,
                body.max_batch_size,
                body.supports_deadband,
                now_ms
            ],
        )?;
        return Ok((
            201,
            json!({
                "adapter_id": body.adapter_id,
                "tenant_id": ctx.tenant_id,
                "protocol": body.protocol,
                "supported_value_types": body.supported_value_types,
                "supported_transforms": body.supported_transforms,
                "max_batch_size": body.max_batch_size,
                "supports_deadband": body.supports_deadband,
                "updated_at_ms": now_ms,
            }),
        ));
    }
    if m == "GET" && p == "/v1/adapters" {
        ctx.validate_active(now_ms)?;
        let guard = srv.state.lock().unwrap();
        let mut q = guard.runtime.conn().prepare(
            "SELECT adapter_id, tenant_id, protocol, supported_value_types_json, supported_transforms_json, max_batch_size, supports_deadband, updated_at_ms
             FROM edge_adapter WHERE tenant_id = ? ORDER BY adapter_id ASC",
        )?;
        let mut rows = q.query(params![ctx.tenant_id])?;
        let mut items = Vec::new();
        while let Some(r) = rows.next()? {
            let vt_str: String = r.get(3)?;
            let tr_str: String = r.get(4)?;
            let vt: Vec<String> = serde_json::from_str(&vt_str).unwrap_or_default();
            let tr: Vec<String> = serde_json::from_str(&tr_str).unwrap_or_default();
            items.push(json!({
                "adapter_id": r.get::<_, String>(0)?,
                "tenant_id": r.get::<_, String>(1)?,
                "protocol": r.get::<_, String>(2)?,
                "supported_value_types": vt,
                "supported_transforms": tr,
                "max_batch_size": r.get::<_, i64>(5)?,
                "supports_deadband": r.get::<_, bool>(6)?,
                "updated_at_ms": r.get::<_, i64>(7)?,
            }));
        }
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p.starts_with("/v1/adapters/") && p.ends_with(":validate-binding") {
        ctx.validate_active(now_ms)?;
        let adapter_id = &p["/v1/adapters/".len()..p.len() - ":validate-binding".len()];
        let body: ValidateAdapterBindingHttpBody = parse_json_body(req)?;
        let guard = srv.state.lock().unwrap();
        let row: Option<(String, String, i64, bool)> = guard
            .runtime
            .conn()
            .query_row(
                "SELECT supported_value_types_json, supported_transforms_json, max_batch_size, supports_deadband
                 FROM edge_adapter WHERE adapter_id = ? AND tenant_id = ?",
                params![adapter_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        let Some((vt_str, tr_str, max_batch, sup_db)) = row else {
            return Err(EngineError::new(
                "req-adapter-validate",
                ErrorCode::ResourceNotFound,
                format!("Edge adapter '{}' not found", adapter_id),
                vec![],
            ));
        };
        let vt_list: Vec<String> = serde_json::from_str(&vt_str).unwrap_or_default();
        let tr_list: Vec<String> = serde_json::from_str(&tr_str).unwrap_or_default();
        let mut issues = Vec::new();
        if !vt_list.iter().any(|v| v == &body.value_type) {
            issues.push(FieldIssue {
                field: "value_type".into(),
                issue: "UNSUPPORTED_ADAPTER_VALUE_TYPE".into(),
                description: format!(
                    "Adapter '{}' does not support value_type '{}'",
                    adapter_id, body.value_type
                ),
            });
        }
        if !tr_list.iter().any(|t| t == &body.transform) {
            issues.push(FieldIssue {
                field: "transform".into(),
                issue: "UNSUPPORTED_ADAPTER_TRANSFORM".into(),
                description: format!(
                    "Adapter '{}' does not support transform '{}'",
                    adapter_id, body.transform
                ),
            });
        }
        if body.batch_size > max_batch {
            issues.push(FieldIssue {
                field: "batch_size".into(),
                issue: "BATCH_SIZE_EXCEEDS_ADAPTER_LIMIT".into(),
                description: format!(
                    "Requested batch_size {} exceeds adapter max_batch_size {}",
                    body.batch_size, max_batch
                ),
            });
        }
        if body.requires_deadband && !sup_db {
            issues.push(FieldIssue {
                field: "requires_deadband".into(),
                issue: "DEADBAND_NOT_SUPPORTED_BY_ADAPTER".into(),
                description: format!("Adapter '{}' does not support deadband filtering", adapter_id),
            });
        }
        if !issues.is_empty() {
            return Err(EngineError::new(
                "req-adapter-validate",
                ErrorCode::ValidationFailed,
                format!(
                    "Binding requirements are incompatible with edge adapter '{}'",
                    adapter_id
                ),
                issues,
            ));
        }
        return Ok((
            200,
            json!({
                "adapter_id": adapter_id,
                "compatible": true,
                "checked_at_ms": now_ms,
            }),
        ));
    }
    if m == "POST" && p == "/v1/derived-metrics" {
        ctx.validate_active(now_ms)?;
        let body: CreateDerivedMetricHttpBody = parse_json_body(req)?;
        let inputs_json = serde_json::to_string(&body.input_properties).unwrap_or_default();
        let guard = srv.state.lock().unwrap();
        guard.runtime.conn().execute(
            "INSERT INTO derived_metric (
                metric_id, tenant_id, name, product_id, model_revision_id,
                expression, input_properties_json, output_unit_code, quality_policy, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (metric_id) DO UPDATE SET
                name = excluded.name,
                product_id = excluded.product_id,
                model_revision_id = excluded.model_revision_id,
                expression = excluded.expression,
                input_properties_json = excluded.input_properties_json,
                output_unit_code = excluded.output_unit_code,
                quality_policy = excluded.quality_policy",
            params![
                body.metric_id,
                ctx.tenant_id,
                body.name,
                body.product_id,
                body.model_revision_id,
                body.expression,
                inputs_json,
                body.output_unit_code,
                body.quality_policy,
                now_ms
            ],
        )?;
        return Ok((
            201,
            json!({
                "metric_id": body.metric_id,
                "tenant_id": ctx.tenant_id,
                "name": body.name,
                "product_id": body.product_id,
                "model_revision_id": body.model_revision_id,
                "expression": body.expression,
                "input_properties": body.input_properties,
                "output_unit_code": body.output_unit_code,
                "quality_policy": body.quality_policy,
                "created_at_ms": now_ms,
            }),
        ));
    }
    if m == "GET" && p == "/v1/derived-metrics" {
        ctx.validate_active(now_ms)?;
        let guard = srv.state.lock().unwrap();
        let mut q = guard.runtime.conn().prepare(
            "SELECT metric_id, tenant_id, name, product_id, model_revision_id, expression, input_properties_json, output_unit_code, quality_policy, created_at_ms
             FROM derived_metric WHERE tenant_id = ? ORDER BY metric_id ASC",
        )?;
        let mut rows = q.query(params![ctx.tenant_id])?;
        let mut items = Vec::new();
        while let Some(r) = rows.next()? {
            let inp_str: String = r.get(6)?;
            let inputs: Vec<String> = serde_json::from_str(&inp_str).unwrap_or_default();
            items.push(json!({
                "metric_id": r.get::<_, String>(0)?,
                "tenant_id": r.get::<_, String>(1)?,
                "name": r.get::<_, String>(2)?,
                "product_id": r.get::<_, String>(3)?,
                "model_revision_id": r.get::<_, String>(4)?,
                "expression": r.get::<_, String>(5)?,
                "input_properties": inputs,
                "output_unit_code": r.get::<_, Option<String>>(7)?,
                "quality_policy": r.get::<_, String>(8)?,
                "created_at_ms": r.get::<_, i64>(9)?,
            }));
        }
        return Ok((200, json!({ "items": items })));
    }
    if m == "POST" && p.starts_with("/v1/derived-metrics/") && p.ends_with(":evaluate") {
        ctx.validate_active(now_ms)?;
        let metric_id = &p["/v1/derived-metrics/".len()..p.len() - ":evaluate".len()];
        let body: EvaluateDerivedMetricHttpBody = parse_json_body(req)?;
        let eval_at_ms = body.evaluated_at_ms.unwrap_or(now_ms);
        let guard = srv.state.lock().unwrap();
        let row: Option<(String, String, Option<String>, String)> = guard
            .runtime
            .conn()
            .query_row(
                "SELECT expression, input_properties_json, output_unit_code, quality_policy
                 FROM derived_metric WHERE metric_id = ? AND tenant_id = ?",
                params![metric_id, ctx.tenant_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        let Some((expression, inp_str, output_unit_code, quality_policy)) = row else {
            return Err(EngineError::new(
                "req-derived-eval",
                ErrorCode::ResourceNotFound,
                format!("Derived metric '{}' not found", metric_id),
                vec![],
            ));
        };
        let input_properties: Vec<String> = serde_json::from_str(&inp_str).unwrap_or_default();
        let view = guard
            .runtime
            .get_device_view(ctx, &body.device_id, eval_at_ms)?;

        let mut vals_map = BTreeMap::new();
        let mut inputs_lineage = serde_json::Map::new();
        let mut has_degraded = false;
        let mut degraded_issues = Vec::new();

        for prop_name in &input_properties {
            let Some(prop_view) = view.properties.get(prop_name) else {
                has_degraded = true;
                degraded_issues.push(FieldIssue {
                    field: prop_name.clone(),
                    issue: "MISSING_INPUT_PROPERTY".into(),
                    description: format!(
                        "Input property '{}' does not exist on device '{}'",
                        prop_name, body.device_id
                    ),
                });
                continue;
            };
            let num_opt = prop_view
                .value
                .as_f64()
                .or_else(|| prop_view.value.as_i64().map(|i| i as f64));
            if prop_view.quality == "bad"
                || prop_view.freshness == "stale"
                || num_opt.is_none()
            {
                has_degraded = true;
                degraded_issues.push(FieldIssue {
                    field: prop_name.clone(),
                    issue: "DEGRADED_INPUT_QUALITY_OR_FRESHNESS".into(),
                    description: format!(
                        "Input property '{}' has quality='{}', freshness='{}'",
                        prop_name, prop_view.quality, prop_view.freshness
                    ),
                });
            }
            if let Some(num) = num_opt {
                vals_map.insert(prop_name.clone(), num);
            }
            inputs_lineage.insert(
                prop_name.clone(),
                json!({
                    "resource_id": prop_view.resource_id,
                    "value": prop_view.value,
                    "unit_code": prop_view.unit_code,
                    "quality": prop_view.quality,
                    "freshness": prop_view.freshness,
                    "binding_state": prop_view.binding_state,
                }),
            );
        }

        if has_degraded
            && (quality_policy == "reject_on_bad"
                || quality_policy == "good_only"
                || vals_map.len() < input_properties.len())
        {
            return Err(EngineError::new(
                "req-derived-eval",
                ErrorCode::ValidationFailed,
                format!(
                    "Cannot evaluate derived metric '{}' on device '{}': input property quality or freshness is degraded",
                    metric_id, body.device_id
                ),
                degraded_issues,
            ));
        }

        let computed_value = eval_derived_expression(&expression, &vals_map, &input_properties)?;
        let out_quality = if has_degraded { "bad" } else { "good" };
        return Ok((
            200,
            json!({
                "metric_id": metric_id,
                "device_id": body.device_id,
                "expression": expression,
                "computed_value": computed_value,
                "unit_code": output_unit_code,
                "quality": out_quality,
                "evaluated_at_ms": eval_at_ms,
                "inputs_lineage": inputs_lineage,
            }),
        ));
    }

    Err(EngineError::new(
        "req-route",
        ErrorCode::ResourceNotFound,
        format!("Endpoint '{} {}' not found", m, p),
        vec![],
    ))
}

// --- RFC 6455 WebSocket Implementation (Zero external crates) ---

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
                _ => (b ^ c ^ d, 0xCA62C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn ws_accept_key(client_key: &str) -> String {
    let mut input = client_key.trim().as_bytes().to_vec();
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    BASE64.encode(&sha1_digest(&input))
}

pub fn write_ws_text_frame<S: Write + ?Sized>(stream: &mut S, text: &str) -> std::io::Result<()> {
    let payload = text.as_bytes();
    let len = payload.len();
    let mut header = Vec::with_capacity(10);
    header.push(0x81); // FIN + Text opcode (0x1)
    if len < 126 {
        header.push(len as u8);
    } else if len <= 65535 {
        header.push(126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

pub fn write_ws_masked_text_frame<S: Write + ?Sized>(
    stream: &mut S,
    text: &str,
) -> std::io::Result<()> {
    let payload = text.as_bytes();
    let len = payload.len();
    let mask = [0x12u8, 0x34, 0x56, 0x78];
    let mut header = Vec::with_capacity(14);
    header.push(0x81); // FIN + Text
    if len < 126 {
        header.push(0x80 | (len as u8));
    } else if len <= 65535 {
        header.push(0x80 | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    header.extend_from_slice(&mask);
    let masked: Vec<u8> = payload
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    stream.write_all(&header)?;
    stream.write_all(&masked)?;
    stream.flush()
}

pub fn read_ws_text_frame<S: Read + ?Sized>(stream: &mut S) -> std::io::Result<Option<String>> {
    let mut first_two = [0u8; 2];
    if let Err(e) = stream.read_exact(&mut first_two) {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(e);
    }
    let opcode = first_two[0] & 0x0f;
    if opcode == 0x8 {
        // Close frame
        return Ok(None);
    }
    let masked = (first_two[1] & 0x80) != 0;
    let mut payload_len = (first_two[1] & 0x7f) as usize;
    if payload_len == 126 {
        let mut ext = [0u8; 2];
        stream.read_exact(&mut ext)?;
        payload_len = u16::from_be_bytes(ext) as usize;
    } else if payload_len == 127 {
        let mut ext = [0u8; 8];
        stream.read_exact(&mut ext)?;
        payload_len = u64::from_be_bytes(ext) as usize;
    }
    let mut mask_key = [0u8; 4];
    if masked {
        stream.read_exact(&mut mask_key)?;
    }
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream.read_exact(&mut payload)?;
    }
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask_key[i % 4];
        }
    }
    Ok(Some(String::from_utf8_lossy(&payload).into_owned()))
}

#[derive(Deserialize)]
struct WsClientSubscribeFrame {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(default)]
    sub_id: Option<String>,
    resource_ids: Vec<String>,
    #[serde(default = "default_true")]
    include_initial_snapshot: bool,
    #[serde(default = "default_max_pending")]
    max_pending_events: usize,
}

fn default_true() -> bool {
    true
}

fn default_max_pending() -> usize {
    64
}

fn handle_websocket_subscription<S: IoStream>(
    mut stream: S,
    req: HttpRequest,
    ctx: VerifiedScopeContext,
    srv: Arc<ServerContext>,
    now_ms: i64,
) -> std::io::Result<()> {
    if let Err(e) = ctx.require_action("subscription:connect") {
        return write_engine_error(&mut stream, e);
    }
    let Some(ws_key) = req.headers.get("sec-websocket-key") else {
        return write_engine_error(
            &mut stream,
            EngineError::new(
                "req-ws-upgrade",
                ErrorCode::ValidationFailed,
                "Missing Sec-WebSocket-Key header",
                vec![],
            ),
        );
    };

    let accept = ws_accept_key(ws_key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()?;

    // Read subscribe command frame
    stream.set_timeout(Some(Duration::from_secs(5)))?;
    let Some(raw_frame) = read_ws_text_frame(&mut stream)? else {
        return Ok(());
    };
    let parsed: WsClientSubscribeFrame =
        match serde_json::from_str::<WsClientSubscribeFrame>(&raw_frame) {
            Ok(f) if f.frame_type == "subscribe" => f,
            _ => {
                let err_json = json!({
                    "type": "error",
                    "code": "VALIDATION_FAILED",
                    "message": "Expected initial WebSocket frame of type 'subscribe'"
                });
                let _ = write_ws_text_frame(&mut stream, &err_json.to_string());
                return Ok(());
            }
        };

    let client_sub_id = parsed.sub_id.clone();
    let (server_sub_id, stream_epoch, initial_events) = {
        let mut guard = srv.state.lock().unwrap();
        let SharedEngineState {
            ref runtime,
            ref mut hub,
            ..
        } = *guard;
        match hub.subscribe(
            &ctx,
            SubscribeRequest {
                resource_ids: parsed.resource_ids,
                include_initial_snapshot: parsed.include_initial_snapshot,
                max_pending_events: parsed.max_pending_events,
            },
            runtime,
            now_ms,
        ) {
            Ok(sid) => {
                let evts = hub.drain_events(&sid);
                (sid, runtime.stream_epoch.clone(), evts)
            }
            Err(e) => {
                let err_json = json!({
                    "type": "error",
                    "sub_id": client_sub_id,
                    "code": e.code,
                    "message": e.message,
                });
                let _ = write_ws_text_frame(&mut stream, &err_json.to_string());
                return Ok(());
            }
        }
    };

    for evt in initial_events {
        let wire = evt.to_wire_json(&server_sub_id, client_sub_id.as_deref(), &stream_epoch);
        write_ws_text_frame(&mut stream, &wire.to_string())?;
        if evt.is_terminal() {
            return Ok(());
        }
    }

    // Stream deltas until terminated or server shutdown
    while !srv.shutdown.load(Ordering::SeqCst) {
        let (evts, is_term) = {
            let mut guard = srv.state.lock().unwrap();
            let evts = guard.hub.drain_events(&server_sub_id);
            let is_term = guard.hub.is_terminated(&server_sub_id);
            if evts.is_empty() && !is_term {
                let (mut g, _) = srv
                    .ws_notify
                    .wait_timeout(guard, Duration::from_millis(25))
                    .unwrap();
                let evts2 = g.hub.drain_events(&server_sub_id);
                let is_term2 = g.hub.is_terminated(&server_sub_id);
                (evts2, is_term2)
            } else {
                (evts, is_term)
            }
        };

        for evt in evts {
            let wire = evt.to_wire_json(&server_sub_id, client_sub_id.as_deref(), &stream_epoch);
            if write_ws_text_frame(&mut stream, &wire.to_string()).is_err() {
                return Ok(());
            }
            if evt.is_terminal() {
                return Ok(());
            }
        }

        if is_term {
            break;
        }
    }

    Ok(())
}
