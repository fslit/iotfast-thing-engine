import http.client
import json
import os
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
BINARY = os.path.join(ROOT, "prototype/target/debug/dev_server")


def telemetry_body():
    return json.dumps({
        "spec": "iotfast.telemetry.v1",
        "message_id": "early-reject-reader",
        "source_id": "gateway-demo",
        "source_epoch": "demo-epoch-1",
        "device_id": "demo-a",
        "model_revision_id": "mr-demo-1",
        "binding_revision_id": "br-demo-a-1",
        "points": [{
            "property": "temperature",
            "value": 245.6,
            "source_at_ms": int(time.time() * 1000),
            "source_time_origin": "gateway_sample",
            "source_seq": "1",
            "quality": {"state": "good", "source_protocol": "modbus", "source_code": None},
            "delivery": {"replayed": False},
        }],
    }, separators=(",", ":")).encode()


def response_bytes(sock):
    data = bytearray()
    while b"\r\n\r\n" not in data:
        chunk = sock.recv(4096)
        if not chunk:
            raise AssertionError("connection closed before response headers")
        data.extend(chunk)
    header_end = data.index(b"\r\n\r\n") + 4
    headers = bytes(data[:header_end]).decode("ascii").split("\r\n")
    length = None
    for header in headers[1:]:
        if not header:
            continue
        name, value = header.split(":", 1)
        if name.lower() == "content-length":
            length = int(value.strip())
    if length is None:
        raise AssertionError("missing Content-Length")
    while len(data) - header_end < length:
        chunk = sock.recv(4096)
        if not chunk:
            raise AssertionError("connection closed before complete response body")
        data.extend(chunk)
    status = int(headers[0].split(" ", 2)[1])
    return status, json.loads(data[header_end:header_end + length])


def assert_error(status, payload, expected_status, expected_code):
    assert status == expected_status, (status, payload)
    assert payload["code"] == expected_code, payload
    assert payload["message"] == expected_code, payload
    assert payload["retryable"] is (expected_status in (429, 503)), payload
    assert payload["retry_after_ms"] == (1000 if expected_status in (429, 503) else None), payload
    assert payload["details"] == [], payload
    assert isinstance(payload["request_id"], str) and payload["request_id"], payload


def main():
    tokens = [secrets.token_hex(32) for _ in range(3)]
    process = None
    version = "unavailable"
    try:
        with tempfile.TemporaryDirectory(prefix="iotfast-m1-early-reject-", dir="/tmp") as data:
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            print(f"command: python3 {os.path.relpath(__file__, ROOT)}")
            print(f"port: 127.0.0.1:{port} (auto-selected loopback)")
            print("data policy: newly created empty /tmp directory; removed on exit")
            env = os.environ.copy()
            env.update(
                IOTFAST_DEV_DATA_DIR=data,
                IOTFAST_DEV_BIND=f"127.0.0.1:{port}",
                IOTFAST_DEV_INGEST_TOKEN=tokens[0],
                IOTFAST_DEV_READER_TOKEN=tokens[1],
                IOTFAST_DEV_ALL_TOKEN=tokens[2],
                DYLD_LIBRARY_PATH=os.path.join(
                    ROOT, "prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0"
                ),
            )
            process = subprocess.Popen(
                [BINARY], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True
            )
            first_log = process.stderr.readline().strip()
            prefix = "DuckDB SELECT version(): "
            if first_log.startswith(prefix):
                version = first_log[len(prefix):]
            body = telemetry_body()
            for _ in range(100):
                if process.poll() is not None:
                    raise AssertionError("dev_server exited before health check")
                try:
                    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                    connection.request("GET", "/health")
                    health = connection.getresponse()
                    health.read()
                    connection.close()
                    if health.status == 200:
                        break
                except OSError:
                    time.sleep(0.05)
            else:
                raise AssertionError("dev_server did not become healthy")

            base_headers = (
                f"POST /v1/telemetry HTTP/1.1\r\n"
                f"Host: 127.0.0.1:{port}\r\n"
                f"Authorization: Bearer {tokens[1]}\r\n"
                f"Content-Type: application/json\r\n"
                f"Content-Length: {len(body)}\r\n"
                f"Connection: close\r\n\r\n"
            ).encode("ascii")
            for attempt in range(50):
                with socket.create_connection(("127.0.0.1", port), timeout=5) as sock:
                    sock.settimeout(5)
                    sock.sendall(base_headers + body)
                    status, payload = response_bytes(sock)
                    assert_error(status, payload, 403, "FORBIDDEN_ACTION")
                print(f"reader rejection {attempt + 1}/50: 403 FORBIDDEN_ACTION")

            with socket.create_connection(("127.0.0.1", port), timeout=5) as sock:
                sock.settimeout(5)
                sock.sendall(base_headers)
                time.sleep(0.1)
                sock.sendall(body)
                status, payload = response_bytes(sock)
                assert_error(status, payload, 403, "FORBIDDEN_ACTION")
            print("slow body rejection: 403 FORBIDDEN_ACTION after 100 ms delayed body")

            durable_headers = base_headers.replace(
                b"Content-Type: application/json\r\n",
                b"Content-Type: application/json\r\nPrefer: durability=durable\r\n",
            ).replace(f"Bearer {tokens[1]}".encode(), f"Bearer {tokens[0]}".encode())
            with socket.create_connection(("127.0.0.1", port), timeout=5) as sock:
                sock.settimeout(5)
                sock.sendall(durable_headers)
                time.sleep(0.1)
                sock.sendall(body)
                status, payload = response_bytes(sock)
                assert_error(status, payload, 503, "DURABILITY_BACKPRESSURE")
            print("early durable rejection: 503 DURABILITY_BACKPRESSURE")
            print(f"DuckDB actual version: {version}")
            print("PASS: 50 reader rejections + slow body + durable early rejection; zero connection resets")
            return 0
    except Exception as error:
        print(f"FAIL: {type(error).__name__}: {error}", file=sys.stderr)
        print(f"DuckDB actual version: {version}", file=sys.stderr)
        return 1
    finally:
        if process is not None and process.poll() is None:
            process.send_signal(signal.SIGINT)
            process.wait(timeout=10)
        if process is not None:
            remaining_log = process.stderr.read().strip()
            if remaining_log:
                print(f"server stderr: {remaining_log}")


exit_code = main()
print(f"exit_code={exit_code}")
raise SystemExit(exit_code)
