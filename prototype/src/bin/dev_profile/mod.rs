use duckdb::{params, Connection};
use iotfast_thing_engine::d00::{
    compute_telemetry_payload_hash, derive_observation_id, parse_source_seq, validate_entity_id,
    validate_message_id, validate_source_epoch, TelemetryEnvelope, ValueType,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}

struct Field(String);
impl Field { fn equiv(&self, name: &str) -> bool { self.0.eq_ignore_ascii_case(name) } }
struct Header { field: Field, value: String }
struct Request { stream: TcpStream, method: String, path: String, headers: Vec<Header>, buffered: Vec<u8>, deadline: Instant, body_read: bool }
impl Request {
    fn read(mut stream: TcpStream) -> std::io::Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        let end = loop {
            if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") { break position + 4; }
            if bytes.len() >= 16384 { return Err(std::io::ErrorKind::InvalidData.into()); }
            stream.set_read_timeout(Some(deadline.checked_duration_since(Instant::now()).ok_or(std::io::ErrorKind::TimedOut)?))?;
            let mut buffer = [0; 1024];
            let size = stream.read(&mut buffer)?;
            if size == 0 { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
            bytes.extend_from_slice(&buffer[..size]);
        };
        let text = std::str::from_utf8(&bytes[..end]).map_err(|_| std::io::ErrorKind::InvalidData)?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next().unwrap_or("").split_whitespace();
        let method = first.next().ok_or(std::io::ErrorKind::InvalidData)?.to_string();
        let path = first.next().ok_or(std::io::ErrorKind::InvalidData)?.to_string();
        if first.next() != Some("HTTP/1.1") || first.next().is_some() { return Err(std::io::ErrorKind::InvalidData.into()); }
        let mut headers = Vec::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (name, value) = line.split_once(':').ok_or(std::io::ErrorKind::InvalidData)?;
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return Err(std::io::ErrorKind::InvalidData.into()); }
            headers.push(Header { field: Field(name.to_string()), value: value.trim().to_string() });
        }
        if headers.iter().any(|header| header.field.equiv("Transfer-Encoding")) { return Err(std::io::ErrorKind::InvalidData.into()); }
        Ok(Self { stream, method, path, headers, buffered: bytes[end..].to_vec(), deadline, body_read: false })
    }
    fn headers(&self) -> &[Header] { &self.headers }
    fn body_length(&self) -> Option<usize> {
        let values: Vec<_> = self.headers.iter().filter(|header| header.field.equiv("Content-Length")).collect();
        if values.len() != 1 { return None; }
        values[0].value.parse().ok()
    }
    fn body(&mut self) -> std::io::Result<Vec<u8>> {
        self.body_read = true;
        let length = self.body_length().ok_or(std::io::ErrorKind::InvalidData)?;
        if length > 65536 { return Err(std::io::ErrorKind::InvalidData.into()); }
        let mut body = std::mem::take(&mut self.buffered);
        body.truncate(length);
        while body.len() < length {
            self.stream.set_read_timeout(Some(self.deadline.checked_duration_since(Instant::now()).ok_or(std::io::ErrorKind::TimedOut)?))?;
            let mut buffer = [0; 4096];
            let remaining = (length - body.len()).min(buffer.len());
            let size = self.stream.read(&mut buffer[..remaining])?;
            if size == 0 { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
            body.extend_from_slice(&buffer[..size]);
        }
        Ok(body)
    }
    fn drain_body(&mut self) -> std::io::Result<()> {
        if self.body_read { return Ok(()); }
        self.body_read = true;
        let length = self.body_length().ok_or(std::io::ErrorKind::InvalidData)?;
        if length > 65536 { return Err(std::io::ErrorKind::InvalidData.into()); }
        let mut remaining = length.saturating_sub(self.buffered.len().min(length));
        self.buffered.clear();
        while remaining > 0 {
            self.stream.set_read_timeout(Some(self.deadline.checked_duration_since(Instant::now()).ok_or(std::io::ErrorKind::TimedOut)?))?;
            let mut buffer = [0; 4096];
            let read_length = remaining.min(buffer.len());
            let size = self.stream.read(&mut buffer[..read_length])?;
            if size == 0 { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
            remaining -= size;
        }
        Ok(())
    }
}

fn valid_depth(body: &[u8]) -> bool {
    let mut depth = 0usize; let mut string = false; let mut escaped = false;
    for byte in body {
        if string { if escaped { escaped = false; } else if *byte == b'\\' { escaped = true; } else if *byte == b'"' { string = false; } }
        else { match byte { b'"' => string = true, b'{' | b'[' => { depth += 1; if depth > 8 { return false; } }, b'}' | b']' => { let Some(next) = depth.checked_sub(1) else { return false }; depth = next; }, _ => {} } }
    }
    depth == 0 && !string
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEnvelope {
    spec: String,
    message_id: String,
    source_id: String,
    source_epoch: String,
    device_id: String,
    model_revision_id: String,
    binding_revision_id: String,
    points: Vec<WirePoint>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePoint {
    property: String,
    value: f64,
    source_at_ms: i64,
    source_time_origin: String,
    source_seq: String,
    quality: Quality,
    delivery: Delivery,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quality {
    state: String,
    #[serde(default)]
    source_protocol: Option<String>,
    #[serde(default)]
    source_code: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivery {
    replayed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    resource_ids: Vec<String>,
    range: TimeRange,
    mode: String,
    #[serde(deserialize_with = "Value::deserialize")]
    aggregation: Value,
    quality_states: Vec<String>,
    order: String,
    limit: usize,
    #[serde(deserialize_with = "Value::deserialize")]
    cursor: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRange {
    start_ms: i64,
    end_ms: i64,
}

struct Receipt {
    hash: String,
    id: String,
    status: &'static str,
}
struct State {
    db: Connection,
    receipts: HashMap<String, Receipt>,
    failed: bool,
    pending: usize,
}
struct Job {
    message: TelemetryEnvelope,
    received: i64,
    observation: String,
    hash: String,
    ingest_id: String,
}
type Shared = Arc<Mutex<State>>;
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn request_id() -> String {
    format!("req-{}-{}", now(), REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

fn source_granted(db: &Connection, source: &str, device: &str, checked_at: i64) -> duckdb::Result<bool> {
    db.query_row(
        "SELECT COUNT(*) FROM source_device_grant WHERE source_id = ? AND device_id = ? AND valid_from_ms <= ? AND ? < valid_to_ms",
        params![source, device, checked_at, checked_at],
        |row| Ok(row.get::<_, i64>(0)? == 1),
    )
}

fn token(name: &str) -> Result<String, &'static str> {
    let value = env::var(name).map_err(|_| "missing development token")?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid development token");
    }
    Ok(value)
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let bind = env::var("IOTFAST_DEV_BIND").unwrap_or_else(|_| "127.0.0.1:18080".into());
    let address: SocketAddrV4 = bind.parse().map_err(|_| "invalid bind address")?;
    if address.ip().octets() != [127, 0, 0, 1] || address.port() == 0 || bind != address.to_string()
    {
        return Err("invalid loopback bind".into());
    }
    let tokens = [
        token("IOTFAST_DEV_INGEST_TOKEN")?,
        token("IOTFAST_DEV_READER_TOKEN")?,
        token("IOTFAST_DEV_ALL_TOKEN")?,
    ];
    if tokens[0] == tokens[1] || tokens[0] == tokens[2] || tokens[1] == tokens[2] {
        return Err("tokens must be distinct".into());
    }
    let dir = env::var("IOTFAST_DEV_DATA_DIR").map_err(|_| "missing data directory")?;
    let path = Path::new(&dir);
    if !path.is_absolute() || !path.is_dir() || fs::read_dir(path)?.next().is_some() {
        return Err("data directory must be absolute and empty".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    let db = Connection::open(path.join("engine.duckdb"))?;
    let version: String = db.query_row("SELECT version()", [], |row| row.get(0))?;
    eprintln!("DuckDB SELECT version(): {version}");
    db.execute_batch("CREATE TABLE model_revision (model_revision_id VARCHAR PRIMARY KEY, model_version VARCHAR, status VARCHAR); INSERT INTO model_revision VALUES ('mr-demo-1','1.0.0','published'); CREATE TABLE device_binding (device_id VARCHAR PRIMARY KEY, resource_id VARCHAR, binding_revision_id VARCHAR, source_id VARCHAR, normalization_owner VARCHAR, valid_from_ms BIGINT); INSERT INTO device_binding VALUES ('demo-a','res-demo-a-temperature','br-demo-a-1','gateway-demo','gateway',1), ('demo-b','res-demo-b-temperature','br-demo-b-1','gateway-demo','gateway',1); CREATE TABLE observations (observation_id VARCHAR PRIMARY KEY, resource_id VARCHAR, device_id VARCHAR, source_at_ms BIGINT, received_at_ms BIGINT, source_seq UBIGINT, value DOUBLE, quality VARCHAR, source_quality_code VARCHAR, replayed BOOLEAN); CREATE TABLE latest AS SELECT * FROM observations WHERE false; CREATE TABLE committed_receipt (message_id VARCHAR PRIMARY KEY, ingest_id VARCHAR, payload_hash VARCHAR, history_status VARCHAR);")?;
    let started = now();
    db.execute_batch("CREATE TABLE source_device_grant (source_id VARCHAR NOT NULL, device_id VARCHAR NOT NULL, valid_from_ms BIGINT NOT NULL, valid_to_ms BIGINT NOT NULL, PRIMARY KEY (source_id, device_id));")?;
    for device in ["demo-a", "demo-b"] {
        db.execute("INSERT INTO source_device_grant VALUES (?, ?, ?, ?)", params!["gateway-demo", device, started, started + 3_600_000])?;
    }
    let state = Arc::new(Mutex::new(State {
        db,
        receipts: HashMap::new(),
        failed: false,
        pending: 0,
    }));
    let (sender, receiver) = mpsc::sync_channel::<Job>(256);
    let worker_state = state.clone();
    std::thread::spawn(move || {
        for job in receiver {
            commit(&worker_state, job);
        }
    });
    let server = TcpListener::bind(address)?;
    for stream in server.incoming() {
        let Ok(stream) = stream else { continue };
        let tokens = tokens.clone();
        let state = state.clone();
        let sender = sender.clone();
        std::thread::spawn(move || {
            if let Ok(request) = Request::read(stream) { serve(request, &tokens, started, &state, &sender); }
        });
    }
    Ok(())
}

fn commit(state: &Shared, job: Job) {
    let mut state = state.lock().unwrap();
    let point = &job.message.points[0];
    let resource = format!("res-{}-temperature", job.message.device_id);
    let transaction = state.db.transaction().and_then(|tx| {
        tx.execute("INSERT INTO observations VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)", params![job.observation, resource, job.message.device_id, point.source_at_ms, job.received, parse_source_seq(&point.source_seq).unwrap(), point.value.as_f64().unwrap(), point.quality.state, point.quality.source_code, point.delivery.replayed])?;
        tx.execute("DELETE FROM latest WHERE resource_id = ? AND (source_at_ms < ? OR (source_at_ms = ? AND source_seq < ?))", params![resource, point.source_at_ms, point.source_at_ms, parse_source_seq(&point.source_seq).unwrap()])?;
        tx.execute("INSERT INTO latest SELECT * FROM observations WHERE observation_id = ? AND NOT EXISTS (SELECT 1 FROM latest WHERE resource_id = ?)", params![job.observation, resource])?;
        tx.execute("INSERT INTO committed_receipt VALUES (?, ?, ?, 'committed')", params![job.message.message_id, job.ingest_id, job.hash])?;
        tx.commit()
    });
    let success = transaction.is_ok();
    state.pending -= 1;
    if !success {
        state.failed = true;
    }
    if let Some(receipt) = state.receipts.get_mut(&job.message.message_id) {
        receipt.status = if success { "committed" } else { "failed" };
    }
}

fn error(code: &str, status: u16, request_id: &str) -> (u16, Value) {
    let retry = status == 429 || status == 503;
    (
        status,
        json!({"request_id":request_id,"code":code,"message":code,"retryable":retry,"retry_after_ms":if retry {Some(1000)} else {None},"details":[]}),
    )
}

fn respond(mut request: Request, status: u16, body: Value) {
    let _ = request.drain_body();
    let body = body.to_string();
    let _ = write!(request.stream, "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", match status { 200 => "OK", 202 => "Accepted", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 409 => "Conflict", 422 => "Unprocessable Entity", 429 => "Too Many Requests", 501 => "Not Implemented", _ => "Service Unavailable" }, body.len());
}

fn serve(
    mut request: Request,
    tokens: &[String; 3],
    started: i64,
    state: &Shared,
    sender: &mpsc::SyncSender<Job>,
) {
    let path = request.path.clone();
    let method = request.method.clone();
    let generated_request_id = request_id();
    if method == "GET" && path == "/health" {
        return respond(
            request,
            200,
            json!({"status":"ok","profile":"orca-mvp-dev-v1","durability":"volatile"}),
        );
    }
    let headers = request.headers();
    let auth: Vec<_> = headers
        .iter()
        .filter(|header| header.field.equiv("Authorization"))
        .collect();
    let identity = if auth.len() == 1 && now() >= started && now() < started + 3_600_000 {
        auth[0]
            .value
            .as_str()
            .strip_prefix("Bearer ")
            .and_then(|secret| tokens.iter().position(|token| token == secret))
    } else {
        None
    };
    let Some(identity) = identity else {
        let (status, body) = error("UNAUTHENTICATED", 401, &generated_request_id);
        return respond(request, status, body);
    };
    let request_ids: Vec<_> = headers
        .iter()
        .filter(|header| header.field.equiv("X-Request-Id"))
        .collect();
    let request_id = if request_ids.is_empty() {
        generated_request_id.clone()
    } else if request_ids.len() == 1
        && (1..=128).contains(&request_ids[0].value.len())
        && request_ids[0]
            .value
            .as_bytes()
            .iter()
            .all(|byte| (0x21..=0x7e).contains(byte))
    {
        request_ids[0].value.to_string()
    } else {
        let (status, body) = error("VALIDATION_FAILED", 422, &generated_request_id);
        return respond(request, status, body);
    };
    if path.contains('?')
        || headers.iter().any(|header| {
            [
                "X-Tenant-Id",
                "X-Role",
                "X-Scope",
                "X-Caller-Id",
                "X-User-Id",
                "X-Allowed-Actions",
                "Forwarded",
                "X-Forwarded-For",
                "X-Forwarded-Host",
                "X-Forwarded-Proto",
            ]
            .iter()
            .any(|name| header.field.equiv(name))
        })
    {
        let (status, body) = error("VALIDATION_FAILED", 422, &request_id);
        return respond(request, status, body);
    }
    let is_telemetry = path == "/v1/telemetry" && method == "POST";
    let is_history = path == "/v1/history/query" && method == "POST";
    let view_device = path
        .strip_prefix("/v1/devices/")
        .and_then(|tail| tail.strip_suffix("/view"));
    if (is_telemetry && identity != 0) || ((is_history || view_device.is_some()) && identity == 0) {
        let (status, body) = error("FORBIDDEN_ACTION", 403, &request_id);
        return respond(request, status, body);
    }
    if is_telemetry {
        let preferences: Vec<_> = headers
            .iter()
            .filter(|header| header.field.equiv("Prefer"))
            .collect();
        if preferences.len() > 1
            || preferences.first().is_some_and(|value| {
                !["durability=volatile", "durability=durable"].contains(&value.value.as_str())
            })
        {
            let (status, body) = error("VALIDATION_FAILED", 422, &request_id);
            return respond(request, status, body);
        }
        if preferences
            .first()
            .is_some_and(|value| value.value.as_str() == "durability=durable")
        {
            let (status, body) = error("DURABILITY_BACKPRESSURE", 503, &request_id);
            return respond(request, status, body);
        }
    }
    if is_telemetry || is_history {
        let content_types: Vec<_> = headers
            .iter()
            .filter(|header| header.field.equiv("Content-Type"))
            .collect();
        let valid_type = content_types.len() == 1
            && ["application/json", "application/json; charset=utf-8"]
                .iter()
                .any(|item| content_types[0].value.as_str().eq_ignore_ascii_case(item));
        if !valid_type
            || headers
                .iter()
                .any(|header| header.field.equiv("Content-Encoding"))
            || request.body_length().is_none_or(|length| length > 65536)
        {
            let (status, body) = error("VALIDATION_FAILED", 422, &request_id);
            return respond(request, status, body);
        }
        let body = match request.body() { Ok(body) => body, Err(_) => {
            let (status, body) = error("VALIDATION_FAILED", 422, &request_id);
            return respond(request, status, body);
        }};
        if !valid_depth(&body) { let (status, body) = error("VALIDATION_FAILED", 422, &request_id); return respond(request, status, body); }
        let result = if is_telemetry {
            ingest(&body, state, sender, &request_id)
        } else {
            history(&body, state, identity, &request_id)
        };
        return respond(request, result.0, result.1);
    }
    if method == "GET" {
        if let Some(device) = view_device {
            let result = view(state, device, identity, &request_id);
            return respond(request, result.0, result.1);
        }
    }
    let known = path.starts_with("/v1/source-state")
        || path.starts_with("/v1/ingestions/")
        || path.ends_with("/shadow")
        || path.starts_with("/v1/control")
        || path.starts_with("/v1/subscriptions")
        || path == "/v1/telemetry"
        || path == "/v1/history/query"
        || view_device.is_some();
    let (status, body) = if known {
        error("CAPABILITY_UNSUPPORTED", 501, &request_id)
    } else {
        error("RESOURCE_NOT_FOUND", 404, &request_id)
    };
    respond(request, status, body);
}

fn ingest(
    body: &[u8],
    state: &Shared,
    sender: &mpsc::SyncSender<Job>,
    request_id: &str,
) -> (u16, Value) {
    let Ok(wire) = serde_json::from_slice::<WireEnvelope>(body) else {
        return error("VALIDATION_FAILED", 422, request_id);
    };
    {
        let locked = state.lock().unwrap();
        let checked_at = now();
        let grant = source_granted(&locked.db, &wire.source_id, &wire.device_id, checked_at);
        match grant {
            Ok(true) => {},
            Ok(false) => return error("SOURCE_NOT_AUTHORIZED", 403, request_id),
            Err(_) => return error("DURABILITY_BACKPRESSURE", 503, request_id),
        }
    }
    if wire.model_revision_id != "mr-demo-1"
        || wire.binding_revision_id != format!("br-{}-1", wire.device_id)
    {
        return error("REVISION_CONFLICT", 409, request_id);
    }
    if wire.spec != "iotfast.telemetry.v1"
        || !validate_message_id(&wire.message_id)
        || !validate_source_epoch(&wire.source_epoch)
        || !validate_entity_id(&wire.source_id)
        || wire.points.len() != 1
    {
        return error("VALIDATION_FAILED", 422, request_id);
    }
    if wire.source_epoch != "demo-epoch-1" {
        return error("CAPABILITY_UNSUPPORTED", 422, request_id);
    }
    let point = &wire.points[0];
    if point.property != "temperature"
        || !(-100.0..=1000.0).contains(&point.value)
        || !(1..=9_007_199_254_740_991).contains(&point.source_at_ms)
        || parse_source_seq(&point.source_seq).is_err()
        || !["good", "uncertain", "bad", "unknown"].contains(&point.quality.state.as_str())
        || [&point.quality.source_protocol, &point.quality.source_code]
            .iter()
            .any(|value| {
                value
                    .as_ref()
                    .is_some_and(|text| text.len() > 128 || !text.is_ascii())
            })
    {
        return error("VALIDATION_FAILED", 422, request_id);
    }
    if point.source_at_ms > now() + 60000 {
        return error("CLOCK_SKEW_EXCEEDED", 422, request_id);
    }
    if point.source_time_origin != "gateway_sample" {
        return if ["device_clock", "protocol_timestamp"]
            .contains(&point.source_time_origin.as_str())
        {
            error("CAPABILITY_UNSUPPORTED", 422, request_id)
        } else {
            error("VALIDATION_FAILED", 422, request_id)
        };
    }
    let message = TelemetryEnvelope {
        spec: wire.spec,
        message_id: wire.message_id,
        source_id: wire.source_id,
        source_epoch: wire.source_epoch,
        device_id: wire.device_id,
        model_revision_id: wire.model_revision_id,
        binding_revision_id: wire.binding_revision_id,
        points: vec![iotfast_thing_engine::d00::TelemetryPoint {
            property: point.property.clone(),
            value: json!(point.value),
            source_at_ms: point.source_at_ms,
            source_time_origin: point.source_time_origin.clone(),
            source_seq: point.source_seq.clone(),
            quality: iotfast_thing_engine::d00::TelemetryQuality {
                state: point.quality.state.clone(),
                source_protocol: point.quality.source_protocol.clone(),
                source_code: point.quality.source_code.clone(),
            },
            delivery: iotfast_thing_engine::d00::TelemetryDelivery {
                replayed: point.delivery.replayed,
            },
        }],
    };
    let hash = compute_telemetry_payload_hash(
        "ten-demo",
        &message,
        &BTreeMap::from([("temperature".into(), ValueType::Float64)]),
    )
    .unwrap();
    let observation = derive_observation_id(
        "ten-demo",
        &message.source_id,
        &message.source_epoch,
        &message.message_id,
        "temperature",
    )
    .unwrap();
    let mut locked = state.lock().unwrap();
    if let Some(receipt) = locked.receipts.get(&message.message_id) {
        if receipt.hash != hash {
            return error("IDEMPOTENCY_CONFLICT", 409, request_id);
        }
        return (
            200,
            json!({"request_id":request_id,"ingest_id":receipt.id,"message_id":message.message_id,"status":"duplicate","history_status":receipt.status}),
        );
    }
    if locked.failed {
        return error("DURABILITY_BACKPRESSURE", 503, request_id);
    }
    if locked.receipts.len() >= 10000 || locked.pending >= 256 {
        return error("RATE_LIMITED", 429, request_id);
    }
    let id = format!("ing-{}", locked.receipts.len() + 1);
    let message_id = message.message_id.clone();
    locked.receipts.insert(
        message_id.clone(),
        Receipt {
            hash: hash.clone(),
            id: id.clone(),
            status: "pending",
        },
    );
    locked.pending += 1;
    if sender
        .try_send(Job {
            message,
            received: now(),
            observation,
            hash,
            ingest_id: id.clone(),
        })
        .is_err()
    {
        locked.receipts.remove(&message_id);
        locked.pending -= 1;
        return error("RATE_LIMITED", 429, request_id);
    }
    (
        202,
        json!({"request_id":request_id,"ingest_id":id,"message_id":message_id,"status":"accepted_volatile","history_status":"pending"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_has_inclusive_start_exclusive_end_and_matches_source_and_device() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE source_device_grant (source_id VARCHAR, device_id VARCHAR, valid_from_ms BIGINT, valid_to_ms BIGINT); INSERT INTO source_device_grant VALUES ('gateway-demo', 'demo-a', 100, 200);").unwrap();
        for (time, expected) in [(99, false), (100, true), (199, true), (200, false)] {
            assert_eq!(source_granted(&db, "gateway-demo", "demo-a", time).unwrap(), expected);
        }
        assert!(!source_granted(&db, "other", "demo-a", 100).unwrap());
        assert!(!source_granted(&db, "gateway-demo", "demo-b", 100).unwrap());
    }

    #[test]
    fn failed_commit_rolls_back_and_stops_new_admission() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE source_device_grant (source_id VARCHAR, device_id VARCHAR, valid_from_ms BIGINT, valid_to_ms BIGINT); INSERT INTO source_device_grant VALUES ('gateway-demo', 'demo-a', 1, 9007199254740991); CREATE TABLE observations (observation_id VARCHAR PRIMARY KEY, resource_id VARCHAR, device_id VARCHAR, source_at_ms BIGINT, received_at_ms BIGINT, source_seq UBIGINT, value DOUBLE, quality VARCHAR, source_quality_code VARCHAR, replayed BOOLEAN); CREATE TABLE latest AS SELECT * FROM observations WHERE false; CREATE TABLE committed_receipt (message_id VARCHAR PRIMARY KEY, ingest_id VARCHAR, payload_hash VARCHAR, history_status VARCHAR); INSERT INTO committed_receipt VALUES ('failed-key','old','old','committed');").unwrap();
        let state = Arc::new(Mutex::new(State { db, receipts: HashMap::new(), failed: false, pending: 0 }));
        let (sender, receiver) = mpsc::sync_channel(256);
        let body = |id: &str| json!({"spec":"iotfast.telemetry.v1","message_id":id,"source_id":"gateway-demo","source_epoch":"demo-epoch-1","device_id":"demo-a","model_revision_id":"mr-demo-1","binding_revision_id":"br-demo-a-1","points":[{"property":"temperature","value":20.5,"source_at_ms":now(),"source_time_origin":"gateway_sample","source_seq":"1","quality":{"state":"bad","source_protocol":null,"source_code":null},"delivery":{"replayed":false}}]}).to_string();
        assert_eq!(ingest(body("failed-key").as_bytes(), &state, &sender, "req").0, 202);
        commit(&state, receiver.recv().unwrap());
        {
            let locked = state.lock().unwrap();
            assert!(locked.failed);
            assert_eq!(locked.receipts["failed-key"].status, "failed");
            assert_eq!(locked.db.query_row("SELECT COUNT(*) FROM observations", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(locked.db.query_row("SELECT COUNT(*) FROM latest", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        }
        assert_eq!(ingest(body("new-key").as_bytes(), &state, &sender, "req").0, 503);
        {
            let mut locked = state.lock().unwrap();
            locked.failed = false;
            locked.pending = 256;
        }
        assert_eq!(ingest(body("new-key").as_bytes(), &state, &sender, "req").0, 429);
    }
}

fn view(state: &Shared, device: &str, identity: usize, request_id: &str) -> (u16, Value) {
    if !["demo-a", "demo-b"].contains(&device) || (identity == 1 && device != "demo-a") {
        return error("RESOURCE_NOT_FOUND", 404, request_id);
    }
    let resource = format!("res-{device}-temperature");
    let binding = format!("br-{device}-1");
    let locked = state.lock().unwrap();
    let observation: Option<(f64, String, i64, i64)> = locked
        .db
        .query_row(
            "SELECT value, quality, source_at_ms, received_at_ms FROM latest WHERE resource_id = ?",
            params![resource],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    let (value, quality, sampled, received, reason, freshness, state_name) = match observation {
        Some((value, quality, sampled, received)) => (
            json!(value),
            quality,
            json!(sampled),
            json!(received),
            Value::Null,
            if now() - sampled > 30000 {
                "stale"
            } else {
                "fresh"
            },
            "observed",
        ),
        None => (
            Value::Null,
            "unknown".into(),
            Value::Null,
            Value::Null,
            json!("no_observation"),
            "unknown",
            "no_observation",
        ),
    };
    (
        200,
        json!({"request_id":request_id,"device_id":device,"device_key":device,"device_name":if device == "demo-a" {"演示设备 A"} else {"演示设备 B"},"product_id":"prod-demo","product_key":"demo-temperature","model_revision_id":"mr-demo-1","model_version":"1.0.0","asset_id":"asset-demo","canonical_device_uri":format!("uns://demo/lab/{device}"),"active_binding_revision_id":binding,"source_connection_state":"unknown","view_generated_at_ms":now(),"view_consistency":"per_resource_latest","properties":{"temperature":{"resource_id":resource,"name":"温度","data_type":"float64","unit":{"code":"Cel","symbol":"°C"},"binding_state":"bound","observation_state":state_name,"value":value,"quality":quality,"quality_reason":reason,"freshness":freshness,"value_sampled_at_ms":sampled,"quality_observed_at_ms":sampled,"received_at_ms":received,"source_time_origin":if state_name == "observed" {Some("gateway_sample")} else {None},"binding_revision_id":binding}}}),
    )
}

fn history(body: &[u8], state: &Shared, identity: usize, request_id: &str) -> (u16, Value) {
    let Ok(query) = serde_json::from_slice::<History>(body) else {
        return error("VALIDATION_FAILED", 422, request_id);
    };
    if query.resource_ids.is_empty() || query.resource_ids.len() > 2 { return error("VALIDATION_FAILED", 422, request_id); }
    if query.resource_ids.iter().any(|resource| {
            !["res-demo-a-temperature", "res-demo-b-temperature"].contains(&resource.as_str())
                || (identity == 1 && resource != "res-demo-a-temperature")
        })
    {
        return error("RESOURCE_NOT_FOUND", 404, request_id);
    }
    if query.mode != "raw" || !query.aggregation.is_null() || !query.cursor.is_null() {
        return error("CAPABILITY_UNSUPPORTED", 422, request_id);
    }
    if query.resource_ids.len() == 2 && query.resource_ids[0] == query.resource_ids[1]
        || query.range.start_ms < 1
        || query.range.end_ms > 9_007_199_254_740_991
        || query.range.end_ms <= query.range.start_ms
        || query.range.end_ms - query.range.start_ms > 3_600_000
        || !(1..=1000).contains(&query.limit)
        || !["asc", "desc"].contains(&query.order.as_str())
        || query.quality_states.is_empty()
        || query
            .quality_states
            .iter()
            .any(|quality| !["good", "uncertain", "bad", "unknown"].contains(&quality.as_str()))
        || query
            .quality_states
            .iter()
            .enumerate()
            .any(|(index, quality)| query.quality_states[..index].contains(quality))
    {
        return error("VALIDATION_FAILED", 422, request_id);
    }
    let mut locked = state.lock().unwrap();
    let Ok(tx) = locked.db.transaction() else {
        return error("DURABILITY_BACKPRESSURE", 503, request_id);
    };
    let direction = if query.order == "asc" { "ASC" } else { "DESC" };
    let sql = format!("SELECT observation_id, resource_id, device_id, source_at_ms, received_at_ms, value, quality, source_quality_code, replayed FROM observations WHERE source_at_ms >= ? AND source_at_ms < ? AND (resource_id = ? OR resource_id = ?) AND (quality = ? OR quality = ? OR quality = ? OR quality = ?) ORDER BY source_at_ms {direction}, observation_id {direction} LIMIT ?");
    let mut statement = match tx.prepare(&sql) {
        Ok(value) => value,
        Err(_) => return error("DURABILITY_BACKPRESSURE", 503, request_id),
    };
    let resource_a = &query.resource_ids[0];
    let resource_b = query.resource_ids.get(1).unwrap_or(resource_a);
    let quality = |index| {
        query
            .quality_states
            .get(index)
            .unwrap_or(&query.quality_states[0])
    };
    let mut rows = match statement.query(params![
        query.range.start_ms,
        query.range.end_ms,
        resource_a,
        resource_b,
        quality(0),
        quality(1),
        quality(2),
        quality(3),
        (query.limit + 1) as i64
    ]) {
        Ok(value) => value,
        Err(_) => return error("DURABILITY_BACKPRESSURE", 503, request_id),
    };
    let mut items = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let resource: String = row.get(1).unwrap();
        let quality: String = row.get(6).unwrap();
        if !query.resource_ids.contains(&resource) || !query.quality_states.contains(&quality) {
            continue;
        }
        let device: String = row.get(2).unwrap();
        items.push(json!({"observation_id":row.get::<_, String>(0).unwrap(),"resource_id":resource,"device_id":device,"property":"temperature","model_revision_id":"mr-demo-1","binding_revision_id":format!("br-{device}-1"),"source_at_ms":row.get::<_, i64>(3).unwrap(),"received_at_ms":row.get::<_, i64>(4).unwrap(),"source_time_origin":"gateway_sample","value_type":"float64","value":row.get::<_, f64>(5).unwrap(),"unit":{"code":"Cel","symbol":"°C"},"quality":quality,"source_quality_code":row.get::<_, Option<String>>(7).unwrap(),"replayed":row.get::<_, bool>(8).unwrap(),"record_kind":"measured"}));
        if items.len() > query.limit {
            let mut result = error("VALIDATION_FAILED", 422, request_id);
            result.1["details"] = json!([{"field":"limit","issue":"RESULT_LIMIT_EXCEEDED","description":"Reduce query range"}]);
            return result;
        }
    }
    drop(rows);
    drop(statement);
    if tx.commit().is_err() {
        return error("DURABILITY_BACKPRESSURE", 503, request_id);
    }
    (
        200,
        json!({"request_id":request_id,"target_id":"local-duckdb","source_mode":"managed_ingest","mode":"raw","consistency":"strong_snapshot","partial":false,"warnings":[],"coverage":{"contiguous_wal_position":null,"event_time_complete_through_ms":null,"event_time_completeness":"unknown","known_gaps":[]},"items":items,"next_cursor":null}),
    )
}
