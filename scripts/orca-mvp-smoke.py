#!/usr/bin/env python3
import base64
import ctypes
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("IOTFAST_DEV_SERVER_BIN", ROOT / "prototype/target/debug/dev_server"))
RESULTS = []
DURABLE_TRACES = []
RESET_TRACES = []
PROCESSES = []
TEMP_ROOT = Path(tempfile.mkdtemp(prefix="iotfast-m2-", dir="/tmp"))
TOKENS = [__import__("secrets").token_hex(32) for _ in range(3)]
SUCCESS = False


def record(case, passed, detail):
    RESULTS.append({"case": case, "passed": bool(passed), "detail": detail})
    print(f"{'PASS' if passed else 'FAIL'} {case}: {detail}", flush=True)


def require(case, condition, detail):
    record(case, condition, detail)
    if not condition:
        raise AssertionError(f"{case}: {detail}")


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def duckdb_library():
    configured = os.environ.get("IOTFAST_DUCKDB_LIBRARY")
    if configured:
        path = Path(configured).resolve()
        if not path.is_file():
            raise RuntimeError(f"DuckDB library does not exist: {path}")
        return path
    suffix = "dylib" if sys.platform == "darwin" else "so"
    candidates = sorted((ROOT / "prototype/target/duckdb-download").glob(f"*/*/libduckdb.{suffix}"))
    if len(candidates) != 1:
        raise RuntimeError("set IOTFAST_DUCKDB_LIBRARY to the DuckDB library used by dev_server")
    return candidates[0]


LIBRARY = duckdb_library()


def environment(data_dir, port, tokens=None):
    values = TOKENS if tokens is None else tokens
    env = os.environ.copy()
    env.update({
        "IOTFAST_DEV_DATA_DIR": str(data_dir),
        "IOTFAST_DEV_BIND": f"127.0.0.1:{port}",
        "IOTFAST_DEV_INGEST_TOKEN": values[0],
        "IOTFAST_DEV_READER_TOKEN": values[1],
        "IOTFAST_DEV_ALL_TOKEN": values[2],
    })
    key = "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH"
    env[key] = str(LIBRARY.parent) + (os.pathsep + env[key] if env.get(key) else "")
    return env


def start_server(data_dir, port, env=None):
    stderr_path = TEMP_ROOT / f"server-{port}.stderr.log"
    stderr = stderr_path.open("w")
    process = subprocess.Popen(
        [str(BINARY)],
        cwd=ROOT,
        env=env or environment(data_dir, port),
        stdout=subprocess.DEVNULL,
        stderr=stderr,
        start_new_session=True,
    )
    PROCESSES.append((process, stderr, stderr_path))
    return process, stderr_path


def stop_servers():
    for process, _, _ in reversed(PROCESSES):
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
    for process, stderr, _ in reversed(PROCESSES):
        if process.poll() is None:
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
        stderr.close()


def wait_ready(process, port, stderr_path):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"dev_server exited {process.returncode}: {stderr_path.read_text()}")
        try:
            status, body, _ = request(port, "GET", "/health")
            if status == 200:
                return body
        except OSError:
            pass
        time.sleep(0.05)
    raise AssertionError(f"dev_server not ready on loopback port {port}")


def _request_once(port, method, path, token=None, body=None, headers=None, raw_length=None, trace=None):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    connection.putrequest(method, path)
    payload = body
    if isinstance(body, (dict, list)):
        payload = json.dumps(body, ensure_ascii=False, separators=(",", ":")).encode()
    if isinstance(payload, str):
        payload = payload.encode()
    all_headers = list(headers or [])
    if token is not None:
        all_headers.append(("Authorization", f"Bearer {token}"))
    if payload is not None and not any(name.lower() == "content-type" for name, _ in all_headers):
        all_headers.append(("Content-Type", "application/json"))
    for name, value in all_headers:
        connection.putheader(name, value)
    if raw_length is not None:
        connection.putheader("Content-Length", str(raw_length))
        payload = None
    elif payload is not None:
        connection.putheader("Content-Length", str(len(payload)))
    connection.endheaders(payload)
    if trace is not None:
        trace["body_fully_sent"] = True
    response = connection.getresponse()
    data = response.read()
    response_headers = dict(response.getheaders())
    status = response.status
    connection.close()
    try:
        parsed = json.loads(data)
    except (json.JSONDecodeError, UnicodeDecodeError):
        parsed = {"_non_json": data.decode(errors="replace")}
    return status, parsed, response_headers


class ResetResponse:
    def __init__(self, error, args, kwargs):
        self.error = error
        self.args = args
        self.kwargs = kwargs

    def __iter__(self):
        raise self.error


def request(*args, **kwargs):
    try:
        return _request_once(*args, **kwargs)
    except ConnectionResetError as error:
        return ResetResponse(error, args, kwargs)


def resolve_error_response(response):
    if not isinstance(response, ResetResponse):
        return response
    args = response.args
    kwargs = response.kwargs
    headers = kwargs.get("headers") or (args[5] if len(args) > 5 else []) or []
    request_id = next((value for name, value in headers if name.lower() == "x-request-id"), None)
    trace = {"method": args[1], "path": args[2], "request_id": request_id, "attempts": []}
    current = response
    for attempt in range(1, 3):
        if attempt > 1:
            try:
                current = _request_once(*args, **kwargs)
            except ConnectionResetError as error:
                current = ResetResponse(error, args, kwargs)
        if isinstance(current, ResetResponse):
            trace["attempts"].append({"attempt": attempt, "exception": f"{type(current.error).__name__}: {current.error}"})
            continue
        status, body, response_headers = current
        trace["attempts"].append({"attempt": attempt, "actual": {"status": status, "headers": response_headers, "body": body}})
        RESET_TRACES.append(trace)
        print("RESET TRACE " + json.dumps(trace, ensure_ascii=False, sort_keys=True), flush=True)
        return current
    RESET_TRACES.append(trace)
    print("RESET TRACE " + json.dumps(trace, ensure_ascii=False, sort_keys=True), flush=True)
    raise current.error


def durable_post(port, payload):
    trace = {
        "request_id": "m2-durable-request",
        "method": "POST",
        "path": "/v1/telemetry",
        "body_fully_sent": False,
        "expected": {"status": 503, "code": "DURABILITY_BACKPRESSURE"},
    }
    started = time.monotonic()
    try:
        response = resolve_error_response(request(
            port,
            "POST",
            "/v1/telemetry",
            TOKENS[0],
            payload,
            headers=[("Prefer", "durability=durable"), ("X-Request-Id", trace["request_id"])],
            trace=trace,
        ))
        status, body, headers = response
        trace["actual"] = {"status": status, "headers": headers, "body": body}
        return response
    except Exception as error:
        trace["exception"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        trace["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)
        DURABLE_TRACES.append(trace)
        print("DURABLE TRACE " + json.dumps(trace, ensure_ascii=False, sort_keys=True), flush=True)


def expect_error(case, response, status, code, retryable=False):
    response = resolve_error_response(response)
    actual_status, body, _ = response
    required = {"request_id", "code", "message", "retryable", "retry_after_ms", "details"}
    condition = (
        actual_status == status
        and required <= body.keys()
        and body.get("code") == code
        and body.get("retryable") is retryable
        and body.get("retry_after_ms") == (1000 if retryable else None)
        and isinstance(body.get("details"), list)
        and all(token not in json.dumps(body) for token in TOKENS)
    )
    require(case, condition, f"status={actual_status}, body={body}")


def message(device, identifier, value, sampled, seq="1", quality="good", replayed=False):
    return {
        "spec": "iotfast.telemetry.v1",
        "message_id": identifier,
        "source_id": "gateway-demo",
        "source_epoch": "demo-epoch-1",
        "device_id": device,
        "model_revision_id": "mr-demo-1",
        "binding_revision_id": f"br-{device}-1",
        "points": [{
            "property": "temperature",
            "value": value,
            "source_at_ms": sampled,
            "source_time_origin": "gateway_sample",
            "source_seq": seq,
            "quality": {"state": quality, "source_protocol": "modbus", "source_code": None},
            "delivery": {"replayed": replayed},
        }],
    }


def observation_id(identifier, prop="temperature"):
    material = b"iotfast.obs.v1\0ten-demo\0gateway-demo\0demo-epoch-1\0" + identifier.encode() + b"\0" + prop.encode()
    return "obs-" + base64.b32encode(hashlib.sha256(material).digest()[:16]).decode().lower().rstrip("=")


def payload_hash(payload):
    point = payload["points"][0]
    canonical = {
        "spec": payload["spec"],
        "tenant_id": "ten-demo",
        "source_id": payload["source_id"],
        "source_epoch": payload["source_epoch"],
        "device_id": payload["device_id"],
        "model_revision_id": payload["model_revision_id"],
        "binding_revision_id": payload["binding_revision_id"],
        "points": [{
            "property": point["property"],
            "value_type": "float64",
            "value": point["value"],
            "source_at_ms": point["source_at_ms"],
            "source_time_origin": point["source_time_origin"],
            "source_seq": point["source_seq"],
            "quality": {
                "state": point["quality"]["state"],
                "source_protocol": point["quality"].get("source_protocol"),
                "source_code": point["quality"].get("source_code"),
            },
        }],
    }
    encoded = json.dumps(canonical, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def query(resource_ids, start, end, limit=1000, quality=None, order="asc"):
    return {
        "resource_ids": resource_ids,
        "range": {"start_ms": start, "end_ms": end},
        "mode": "raw",
        "aggregation": None,
        "quality_states": quality or ["good", "uncertain", "bad", "unknown"],
        "order": order,
        "limit": limit,
        "cursor": None,
    }


def wait_history(port, expected, start, end):
    start = max(start, end - 3_600_000)
    request_body = query(["res-demo-a-temperature", "res-demo-b-temperature"], start, end)
    for _ in range(50):
        status, body, _ = request(port, "POST", "/v1/history/query", TOKENS[2], request_body)
        if status == 200 and len(body["items"]) == expected:
            return body
        time.sleep(0.1)
    raise AssertionError(f"history did not reach {expected} rows: status={status}, body={body}")


def view(port, device, token=None):
    return request(port, "GET", f"/v1/devices/{device}/view", token or TOKENS[2])


def startup_rejection(name, mutate, nonempty=False):
    case_dir = TEMP_ROOT / name
    case_dir.mkdir()
    marker = None
    if nonempty:
        marker = case_dir / "keep.txt"
        marker.write_text("preserve")
    port = free_port()
    env = environment(case_dir, port)
    mutate(env)
    result = subprocess.run([str(BINARY)], cwd=ROOT, env=env, capture_output=True, text=True, timeout=10)
    require(f"M01 {name} exit", result.returncode != 0, f"exit={result.returncode}")
    bind_port = int(env["IOTFAST_DEV_BIND"].rsplit(":", 1)[1])
    with socket.socket() as sock:
        sock.settimeout(0.2)
        require(f"M01 {name} no listener", sock.connect_ex(("127.0.0.1", bind_port)) != 0, f"port={bind_port}")
    if marker is not None:
        require(f"M01 {name} preserved", marker.read_text() == "preserve", str(marker))


class DuckDB:
    class Result(ctypes.Structure):
        _fields_ = [("columns", ctypes.c_uint64), ("rows", ctypes.c_uint64), ("changed", ctypes.c_uint64),
                    ("internal_columns", ctypes.c_void_p), ("error", ctypes.c_void_p)]

    def __init__(self, library, database):
        self.lib = ctypes.CDLL(str(library))
        handle = ctypes.c_void_p
        self.lib.duckdb_create_config.argtypes = [ctypes.POINTER(handle)]
        self.lib.duckdb_set_config.argtypes = [handle, ctypes.c_char_p, ctypes.c_char_p]
        self.lib.duckdb_destroy_config.argtypes = [ctypes.POINTER(handle)]
        self.lib.duckdb_open_ext.argtypes = [ctypes.c_char_p, ctypes.POINTER(handle), handle, ctypes.POINTER(ctypes.c_void_p)]
        self.lib.duckdb_close.argtypes = [ctypes.POINTER(handle)]
        self.lib.duckdb_connect.argtypes = [handle, ctypes.POINTER(handle)]
        self.lib.duckdb_disconnect.argtypes = [ctypes.POINTER(handle)]
        self.lib.duckdb_query.argtypes = [handle, ctypes.c_char_p, ctypes.POINTER(self.Result)]
        self.lib.duckdb_destroy_result.argtypes = [ctypes.POINTER(self.Result)]
        self.lib.duckdb_row_count.argtypes = [ctypes.POINTER(self.Result)]
        self.lib.duckdb_row_count.restype = ctypes.c_uint64
        self.lib.duckdb_column_count.argtypes = [ctypes.POINTER(self.Result)]
        self.lib.duckdb_column_count.restype = ctypes.c_uint64
        self.lib.duckdb_value_varchar.argtypes = [ctypes.POINTER(self.Result), ctypes.c_uint64, ctypes.c_uint64]
        self.lib.duckdb_value_varchar.restype = ctypes.c_void_p
        self.lib.duckdb_free.argtypes = [ctypes.c_void_p]
        config = handle()
        self.check(self.lib.duckdb_create_config(ctypes.byref(config)), "create config")
        self.check(self.lib.duckdb_set_config(config, b"access_mode", b"READ_ONLY"), "set READ_ONLY")
        self.db = handle()
        error = ctypes.c_void_p()
        state = self.lib.duckdb_open_ext(str(database).encode(), ctypes.byref(self.db), config, ctypes.byref(error))
        self.lib.duckdb_destroy_config(ctypes.byref(config))
        if error.value:
            message_text = ctypes.string_at(error.value).decode(errors="replace")
            self.lib.duckdb_free(error)
        else:
            message_text = ""
        self.check(state, f"open READ_ONLY: {message_text}")
        self.connection = handle()
        self.check(self.lib.duckdb_connect(self.db, ctypes.byref(self.connection)), "connect")

    @staticmethod
    def check(state, operation):
        if state != 0:
            raise AssertionError(f"DuckDB {operation} failed with state {state}")

    def rows(self, sql):
        result = self.Result()
        self.check(self.lib.duckdb_query(self.connection, sql.encode(), ctypes.byref(result)), sql)
        try:
            rows = []
            for row in range(self.lib.duckdb_row_count(ctypes.byref(result))):
                values = []
                for column in range(self.lib.duckdb_column_count(ctypes.byref(result))):
                    pointer = self.lib.duckdb_value_varchar(ctypes.byref(result), column, row)
                    values.append(ctypes.string_at(pointer).decode() if pointer else None)
                    if pointer:
                        self.lib.duckdb_free(pointer)
                rows.append(values)
            return rows
        finally:
            self.lib.duckdb_destroy_result(ctypes.byref(result))

    def close(self):
        self.lib.duckdb_disconnect(ctypes.byref(self.connection))
        self.lib.duckdb_close(ctypes.byref(self.db))


def main():
    require("environment binary", BINARY.is_file(), str(BINARY))
    require("environment DuckDB library", LIBRARY.is_file(), str(LIBRARY))
    print(f"INFO temp_root={TEMP_ROOT}")
    print(f"INFO duckdb_library={LIBRARY}")
    startup_rejection("non-loopback", lambda env: env.update(IOTFAST_DEV_BIND=f"0.0.0.0:{free_port()}"))
    startup_rejection("missing-token", lambda env: env.pop("IOTFAST_DEV_READER_TOKEN"))
    startup_rejection("invalid-token", lambda env: env.update(IOTFAST_DEV_INGEST_TOKEN="a" * 63))
    startup_rejection("same-tokens", lambda env: env.update(IOTFAST_DEV_READER_TOKEN=env["IOTFAST_DEV_INGEST_TOKEN"]))
    startup_rejection("nonempty-dir", lambda env: None, nonempty=True)

    data_dir = TEMP_ROOT / "session"
    data_dir.mkdir()
    port = free_port()
    process, stderr_path = start_server(data_dir, port)
    health = wait_ready(process, port, stderr_path)
    require("M01 health", health == {"status": "ok", "profile": "orca-mvp-dev-v1", "durability": "volatile"}, str(health))
    status, identified, _ = request(port, "GET", "/v1/devices/demo-a/view", TOKENS[1], headers=[("X-Request-Id", "m2-view-request")])
    require("M02 request ID echo", status == 200 and identified["request_id"] == "m2-view-request", str(identified["request_id"]))
    expect_error("M02 invalid request ID", request(port, "GET", "/v1/devices/demo-a/view", TOKENS[1], headers=[("X-Request-Id", "bad request")]), 422, "VALIDATION_FAILED")

    for name, token, headers in [
        ("missing", None, []),
        ("wrong", "0" * 64, []),
        ("forged-only", None, [("X-Tenant-Id", "ten-demo"), ("X-Role", "admin")]),
    ]:
        expect_error(f"M02 {name} credential", request(port, "GET", "/v1/devices/demo-a/view", token, headers=headers), 401, "UNAUTHENTICATED")
    expect_error("M02 multiple credentials", request(
        port, "GET", "/v1/devices/demo-a/view", headers=[("Authorization", f"Bearer {TOKENS[1]}"), ("Authorization", f"Bearer {TOKENS[2]}")]
    ), 401, "UNAUTHENTICATED")

    for name, response in [
        ("forged header", request(port, "GET", "/v1/devices/demo-a/view", TOKENS[1], headers=[("X-Scope", "*")])) ,
        ("query scope", request(port, "GET", "/v1/devices/demo-a/view?tenant_id=ten-demo", TOKENS[1])),
        ("body scope", request(port, "POST", "/v1/telemetry", TOKENS[0], dict(message("demo-a", "m2-forged-body", 1, int(time.time() * 1000)), scope={"resource_ids": ["*"]}))),
    ]:
        expect_error(f"M03 {name}", response, 422, "VALIDATION_FAILED")
    expect_error("M03 reader device B", view(port, "demo-b", TOKENS[1]), 404, "RESOURCE_NOT_FOUND")
    expect_error("M03 mixed history", request(port, "POST", "/v1/history/query", TOKENS[1], query(["res-demo-a-temperature", "res-demo-b-temperature"], 1, 2)), 404, "RESOURCE_NOT_FOUND")
    expect_error("M03 gateway read", view(port, "demo-a", TOKENS[0]), 403, "FORBIDDEN_ACTION")
    expect_error("M03 reader write", request(port, "POST", "/v1/telemetry", TOKENS[1], message("demo-a", "m2-reader-write", 1, int(time.time() * 1000))), 403, "FORBIDDEN_ACTION")

    for device in ["demo-a", "demo-b"]:
        status, body, _ = view(port, device)
        point = body["properties"]["temperature"]
        require(f"M04 initial {device}", status == 200 and point["value"] is None and point["quality"] == "unknown"
                and point["observation_state"] == "no_observation" and point["value_sampled_at_ms"] is None
                and point["binding_revision_id"] == f"br-{device}-1", str(point))

    now = int(time.time() * 1000)
    stale_sample = now - 31000
    fresh_sample = now - 1000
    accepted = []

    def ingest(payload):
        response = request(port, "POST", "/v1/telemetry", TOKENS[0], payload)
        status, body, _ = response
        require(f"M09 accept {payload['message_id']}", status == 202 and body["status"] == "accepted_volatile"
                and body["history_status"] == "pending" and set(body) == {"request_id", "ingest_id", "message_id", "status", "history_status"}, str(body))
        accepted.append(payload)
        return response

    stale = message("demo-a", "m2-stale", 240.0, stale_sample, "10")
    expect_error("M09 durable rejected", durable_post(port, stale), 503, "DURABILITY_BACKPRESSURE", True)
    status, body, _ = view(port, "demo-a")
    require("M09 durable no latest side effect", body["properties"]["temperature"]["value"] is None, str(body))
    ingest(stale)
    wait_history(port, 1, 1, now + 1000)
    status, stale_view, _ = view(port, "demo-a")
    stale_point = stale_view["properties"]["temperature"]
    require("M07 stale good", stale_point["quality"] == "good" and stale_point["freshness"] == "stale"
            and stale_point["value_sampled_at_ms"] == stale_sample and stale_point["quality_observed_at_ms"] == stale_sample, str(stale_point))
    status, stale_view_again, _ = view(port, "demo-a")
    require("M07 GET preserves sample time", stale_view_again["properties"]["temperature"]["value_sampled_at_ms"] == stale_sample, str(stale_view_again))

    first = message("demo-a", "m2-a-main", 245.6, fresh_sample, "1000")
    _, first_ack, _ = ingest(first)
    wait_history(port, 2, 1, now + 1000)
    replay = message("demo-a", "m2-a-main", 245.6, fresh_sample, "1000", replayed=True)
    status, duplicate, _ = request(port, "POST", "/v1/telemetry", TOKENS[0], replay)
    require("M08 replay duplicate", status == 200 and duplicate["status"] == "duplicate" and duplicate["ingest_id"] == first_ack["ingest_id"]
            and duplicate["history_status"] == "committed", str(duplicate))
    expect_error("M08 idempotency conflict", request(port, "POST", "/v1/telemetry", TOKENS[0], message("demo-a", "m2-a-main", 245.7, fresh_sample, "1000")), 409, "IDEMPOTENCY_CONFLICT")

    second = message("demo-b", "m2-b-main", 31.2, fresh_sample, "1000")
    ingest(second)
    wait_history(port, 3, 1, now + 1000)
    for device, expected in [("demo-a", 245.6), ("demo-b", 31.2)]:
        status, body, _ = view(port, device)
        point = body["properties"]["temperature"]
        require(f"M04 engineering {device}", status == 200 and point["value"] == expected and point["freshness"] == "fresh"
                and point["quality"] == "good" and point["unit"] == {"code": "Cel", "symbol": "°C"}
                and point["value_sampled_at_ms"] == fresh_sample and body["model_revision_id"] == "mr-demo-1"
                and body["model_version"] == "1.0.0" and body["active_binding_revision_id"] == f"br-{device}-1", str(point))

    bad = message("demo-b", "m2-b-bad", 31.3, fresh_sample + 1, "1001", "bad")
    ingest(bad)
    wait_history(port, 4, 1, now + 1000)
    _, bad_view, _ = view(port, "demo-b")
    require("M07 bad fresh", bad_view["properties"]["temperature"]["quality"] == "bad"
            and bad_view["properties"]["temperature"]["freshness"] == "fresh"
            and bad_view["properties"]["temperature"]["value"] == 31.3, str(bad_view["properties"]["temperature"]))

    older = message("demo-a", "m2-a-older", 244.0, fresh_sample - 1, "1002")
    same_999 = message("demo-a", "m2-a-seq999", 244.9, fresh_sample, "999")
    same_1001 = message("demo-a", "m2-a-seq1001", 246.0, fresh_sample, "1001")
    final = message("demo-a", "m2-a-final", 245.6, fresh_sample, "1002")
    for payload in [older, same_999, same_1001, final]:
        ingest(payload)
    wait_history(port, 8, 1, now + 1000)
    _, ordered_view, _ = view(port, "demo-a")
    require("M07 out-of-order and numeric seq", ordered_view["properties"]["temperature"]["value"] == 245.6
            and ordered_view["properties"]["temperature"]["value_sampled_at_ms"] == fresh_sample, str(ordered_view["properties"]["temperature"]))

    invalid_binding = message("demo-b", "m2-wrong-binding", 1, now)
    invalid_binding["binding_revision_id"] = "br-demo-a-1"
    expect_error("M06 wrong binding", request(port, "POST", "/v1/telemetry", TOKENS[0], invalid_binding), 409, "REVISION_CONFLICT")
    invalid_model = message("demo-a", "m2-wrong-model", 1, now)
    invalid_model["model_revision_id"] = "mr-other"
    expect_error("M06 wrong model", request(port, "POST", "/v1/telemetry", TOKENS[0], invalid_model), 409, "REVISION_CONFLICT")
    wrong_source = message("demo-a", "m2-wrong-source", 1, now)
    wrong_source["source_id"] = "gateway-other"
    expect_error("M06 wrong source", request(port, "POST", "/v1/telemetry", TOKENS[0], wrong_source), 403, "SOURCE_NOT_AUTHORIZED")
    unsupported_epoch = message("demo-a", "m2-wrong-epoch", 1, now)
    unsupported_epoch["source_epoch"] = "demo-epoch-2"
    expect_error("M06 unsupported epoch", request(port, "POST", "/v1/telemetry", TOKENS[0], unsupported_epoch), 422, "CAPABILITY_UNSUPPORTED")
    unsupported_origin = message("demo-a", "m2-origin", 1, now)
    unsupported_origin["points"][0]["source_time_origin"] = "device_clock"
    expect_error("M06 unsupported origin", request(port, "POST", "/v1/telemetry", TOKENS[0], unsupported_origin), 422, "CAPABILITY_UNSUPPORTED")
    future = message("demo-a", "m2-future", 1, int(time.time() * 1000) + 120000)
    expect_error("M07 future skew", request(port, "POST", "/v1/telemetry", TOKENS[0], future), 422, "CLOCK_SKEW_EXCEEDED")

    atomic = message("demo-a", "m2-atomic", 10, now)
    atomic["points"].append(dict(atomic["points"][0], property="unknown"))
    expect_error("M05 mixed invalid batch", request(port, "POST", "/v1/telemetry", TOKENS[0], atomic), 422, "VALIDATION_FAILED")
    wait_history(port, 8, 1, now + 1000)
    corrected = message("demo-a", "m2-atomic", 10, now)
    ingest(corrected)
    wait_history(port, 9, 1, now + 1000)

    strict_cases = []
    duplicate_point = message("demo-a", "m2-duplicate-point", 1, now)
    duplicate_point["points"] *= 2
    strict_cases.append(("duplicate property", duplicate_point, None))
    for name, mutate in [
        ("null value", lambda body: body["points"][0].update(value=None)),
        ("Rust enum value", lambda body: body["points"][0].update(value={"Float64": 1})),
        ("string value", lambda body: body["points"][0].update(value="1")),
        ("out of range", lambda body: body["points"][0].update(value=1001)),
        ("unknown top field", lambda body: body.update(tenant_id="ten-demo")),
        ("unknown spec", lambda body: body.update(spec="other")),
        ("numeric seq", lambda body: body["points"][0].update(source_seq=1)),
        ("leading zero seq", lambda body: body["points"][0].update(source_seq="01")),
        ("overflow seq", lambda body: body["points"][0].update(source_seq="18446744073709551616")),
    ]:
        body = message("demo-a", f"m2-{name.replace(' ', '-')}", 1, now)
        mutate(body)
        strict_cases.append((name, body, None))
    strict_cases.extend([
        ("empty body", b"", None),
        ("invalid JSON", b"{", None),
        ("wrong content type", json.dumps(message("demo-a", "m2-content-type", 1, now)).encode(), [("Content-Type", "text/plain")]),
        ("too deep", b'{"spec":[[[[[[[[1]]]]]]]]}', None),
        ("declared oversized", None, [("Content-Type", "application/json")]),
    ])
    for name, body, headers in strict_cases:
        response = request(port, "POST", "/v1/telemetry", TOKENS[0], body, headers=headers, raw_length=70000 if name == "declared oversized" else None)
        expect_error(f"M05 {name}", response, 422, "VALIDATION_FAILED")
    wait_history(port, 9, 1, now + 1000)

    all_resources = ["res-demo-a-temperature", "res-demo-b-temperature"]
    history_start = stale_sample - 1
    history_end = now + 1000
    history = wait_history(port, 9, 1, now + 1000)
    require("M10 raw response", history["target_id"] == "local-duckdb" and history["source_mode"] == "managed_ingest"
            and history["mode"] == "raw" and history["consistency"] == "strong_snapshot" and history["partial"] is False
            and history["next_cursor"] is None and history["coverage"]["event_time_completeness"] == "unknown", str(history))
    expected_ids = {observation_id(item["message_id"]) for item in accepted}
    require("M08 observation golden IDs", {item["observation_id"] for item in history["items"]} == expected_ids,
            f"expected={sorted(expected_ids)}")
    require("M10 history fields", all({"observation_id", "resource_id", "device_id", "property", "model_revision_id",
            "binding_revision_id", "source_at_ms", "received_at_ms", "source_time_origin", "value_type", "value", "unit",
            "quality", "source_quality_code", "replayed", "record_kind"} <= item.keys() for item in history["items"]), "all rows")
    asc_keys = [(item["source_at_ms"], item["observation_id"]) for item in history["items"]]
    require("M10 ascending stable order", asc_keys == sorted(asc_keys), str(asc_keys))
    status, desc, _ = request(port, "POST", "/v1/history/query", TOKENS[2], query(all_resources, history_start, history_end, order="desc"))
    desc_keys = [(item["source_at_ms"], item["observation_id"]) for item in desc["items"]]
    require("M10 descending stable order", status == 200 and desc_keys == sorted(desc_keys, reverse=True), str(desc_keys))
    status, boundary, _ = request(port, "POST", "/v1/history/query", TOKENS[2], query(["res-demo-b-temperature"], fresh_sample, fresh_sample + 1))
    require("M10 half-open range", status == 200 and len(boundary["items"]) == 1 and boundary["items"][0]["value"] == 31.2, str(boundary))
    status, empty, _ = request(port, "POST", "/v1/history/query", TOKENS[2], query(all_resources, 1, 2))
    require("M10 empty result", status == 200 and empty["items"] == [], str(empty))
    status, bad_only, _ = request(port, "POST", "/v1/history/query", TOKENS[2], query(all_resources, history_start, history_end, quality=["bad"]))
    require("M10 quality filter", status == 200 and len(bad_only["items"]) == 1 and bad_only["items"][0]["quality"] == "bad", str(bad_only))
    expect_error("M10 limit overflow", request(port, "POST", "/v1/history/query", TOKENS[2], query(all_resources, history_start, history_end, limit=1)), 422, "VALIDATION_FAILED")
    for name, body in [
        ("cursor", dict(query(all_resources, history_start, history_end), cursor="abc")),
        ("aggregate", dict(query(all_resources, history_start, history_end), mode="aggregate")),
        ("aggregation", dict(query(all_resources, history_start, history_end), aggregation={"bucket_ms": 1000})),
    ]:
        expect_error(f"M10 {name}", request(port, "POST", "/v1/history/query", TOKENS[2], body), 422, "CAPABILITY_UNSUPPORTED")
    for name, body in [
        ("wide range", query(all_resources, 1, 3600002)),
        ("too many resources", query([all_resources[0], all_resources[0], all_resources[1]], history_start, history_end)),
        ("zero limit", query(all_resources, history_start, history_end, limit=0)),
        ("bad range", query(all_resources, 2, 2)),
        ("bad quality", query(all_resources, history_start, history_end, quality=["good", "invalid"])),
        ("duplicate quality", query(all_resources, history_start, history_end, quality=["good", "good"])),
        ("unknown query field", dict(query(all_resources, history_start, history_end), sql="SELECT * FROM observations")),
    ]:
        expect_error(f"M10 {name}", request(port, "POST", "/v1/history/query", TOKENS[2], body), 422, "VALIDATION_FAILED")

    for path in ["/v1/source-state", "/v1/ingestions/ing-1", "/v1/devices/demo-a/shadow", "/v1/subscriptions", "/v1/control/config"]:
        expect_error(f"M12 unimplemented {path}", request(port, "GET", path, TOKENS[2]), 501, "CAPABILITY_UNSUPPORTED")
    expect_error("M12 unknown route", request(port, "GET", "/v1/not-a-route", TOKENS[2]), 404, "RESOURCE_NOT_FOUND")
    status, _, headers = request(port, "OPTIONS", "/v1/telemetry", TOKENS[0])
    require("M12 no CORS", status == 501 and "Access-Control-Allow-Origin" not in headers, f"status={status}, headers={headers}")

    duplicate_key = json.dumps(message("demo-a", "m2-duplicate-json-key", 1, now), separators=(",", ":"))
    duplicate_key = duplicate_key.replace('"spec":"iotfast.telemetry.v1"', '"spec":"iotfast.telemetry.v1","spec":"iotfast.telemetry.v1"')
    expect_error("M05 duplicate JSON key", request(port, "POST", "/v1/telemetry", TOKENS[0], duplicate_key), 422, "VALIDATION_FAILED")
    wait_history(port, 9, 1, now + 1000)

    process.send_signal(signal.SIGINT)
    process.wait(timeout=10)
    record("M11 SIGINT", True, f"exit={process.returncode}")
    with socket.socket() as sock:
        sock.settimeout(0.2)
        require("M11 port released", sock.connect_ex(("127.0.0.1", port)) != 0, f"port={port}")
    version_lines = [line for line in stderr_path.read_text().splitlines() if "DuckDB SELECT version():" in line]
    require("M11 server DuckDB version", len(version_lines) == 1, str(version_lines))
    database = data_dir / "engine.duckdb"
    require("M11 database exists", database.is_file(), str(database))
    reader = DuckDB(LIBRARY, database)
    try:
        rows = reader.rows("SELECT version()")
        require("M11 independent DuckDB version", len(rows) == 1 and rows[0][0].startswith("v"), str(rows))
        stored = reader.rows("SELECT observation_id, resource_id, device_id, source_at_ms, value, quality FROM observations")
        require("M11 independent DuckDB row count", len(stored) == len(accepted), f"rows={len(stored)}, expected={len(accepted)}")
        require("M11 independent DuckDB IDs", {row[0] for row in stored} == expected_ids, f"rows={len(stored)}")
        grants = reader.rows("SELECT source_id, device_id, valid_from_ms, valid_to_ms FROM source_device_grant ORDER BY device_id")
        require("M02 independent grant records", len(grants) == 2 and all(row[0] == "gateway-demo" for row in grants)
                and [row[1] for row in grants] == ["demo-a", "demo-b"]
                and all(int(row[3]) - int(row[2]) == 3_600_000 for row in grants), str(grants))
        hashes = reader.rows("SELECT payload_hash FROM committed_receipt WHERE message_id = 'm2-a-main'")
        require("M08 independent JCS payload hash", hashes == [[payload_hash(first)]], f"stored={hashes}, expected={payload_hash(first)}")
    finally:
        reader.close()


try:
    main()
    SUCCESS = True
except Exception as error:
    record("SMOKE RESULT", False, f"{type(error).__name__}: {error}")
finally:
    stop_servers()
    summary = {
        "task_id": "task_0a73f16941fc",
        "model": "auto/kimi-k3.1-pro (emergency substitute; not Terra validation)",
        "platform": platform.platform(),
        "python": sys.version.split()[0],
        "binary": str(BINARY),
        "duckdb_library": str(LIBRARY),
        "durable_traces": DURABLE_TRACES,
        "reset_traces": RESET_TRACES,
        "results": RESULTS,
    }
    (TEMP_ROOT / "results.json").write_text(json.dumps(summary, ensure_ascii=False, indent=2))
    if SUCCESS:
        for child in TEMP_ROOT.iterdir():
            if child.is_dir():
                shutil.rmtree(child)
        print(f"SMOKE PASS assertions={len(RESULTS)} evidence={TEMP_ROOT / 'results.json'}")
    else:
        print(f"SMOKE FAIL evidence={TEMP_ROOT / 'results.json'}")
if not SUCCESS:
    sys.exit(1)
