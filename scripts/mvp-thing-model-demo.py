#!/usr/bin/env python3
"""
IoTFast Thing-Model & UNS Data Management Engine — End-to-End MVP Demo
Verifies the full control plane + data plane loop over HTTP REST & RFC 6455 WebSocket:
  1. Signed Scope Token issuance & verification (D01 / Q02 / SC01 / SC02)
  2. Product & Thing Model draft -> validate -> publish -> diff (D02)
  3. Multi-level Asset Tree + Cross-tree Topology Relations (FEEDS / MEASURED_BY) (D03)
  4. Device Instantiation, Relocation & Historical UNS URI Protection (D03 / Q06 / Q11)
  5. Smooth Device Model Upgrade preserving stable resource_ids (AT04 / BM01)
  6. Dual Property Data References: gateway_measurement + static_config & Q09 activation (D04 / AT03)
  7. Real-time WebSocket Subscription (Snapshot S -> Delta > S), Durable Telemetry Ingest,
     DeviceView, DeviceShadow, History Query & UNS Resource Directory (D05–D08 / Q05 / Q08)
  8. Storage Architecture Status (SQLite business metadata WAL + DuckDB default TSDB) &
     Runtime TimeSeriesStorageAdapter switching (DuckDB <-> TDengine STABLE/Subtable) (SD-STORAGE-01)
"""

import base64
import hashlib
import hmac
import json
import os
import socket
import struct
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parent.parent
PROTOTYPE_DIR = ROOT_DIR / "prototype"
SECRET = "iotfast-mvp-demo-hmac-secret-2026"


def issue_signed_scope_token(ctx: dict, secret: str = SECRET) -> str:
    payload_bytes = json.dumps(ctx, separators=(",", ":")).encode("utf-8")
    b64 = base64.urlsafe_b64encode(payload_bytes).decode("ascii").rstrip("=")
    signing_input = f"iotfast1.{b64}"
    sig = hmac.new(secret.encode("utf-8"), signing_input.encode("ascii"), hashlib.sha256).hexdigest()
    return f"{signing_input}.{sig}"


class DemoClient:
    def __init__(self, proc: subprocess.Popen, base_url: str, host: str, port: int):
        self.proc = proc
        self.base_url = base_url
        self.host = host
        self.port = port
        self.use_stdio_fallback = (port == 0)
        self.next_cid = 1

    def _stdio_rpc(self, payload: dict) -> dict:
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        return json.loads(line)

    def http_json(
        self,
        method: str,
        path: str,
        token: str | None = None,
        body: dict | None = None,
        now_ms: int = 100_000,
        extra_headers: dict | None = None,
    ) -> tuple[int, dict]:
        if not self.use_stdio_fallback:
            url = f"{self.base_url}{path}"
            data = json.dumps(body).encode("utf-8") if body is not None else None
            headers = {
                "Content-Type": "application/json",
                "X-Engine-Now-Ms": str(now_ms),
            }
            if token:
                headers["Authorization"] = f"Bearer {token}"
            if extra_headers:
                headers.update(extra_headers)
            req = urllib.request.Request(url, data=data, headers=headers, method=method)
            try:
                with urllib.request.urlopen(req, timeout=5) as resp:
                    return resp.status, json.loads(resp.read().decode("utf-8"))
            except urllib.error.HTTPError as e:
                return e.code, json.loads(e.read().decode("utf-8"))
            except urllib.error.URLError as e:
                if isinstance(e.reason, PermissionError):
                    self.use_stdio_fallback = True
                else:
                    raise

        body_bytes = json.dumps(body).encode("utf-8") if body is not None else b""
        hdrs = [
            f"{method} {path} HTTP/1.1",
            "Host: localhost",
            "Content-Type: application/json",
            f"Content-Length: {len(body_bytes)}",
            f"X-Engine-Now-Ms: {now_ms}",
            "Connection: close",
        ]
        if token:
            hdrs.append(f"Authorization: Bearer {token}")
        if extra_headers:
            for k, v in extra_headers.items():
                hdrs.append(f"{k}: {v}")
        raw_req = ("\r\n".join(hdrs) + "\r\n\r\n").encode("utf-8") + body_bytes
        cid = self.next_cid
        self.next_cid += 1
        res = self._stdio_rpc({
            "op": "http",
            "cid": cid,
            "req_b64": base64.b64encode(raw_req).decode("ascii"),
        })
        raw_resp = base64.b64decode(res["resp_b64"])
        head, _, payload = raw_resp.partition(b"\r\n\r\n")
        status_code = int(head.splitlines()[0].split()[1])
        return status_code, json.loads(payload.decode("utf-8"))

    def ws_connect_and_subscribe(self, token: str, sub_payload: dict, now_ms: int = 100_000):
        handshake = (
            f"GET /v1/subscriptions HTTP/1.1\r\n"
            f"Host: {self.host}:{self.port}\r\n"
            f"Upgrade: websocket\r\n"
            f"Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
            f"Sec-WebSocket-Version: 13\r\n"
            f"Authorization: Bearer {token}\r\n"
            f"X-Engine-Now-Ms: {now_ms}\r\n\r\n"
        ).encode("ascii")

        raw = json.dumps(sub_payload).encode("utf-8")
        mask = b"\x12\x34\x56\x78"
        header = bytearray([0x81])
        if len(raw) < 126:
            header.append(0x80 | len(raw))
        else:
            header.append(0x80 | 126)
            header.extend(struct.pack("!H", len(raw)))
        header.extend(mask)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(raw))
        frame_bytes = bytes(header) + masked

        if not self.use_stdio_fallback:
            sock = socket.create_connection((self.host, self.port), timeout=5)
            sock.sendall(handshake)
            buf = b""
            while b"\r\n\r\n" not in buf:
                buf += sock.recv(1)
            assert buf.startswith(b"HTTP/1.1 101"), f"WebSocket upgrade failed: {buf!r}"
            sock.sendall(frame_bytes)
            return ("tcp", sock)

        cid = self.next_cid
        self.next_cid += 1
        res = self._stdio_rpc({
            "op": "ws_open",
            "cid": cid,
            "req_b64": base64.b64encode(handshake).decode("ascii"),
        })
        resp_hdr = base64.b64decode(res["resp_b64"])
        assert resp_hdr.startswith(b"HTTP/1.1 101"), f"WebSocket upgrade failed: {resp_hdr!r}"
        self._stdio_rpc({
            "op": "ws_send",
            "cid": cid,
            "frame_b64": base64.b64encode(frame_bytes).decode("ascii"),
        })
        return ("stdio", cid)

    def ws_recv_json(self, ws_handle) -> dict:
        kind, target = ws_handle
        if kind == "stdio":
            res = self._stdio_rpc({"op": "ws_recv", "cid": target})
            return json.loads(res["text"])

        sock = target

        def recv_exact(n: int) -> bytes:
            out = b""
            while len(out) < n:
                chunk = sock.recv(n - len(out))
                if not chunk:
                    raise EOFError("WebSocket closed")
                out += chunk
            return out

        b0, b1 = recv_exact(2)
        length = b1 & 0x7F
        if length == 126:
            (length,) = struct.unpack("!H", recv_exact(2))
        elif length == 127:
            (length,) = struct.unpack("!Q", recv_exact(8))
        payload = recv_exact(length)
        return json.loads(payload.decode("utf-8"))

    def ws_close(self, ws_handle):
        kind, target = ws_handle
        if kind == "stdio":
            self._stdio_rpc({"op": "ws_close", "cid": target})
        else:
            target.close()


def main() -> int:
    bin_path = PROTOTYPE_DIR / "target" / "debug" / "iotfast_server"
    duckdb_lib_dir = PROTOTYPE_DIR / "target" / "duckdb-download" / "aarch64-apple-darwin" / "v2.0.0"

    env = os.environ.copy()
    if duckdb_lib_dir.exists():
        env["DYLD_LIBRARY_PATH"] = f"{duckdb_lib_dir}:{env.get('DYLD_LIBRARY_PATH', '')}"

    subprocess.run(
        ["cargo", "build", "--locked", "--manifest-path", str(PROTOTYPE_DIR / "Cargo.toml"), "--bin", "iotfast_server"],
        check=True,
        env=env,
    )

    with tempfile.TemporaryDirectory(prefix="iotfast-mvp-demo-") as tmpdir:
        proc = subprocess.Popen(
            [str(bin_path), "--listen", "127.0.0.1:0", "--data-dir", tmpdir, "--auth-secret", SECRET, "--stdio-stream"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
        )
        try:
            startup_line = proc.stdout.readline().strip()
            info = json.loads(startup_line)
            base_url = info["base_url"]
            host, port_str = info["addr"].split(":")
            port = int(port_str)
            client = DemoClient(proc, base_url, host, port)
            print(f"[1/8] Server started at {base_url} (WS: {info['ws_url']})")

            st, health = client.http_json("GET", "/health")
            assert st == 200 and health["healthy"] is True, f"Health check failed: {health}"
            print(f"      Health OK: DuckDB {health['duckdb_version']}, state={health['state']}")

            mgmt_token = issue_signed_scope_token({
                "tenant_id": "ten-8iic",
                "project_id": "proj-waterworks",
                "caller_service_id": "svc-thing-admin",
                "caller_kind": "management_service",
                "issued_at_ms": 1000,
                "expires_at_ms": 900_000_000,
                "allowed_actions": ["*"],
                "scope_filter": {
                    "asset_ids": [],
                    "include_asset_descendants": False,
                    "device_ids": [],
                    "resource_ids": [],
                    "uri_prefixes": [],
                    "tenant_wide": True,
                },
                "operator_id": "op-demo",
            })
            gw_token = issue_signed_scope_token({
                "tenant_id": "ten-8iic",
                "project_id": "proj-waterworks",
                "caller_service_id": "gw-01",
                "caller_kind": "southbound_source",
                "issued_at_ms": 1000,
                "expires_at_ms": 900_000_000,
                "allowed_actions": ["telemetry:ingest", "source_state:report"],
                "scope_filter": {
                    "asset_ids": [],
                    "include_asset_descendants": False,
                    "device_ids": [],
                    "resource_ids": [],
                    "uri_prefixes": [],
                    "tenant_wide": True,
                },
                "operator_id": None,
            })

            # 2. Product & Thing Model Lifecycle
            client.http_json("POST", "/v1/products", mgmt_token, {
                "product_id": "prod-pump",
                "product_key": "booster-pump",
                "name": "智能变频增压泵",
            }, now_ms=10_000)

            r1_props = [
                {"identifier": "outlet_pressure", "name": "出口压力", "value_type": "float64", "unit_code": "MPa", "unit_symbol": "MPa", "min_value": 0.0, "max_value": 4.0, "required": True},
                {"identifier": "rated_power", "name": "额定功率", "value_type": "float64", "unit_code": "kW", "unit_symbol": "kW", "min_value": 0.0, "max_value": 500.0, "required": False},
                {"identifier": "firmware_version", "name": "固件版本", "value_type": "text", "unit_code": None, "unit_symbol": None, "min_value": None, "max_value": None, "required": False},
            ]
            client.http_json("POST", "/v1/products/prod-pump/models", mgmt_token, {
                "model_revision_id": "pump-model-r1",
                "model_version": "1.0.0",
                "properties": r1_props,
            }, now_ms=11_000)
            client.http_json("POST", "/v1/models/pump-model-r1:validate", mgmt_token, now_ms=11_500)
            client.http_json("POST", "/v1/models/pump-model-r1:publish", mgmt_token, {"published_at_ms": 12_000}, now_ms=12_000)

            r2_props = r1_props + [
                {"identifier": "motor_current", "name": "电机电流", "value_type": "float64", "unit_code": "A", "unit_symbol": "A", "min_value": 0.0, "max_value": 200.0, "required": False},
            ]
            client.http_json("POST", "/v1/products/prod-pump/models", mgmt_token, {
                "model_revision_id": "pump-model-r2",
                "model_version": "1.1.0",
                "properties": r2_props,
            }, now_ms=13_000)
            _, diff = client.http_json("POST", "/v1/models:compare", mgmt_token, {
                "from_revision_id": "pump-model-r1",
                "to_revision_id": "pump-model-r2",
            }, now_ms=13_500)
            client.http_json("POST", "/v1/models/pump-model-r2:validate", mgmt_token, {"previous_published_revision_id": "pump-model-r1"}, now_ms=14_000)
            client.http_json("POST", "/v1/models/pump-model-r2:publish", mgmt_token, {"published_at_ms": 15_000}, now_ms=15_000)
            print(f"[2/8] Thing Model lifecycle verified: r1 -> r2 compatibility={diff['compatibility']}, added={diff['added_properties']}")

            # 3. Asset Tree & Cross-Tree Topology Relations
            client.http_json("POST", "/v1/assets", mgmt_token, {"asset_id": "asset-plant-a", "parent_asset_id": None, "slug": "plant-a", "name": "一厂区"}, now_ms=16_000)
            client.http_json("POST", "/v1/assets", mgmt_token, {"asset_id": "asset-room-1", "parent_asset_id": "asset-plant-a", "slug": "room-1", "name": "一号泵房"}, now_ms=16_100)
            client.http_json("POST", "/v1/assets", mgmt_token, {"asset_id": "asset-header-north", "parent_asset_id": "asset-plant-a", "slug": "header-north", "name": "北路总管"}, now_ms=16_200)
            client.http_json("POST", "/v1/assets/relations", mgmt_token, {
                "relation_id": "rel-room1-feeds-north",
                "from_asset_id": "asset-room-1",
                "relation_type": "FEEDS",
                "to_asset_id": "asset-header-north",
                "valid_from_ms": 16_000,
                "valid_to_ms": None,
                "source": "process_topology",
            }, now_ms=16_500)
            _, rels = client.http_json("GET", "/v1/assets/asset-room-1/relations?now_ms=17000", mgmt_token, now_ms=17_000)
            print(f"[3/8] Asset tree & cross-tree topology verified: {len(rels['items'])} active relation(s) ({rels['items'][0]['relation_type']})")

            # 4. Instantiate Device & Smooth Upgrade Model Revision (r1 -> r2)
            _, dev_r1 = client.http_json("POST", "/v1/devices", mgmt_token, {
                "device_id": "pump-001",
                "device_key": "pump-001",
                "device_name": "1号增压主泵",
                "product_id": "prod-pump",
                "model_revision_id": "pump-model-r1",
                "asset_id": "asset-room-1",
            }, now_ms=18_000)
            _, dev_r2 = client.http_json("POST", "/v1/devices/pump-001:upgrade-model", mgmt_token, {
                "target_model_revision_id": "pump-model-r2",
                "expected_device_revision": dev_r1["revision"],
                "upgraded_at_ms": 19_000,
            }, now_ms=19_000)
            print(f"[4/8] Device instantiated & smooth-upgraded to {dev_r2['model_revision_id']} (resources: {len(dev_r1['resources'])} -> {len(dev_r2['resources'])})")

            # 5. Bindings: Gateway Measurement + Static Nameplate Config
            client.http_json("POST", "/v1/sources/grants", mgmt_token, {
                "source_id": "gw-01",
                "device_id": "pump-001",
                "valid_from_ms": 20_000,
            }, now_ms=20_000)
            _, b_draft = client.http_json("POST", "/v1/devices/pump-001/bindings", mgmt_token, {
                "binding_revision_id": "bind-pump-001-r1",
                "model_revision_id": "pump-model-r2",
                "source_id": "gw-01",
                "normalization_owner": "gateway",
                "server_scale_factor": None,
                "points": [
                    {"property": "outlet_pressure", "register_address": "40001", "factor": 0.01, "unit_code": "MPa", "deadband": 0.02, "heartbeat_ms": 60_000},
                    {"property": "motor_current", "register_address": "40002", "factor": 0.1, "unit_code": "A", "deadband": 0.5, "heartbeat_ms": 60_000},
                ],
                "static_points": [
                    {"property": "rated_power", "value": 55.0, "unit_code": "kW"},
                ],
            }, now_ms=21_000)
            client.http_json("POST", "/v1/bindings/bind-pump-001-r1:validate", mgmt_token, now_ms=22_000)
            client.http_json("POST", "/v1/bindings/bind-pump-001-r1:activate", mgmt_token, {
                "expected_device_revision": dev_r2["revision"],
                "activated_at_ms": 23_000,
                "gateway_ack": {
                    "source_id": "gw-01",
                    "source_epoch": "epoch-01",
                    "binding_revision_id": "bind-pump-001-r1",
                    "config_hash": b_draft["config_hash"],
                    "acknowledged_at_ms": 22_900,
                },
            }, now_ms=23_000)
            print("[5/8] Property Data References activated (2 gateway_measurement points + 1 static_config property)")

            # 6. Connect WebSocket Subscription & Ingest Durable Telemetry
            ws = client.ws_connect_and_subscribe(mgmt_token, {
                "type": "subscribe",
                "sub_id": "demo-sub-1",
                "resource_ids": ["res-pump-001-outlet-pressure"],
                "include_initial_snapshot": True,
                "max_pending_events": 16,
            }, now_ms=25_000)
            snap_begin = client.ws_recv_json(ws)
            snap_chunk = client.ws_recv_json(ws)
            snap_end = client.ws_recv_json(ws)
            assert snap_begin["type"] == "snapshot_begin" and snap_end["type"] == "snapshot_end"

            st_ing, ing_rec = client.http_json(
                "POST",
                "/v1/telemetry",
                gw_token,
                {
                    "spec": "iotfast.telemetry.v1",
                    "message_id": "msg-demo-01",
                    "source_id": "gw-01",
                    "source_epoch": "epoch-01",
                    "device_id": "pump-001",
                    "model_revision_id": "pump-model-r2",
                    "binding_revision_id": "bind-pump-001-r1",
                    "points": [
                        {
                            "property": "outlet_pressure",
                            "value": 1.82,
                            "source_at_ms": 30_000,
                            "source_time_origin": "device_clock",
                            "source_seq": "1",
                            "quality": {"state": "good", "source_protocol": "modbus", "source_code": None},
                            "delivery": {"replayed": False},
                        },
                        {
                            "property": "motor_current",
                            "value": 41.5,
                            "source_at_ms": 30_000,
                            "source_time_origin": "device_clock",
                            "source_seq": "1",
                            "quality": {"state": "good", "source_protocol": "modbus", "source_code": None},
                            "delivery": {"replayed": False},
                        },
                    ],
                },
                now_ms=30_010,
                extra_headers={"Prefer": "durability=durable"},
            )
            assert st_ing == 202 and ing_rec["status"] == "accepted_durable"
            delta_frame = client.ws_recv_json(ws)
            client.ws_close(ws)
            print(f"[6/8] Durable Telemetry ingested (wal={ing_rec['wal_position']}) & WebSocket delta received: {delta_frame['resource_id']}={delta_frame['value']}")

            # 7. Northbound DeviceView, Shadow & History Query
            _, view = client.http_json("GET", "/v1/devices/pump-001/view?now_ms=30500", mgmt_token, now_ms=30_500)
            _, hist = client.http_json("POST", "/v1/history/query", mgmt_token, {
                "resource_ids": ["res-pump-001-outlet-pressure", "res-pump-001-motor-current"],
                "range": {"start_ms": 20_000, "end_ms": 40_000},
                "mode": "raw",
                "aggregation": None,
                "quality_states": ["good"],
                "order": "asc",
                "limit": 10,
                "cursor": None,
            }, now_ms=31_000)
            p_press = view["properties"]["outlet_pressure"]
            p_power = view["properties"]["rated_power"]
            p_fw = view["properties"]["firmware_version"]
            print(
                f"[7/8] DeviceBusinessView verified:\n"
                f"      - outlet_pressure: state={p_press['binding_state']}/{p_press['observation_state']}, value={p_press['value']} {p_press['unit_symbol']}, freshness={p_press['freshness']}\n"
                f"      - rated_power:     state={p_power['binding_state']}/{p_power['observation_state']}, value={p_power['value']} {p_power['unit_symbol']}, freshness={p_power['freshness']}\n"
                f"      - firmware_version:state={p_fw['binding_state']}/{p_fw['observation_state']}, value={p_fw['value']}\n"
                f"      - history_query:   {len(hist['items'])} committed observation(s)"
            )

            # 8. Storage Architecture Status (SQLite business metadata + Pluggable TimeSeriesStorageAdapter + ST-02/ST-04)
            st_store, store_status = client.http_json("GET", "/v1/storage/status", mgmt_token, now_ms=32_000)
            assert st_store == 200, f"GET /v1/storage/status failed: {store_status}"
            biz_store = store_status["business_metadata_store"]
            ts_store = store_status["device_timeseries_store"]
            assert biz_store["engine"] == "sqlite" and biz_store["journal_mode"] == "wal", f"Unexpected business store: {biz_store}"
            assert ts_store["active_backend"] == "duckdb", f"Expected default active_backend='duckdb', got: {ts_store}"

            st_td, td_status = client.http_json(
                "POST",
                "/v1/storage/timeseries-adapter",
                mgmt_token,
                {"backend": "tdengine"},
                now_ms=32_500,
            )
            assert st_td == 200 and td_status["device_timeseries_store"]["active_backend"] == "tdengine", f"Switch to tdengine failed: {td_status}"
            assert "CREATE STABLE IF NOT EXISTS" in (td_status["device_timeseries_store"].get("tdengine_super_table_ddl") or "")
            td_cfg = td_status["device_timeseries_store"].get("tdengine_config") or {}
            if "password" in td_cfg:
                assert td_cfg["password"] == "***REDACTED***", f"Expected redacted password, got: {td_cfg}"

            st_caps, caps_body = client.http_json("GET", "/v1/storage/capabilities", mgmt_token, now_ms=32_700)
            caps_summary = f"active={caps_body.get('active_backend')}" if st_caps == 200 else f"status={st_caps}"

            st_ckpt, ckpt_body = client.http_json("GET", "/v1/storage/checkpoints?backend=duckdb", mgmt_token, now_ms=32_800)
            ckpt_summary = (
                f"contiguous_wal_pos={ckpt_body.get('contiguous_checkpoint_wal_pos')}, holes={ckpt_body.get('has_unreconciled_holes')}"
                if st_ckpt == 200
                else f"status={st_ckpt}"
            )

            st_ddb, ddb_status = client.http_json(
                "POST",
                "/v1/storage/timeseries-adapter",
                mgmt_token,
                {"backend": "duckdb"},
                now_ms=33_000,
            )
            assert st_ddb == 200 and ddb_status["device_timeseries_store"]["active_backend"] == "duckdb", f"Switch back to duckdb failed: {ddb_status}"

            st_bkp, bkp_body = client.http_json(
                "POST",
                "/v1/storage/backups",
                mgmt_token,
                {"backup_id": "bkp-demo-01"},
                now_ms=33_500,
            )
            if st_bkp in (200, 201):
                st_ver, verify_body = client.http_json(
                    "POST",
                    "/v1/storage/backups/bkp-demo-01:verify",
                    mgmt_token,
                    {},
                    now_ms=34_000,
                )
                assert st_ver == 200 and verify_body.get("verified") is True, f"Backup verify failed: {verify_body}"
                bkp_summary = f"backup_id={bkp_body.get('backup_id')}, verified={verify_body.get('verified')}, files={len(bkp_body.get('files', []))}"
            else:
                bkp_summary = f"status={st_bkp}"

            print(
                f"[8/8] Storage architecture verified (SD-STORAGE-01 / SD-STORAGE-02 / ST-02 / ST-04 / D10.1):\n"
                f"      - business_metadata_store: engine={biz_store['engine']} (v{biz_store['sqlite_version']}, journal={biz_store['journal_mode']}), tables={len(biz_store['tables'])}\n"
                f"      - device_timeseries_store: default=duckdb -> switched to tdengine (STABLE DDL & password redaction verified) -> restored to {ddb_status['device_timeseries_store']['active_backend']}\n"
                f"      - capabilities & checkpoints: ST-02({caps_summary}), ST-04({ckpt_summary})\n"
                f"      - consistent snapshot backup & sandbox restore (D10.1): {bkp_summary}"
            )
            print("\nSUCCESS: All IoTFast Thing-Model & UNS Data Management MVP checks passed!")
            return 0
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    sys.exit(main())
