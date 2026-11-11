import json
import os
import secrets
import signal
import socket
import subprocess
import tempfile
import time

root = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
tokens = [secrets.token_hex(32) for _ in range(3)]
with tempfile.TemporaryDirectory(prefix="iotfast-m1-", dir="/tmp") as data:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env = os.environ.copy()
    env.update(IOTFAST_DEV_DATA_DIR=data, IOTFAST_DEV_BIND=f"127.0.0.1:{port}",
               IOTFAST_DEV_INGEST_TOKEN=tokens[0], IOTFAST_DEV_READER_TOKEN=tokens[1],
               IOTFAST_DEV_ALL_TOKEN=tokens[2],
               DYLD_LIBRARY_PATH=os.path.join(root, "prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0"))
    process = subprocess.Popen([os.path.join(root, "prototype/target/debug/dev_server")], env=env,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    def call(path, token=None, body=None, extra=()):
        args = ["curl", "-sS", "--max-time", "3", "-w", "\n%{http_code}", f"http://127.0.0.1:{port}{path}"]
        if token is not None:
            args += ["-H", f"Authorization: Bearer {token}"]
        for header in extra:
            args += ["-H", header]
        if body is not None:
            args += ["-H", "Content-Type: application/json", "--data-binary", json.dumps(body)]
        result = subprocess.run(args, capture_output=True, text=True, check=True)
        payload, status = result.stdout.rsplit("\n", 1)
        return int(status), json.loads(payload)
    try:
        for attempt in range(100):
            if process.poll() is not None:
                raise RuntimeError("server exited: " + process.stderr.read())
            try:
                health = call("/health")
                break
            except subprocess.CalledProcessError:
                time.sleep(0.05)
        else:
            raise RuntimeError("server did not start")
        assert health[0] == 200 and health[1]["durability"] == "volatile"
        assert call("/v1/devices/demo-a/view")[0] == 401
        assert call("/v1/devices/demo-b/view", tokens[1])[0] == 404
        assert call("/v1/devices/demo-a/view", tokens[0])[0] == 403
        stamp = int(time.time() * 1000) - 1000
        def message(device, identifier, value, quality, sampled):
            return {"spec":"iotfast.telemetry.v1", "message_id":identifier, "source_id":"gateway-demo",
                    "source_epoch":"demo-epoch-1", "device_id":device, "model_revision_id":"mr-demo-1",
                    "binding_revision_id":f"br-{device}-1", "points":[{"property":"temperature", "value":value,
                    "source_at_ms":sampled, "source_time_origin":"gateway_sample", "source_seq":"1",
                    "quality":{"state":quality, "source_protocol":"modbus", "source_code":None},
                    "delivery":{"replayed":False}}]}
        first = message("demo-a", "m1-a", 20.5, "bad", stamp)
        assert call("/v1/telemetry", tokens[0], first, ["Prefer: durability=durable"])[0] == 503
        assert call("/v1/telemetry", tokens[0], first)[0] == 202
        assert call("/v1/telemetry", tokens[0], first)[0] == 200
        changed = message("demo-a", "m1-a", 22, "good", stamp)
        assert call("/v1/telemetry", tokens[0], changed)[0] == 409
        invalid = message("demo-a", "m1-invalid", 1, "good", stamp)
        invalid["scope"] = {"device_id":"demo-b"}
        assert call("/v1/telemetry", tokens[0], invalid)[0] == 422
        del invalid["scope"]
        invalid["points"].append(invalid["points"][0])
        assert call("/v1/telemetry", tokens[0], invalid)[0] == 422
        invalid["points"].pop()
        invalid["points"][0]["source_at_ms"] = stamp-200
        assert call("/v1/telemetry", tokens[0], invalid)[0] == 202
        assert call("/v1/telemetry", tokens[0], message("demo-b", "m1-b", -3.25, "good", stamp))[0] == 202
        assert call("/v1/telemetry", tokens[0], message("demo-a", "m1-old", 5, "good", stamp-100))[0] == 202
        for attempt in range(100):
            view_a = call("/v1/devices/demo-a/view", tokens[2])[1]["properties"]["temperature"]
            view_b = call("/v1/devices/demo-b/view", tokens[2])[1]["properties"]["temperature"]
            if view_a["value"] == 20.5 and view_b["value"] == -3.25:
                break
            time.sleep(0.05)
        assert view_a["value"] == 20.5 and view_a["quality"] == "bad" and view_a["value_sampled_at_ms"] == stamp
        assert view_b["value"] == -3.25
        query = {"resource_ids":["res-demo-a-temperature", "res-demo-b-temperature"],
                 "range":{"start_ms":stamp-201, "end_ms":stamp+1}, "mode":"raw", "aggregation":None,
                 "quality_states":["good", "bad"], "order":"asc", "limit":100, "cursor":None}
        for attempt in range(100):
            status, history = call("/v1/history/query", tokens[2], query)
            if status == 200 and len(history["items"]) == 4:
                break
            time.sleep(0.05)
        assert status == 200 and len(history["items"]) == 4
        assert call("/v1/history/query", tokens[1], query)[0] == 404
        print("PASS health, auth, scope, atomic rejection, durable, dedup, conflict, device isolation, bad quality, sample time, ordering, raw history")
    finally:
        process.send_signal(signal.SIGINT)
        process.wait(timeout=10)
        print("server stderr:", process.stderr.read().strip())
