#!/usr/bin/env python3
"""
Local Apifox Live Debug Server Launcher for `iotfast-thing-engine`.

Starts `iotfast_server` (default `http://127.0.0.1:8080`), issues a long-lived
full-scope HMAC-SHA256 Bearer Token for Apifox Global Auth, and optionally seeds
the standard demo dataset (`prod-pump`, `pump-model-r1`, `asset-room-1`,
`pump-001`, `gw-01`, `bind-pump-001-r1`, and initial telemetry `1.65 MPa`) so
every GET/POST endpoint in Apifox can be clicked and tested immediately.

Security rule (AGENTS.md):
- Uses a local development HMAC secret (`iotfast-apifox-local-dev-secret-2026`).
- Never embeds or writes any external credentials or `afxp_` tokens.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent
PROTOTYPE_DIR = REPO_ROOT / "prototype"
DEFAULT_SECRET = "iotfast-apifox-local-dev-secret-2026"


def issue_signed_scope_token(ctx: Dict[str, Any], secret: str = DEFAULT_SECRET) -> str:
    payload_bytes = json.dumps(ctx, separators=(",", ":")).encode("utf-8")
    b64 = base64.urlsafe_b64encode(payload_bytes).decode("ascii").rstrip("=")
    signing_input = f"iotfast1.{b64}"
    sig = hmac.new(
        secret.encode("utf-8"),
        signing_input.encode("ascii"),
        hashlib.sha256,
    ).hexdigest()
    return f"{signing_input}.{sig}"


def build_apifox_universal_token(secret: str = DEFAULT_SECRET) -> str:
    """
    Issues a long-lived development token valid through year 2100 that authorizes:
    - All Control-Plane & Northbound actions (`allowed_actions: ["*"]`, `tenant_wide: True`)
    - Southbound ingestion for `gw-01` (`caller_service_id: "gw-01"`, `caller_kind: "southbound_source"`)
    - Manual window operator actions (`operator_id: "op-apifox"`)
    """
    return issue_signed_scope_token(
        {
            "tenant_id": "ten-8iic",
            "project_id": "proj-waterworks",
            "caller_service_id": "gw-01",
            "caller_kind": "southbound_source",
            "issued_at_ms": 0,
            "expires_at_ms": 4_102_444_800_000,
            "allowed_actions": ["*"],
            "scope_filter": {
                "asset_ids": [],
                "include_asset_descendants": True,
                "device_ids": [],
                "resource_ids": [],
                "uri_prefixes": [],
                "tenant_wide": True,
            },
            "operator_id": "op-apifox",
        },
        secret=secret,
    )


class LocalServerClient:
    def __init__(self, proc: subprocess.Popen, base_url: str, port: int) -> None:
        self.proc = proc
        self.base_url = base_url
        self.port = port
        self.use_stdio_fallback = port == 0
        self.next_cid = 1

    def _stdio_rpc(self, payload: Dict[str, Any]) -> Dict[str, Any]:
        assert self.proc.stdin is not None and self.proc.stdout is not None
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        return json.loads(line)

    def request_json(
        self,
        method: str,
        path: str,
        token: Optional[str] = None,
        body: Optional[Dict[str, Any]] = None,
        now_ms: Optional[int] = None,
    ) -> Tuple[int, Dict[str, Any]]:
        if not self.use_stdio_fallback:
            url = f"{self.base_url}{path}"
            data = json.dumps(body).encode("utf-8") if body is not None else None
            headers = {"Content-Type": "application/json"}
            if now_ms is not None:
                headers["X-Engine-Now-Ms"] = str(now_ms)
            if token:
                headers["Authorization"] = f"Bearer {token}"
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
            "Connection: close",
        ]
        if now_ms is not None:
            hdrs.append(f"X-Engine-Now-Ms: {now_ms}")
        if token:
            hdrs.append(f"Authorization: Bearer {token}")
        raw_req = ("\r\n".join(hdrs) + "\r\n\r\n").encode("utf-8") + body_bytes
        cid = self.next_cid
        self.next_cid += 1
        res = self._stdio_rpc(
            {
                "op": "http",
                "cid": cid,
                "req_b64": base64.b64encode(raw_req).decode("ascii"),
            }
        )
        raw_resp = base64.b64decode(res["resp_b64"])
        head, _, payload = raw_resp.partition(b"\r\n\r\n")
        status_code = int(head.splitlines()[0].split()[1])
        return status_code, json.loads(payload.decode("utf-8"))


def seed_demo_data(client: LocalServerClient, token: str) -> None:
    """
    Pre-populates the standard demo scenario (`prod-pump`, `pump-model-r1`,
    `pump-model-r2`, `asset-plant-a`, `asset-room-1`, `asset-room-2`,
    `asset-header-north`, `pump-001`, `gw-01`, `bind-pump-001-r1`, and initial
    telemetry `1.65 MPa` -> `ing-000001`).
    """
    # 1. Product
    st, _ = client.request_json(
        "POST",
        "/v1/products",
        token,
        {
            "product_id": "prod-pump",
            "product_key": "booster-pump",
            "name": "智能变频增压泵",
        },
        now_ms=10_000,
    )
    if st not in (200, 201, 409):
        raise RuntimeError(f"Failed to seed product prod-pump (HTTP {st})")

    # 2. Model Revisions r1 & r2
    r1_props = [
        {
            "identifier": "outlet_pressure",
            "name": "出口压力",
            "value_type": "float64",
            "unit_code": "MPa",
            "unit_symbol": "MPa",
            "min_value": 0.0,
            "max_value": 4.0,
            "required": True,
        },
        {
            "identifier": "rated_power",
            "name": "额定功率",
            "value_type": "float64",
            "unit_code": "kW",
            "unit_symbol": "kW",
            "min_value": 0.0,
            "max_value": 500.0,
            "required": False,
        },
    ]
    client.request_json(
        "POST",
        "/v1/products/prod-pump/models",
        token,
        {
            "model_revision_id": "pump-model-r1",
            "product_id": "prod-pump",
            "model_version": "1.0.0",
            "properties": r1_props,
        },
        now_ms=11_000,
    )
    client.request_json("POST", "/v1/models/pump-model-r1:validate", token, {}, now_ms=11_500)
    client.request_json(
        "POST",
        "/v1/models/pump-model-r1:publish",
        token,
        {"published_at_ms": 12_000},
        now_ms=12_000,
    )

    r2_props = r1_props + [
        {
            "identifier": "motor_current",
            "name": "电机电流",
            "value_type": "float64",
            "unit_code": "A",
            "unit_symbol": "A",
            "min_value": 0.0,
            "max_value": 200.0,
            "required": False,
        }
    ]
    client.request_json(
        "POST",
        "/v1/products/prod-pump/models",
        token,
        {
            "model_revision_id": "pump-model-r2",
            "product_id": "prod-pump",
            "model_version": "1.1.0",
            "properties": r2_props,
        },
        now_ms=13_000,
    )
    client.request_json(
        "POST",
        "/v1/models/pump-model-r2:validate",
        token,
        {"previous_published_revision_id": "pump-model-r1"},
        now_ms=13_500,
    )
    client.request_json(
        "POST",
        "/v1/models/pump-model-r2:publish",
        token,
        {"published_at_ms": 14_000},
        now_ms=14_000,
    )

    # 3. Assets & Cross-Tree Relation
    client.request_json(
        "POST",
        "/v1/assets",
        token,
        {"asset_id": "asset-plant-a", "parent_asset_id": None, "slug": "plant-a", "name": "一厂区"},
        now_ms=15_000,
    )
    client.request_json(
        "POST",
        "/v1/assets",
        token,
        {
            "asset_id": "asset-room-1",
            "parent_asset_id": "asset-plant-a",
            "slug": "room-1",
            "name": "一号泵房",
        },
        now_ms=15_100,
    )
    client.request_json(
        "POST",
        "/v1/assets",
        token,
        {
            "asset_id": "asset-room-2",
            "parent_asset_id": "asset-plant-a",
            "slug": "room-2",
            "name": "二号泵房",
        },
        now_ms=15_200,
    )
    client.request_json(
        "POST",
        "/v1/assets",
        token,
        {
            "asset_id": "asset-header-north",
            "parent_asset_id": "asset-plant-a",
            "slug": "header-north",
            "name": "北路总管",
        },
        now_ms=15_300,
    )
    client.request_json(
        "POST",
        "/v1/assets/relations",
        token,
        {
            "relation_id": "rel-room1-feeds-north",
            "from_asset_id": "asset-room-1",
            "relation_type": "FEEDS",
            "to_asset_id": "asset-header-north",
            "valid_from_ms": 16_000,
            "valid_to_ms": None,
            "source": "process_topology",
        },
        now_ms=16_000,
    )

    # 4. Device pump-001
    client.request_json(
        "POST",
        "/v1/devices",
        token,
        {
            "device_id": "pump-001",
            "device_key": "pump-001",
            "device_name": "1号增压主泵",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "asset_id": "asset-room-1",
        },
        now_ms=18_000,
    )

    # 5. Source Grant & Active Binding
    client.request_json(
        "POST",
        "/v1/sources/grants",
        token,
        {"source_id": "gw-01", "device_id": "pump-001", "valid_from_ms": 20_000},
        now_ms=20_000,
    )
    client.request_json(
        "POST",
        "/v1/devices/pump-001/bindings",
        token,
        {
            "binding_revision_id": "bind-pump-001-r1",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r1",
            "source_id": "gw-01",
            "normalization_owner": "gateway",
            "server_scale_factor": None,
            "points": [
                {
                    "property": "outlet_pressure",
                    "register_address": "40001",
                    "factor": 0.01,
                    "unit_code": "MPa",
                    "deadband": 0.02,
                    "heartbeat_ms": 60_000,
                }
            ],
            "static_points": [
                {
                    "property": "rated_power",
                    "value": 55.0,
                    "unit_code": "kW",
                }
            ],
        },
        now_ms=21_000,
    )
    client.request_json(
        "POST",
        "/v1/bindings/bind-pump-001-r1:validate",
        token,
        {},
        now_ms=22_000,
    )
    client.request_json(
        "POST",
        "/v1/bindings/bind-pump-001-r1:activate",
        token,
        {
            "expected_device_revision": 1,
            "activated_at_ms": 23_000,
            "gateway_ack": None,
            "manual_stop_window": {
                "window_start_ms": 22_500,
                "window_end_ms": 23_500,
                "operator_id": "op-apifox",
            },
        },
        now_ms=23_000,
    )

    # 6. Initial Telemetry Sample (1.65 MPa -> ing-000001)
    st_tel, tel_rec = client.request_json(
        "POST",
        "/v1/telemetry",
        token,
        {
            "spec": "iotfast.telemetry.v1",
            "message_id": "msg-seed-001",
            "source_id": "gw-01",
            "source_epoch": "epoch-01",
            "device_id": "pump-001",
            "model_revision_id": "pump-model-r1",
            "binding_revision_id": "bind-pump-001-r1",
            "points": [
                {
                    "property": "outlet_pressure",
                    "value": 1.65,
                    "source_at_ms": 25_000,
                    "source_time_origin": "device_clock",
                    "source_seq": "1",
                    "quality": {
                        "state": "good",
                        "source_protocol": "modbus",
                        "source_code": None,
                    },
                    "delivery": {
                        "replayed": False,
                    },
                }
            ],
        },
        now_ms=25_005,
    )
    if st_tel not in (200, 202):
        raise RuntimeError(f"Failed to seed initial telemetry sample (HTTP {st_tel}): {tel_rec}")

    # 7. Stage 6 Edge Adapter Capability & Derived Metric Definition
    st_adp, adp_rec = client.request_json(
        "POST",
        "/v1/adapters",
        token,
        {
            "adapter_id": "adapter-modbus-gw01",
            "protocol": "modbus_tcp",
            "supported_value_types": ["float64", "int64", "bool", "text"],
            "supported_transforms": ["gateway_normalized", "linear_scale"],
            "max_batch_size": 500,
            "supports_deadband": True,
        },
        now_ms=25_500,
    )
    if st_adp not in (200, 201, 404):
        raise RuntimeError(f"Failed to seed edge adapter (HTTP {st_adp}): {adp_rec}")

    st_dm, dm_rec = client.request_json(
        "POST",
        "/v1/derived-metrics",
        token,
        {
            "metric_id": "metric-specific-power",
            "name": "单位压力功率比",
            "product_id": "prod-pump",
            "model_revision_id": "pump-model-r1",
            "expression": "rated_power / outlet_pressure",
            "input_properties": ["rated_power", "outlet_pressure"],
            "output_unit_code": "kW_per_MPa",
            "quality_policy": "reject_on_bad",
        },
        now_ms=25_600,
    )
    if st_dm not in (200, 201, 404):
        raise RuntimeError(f"Failed to seed derived metric (HTTP {st_dm}): {dm_rec}")

    # 8. D10.1 Consistent Snapshot Backup (bkp-seed-001)
    st_bkp, bkp_rec = client.request_json(
        "POST",
        "/v1/storage/backups",
        token,
        {"backup_id": "bkp-seed-001"},
        now_ms=25_800,
    )
    if st_bkp not in (200, 201, 404, 409):
        raise RuntimeError(f"Failed to seed storage backup bkp-seed-001 (HTTP {st_bkp}): {bkp_rec}")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Start local iotfast_server with pre-seeded demo data for Apifox live debugging"
    )
    parser.add_argument("--port", type=int, default=8080, help="TCP port to listen on (default: 8080)")
    parser.add_argument(
        "--data-dir",
        default="",
        help="Optional persistent data directory (defaults to temporary directory)",
    )
    parser.add_argument(
        "--seed",
        dest="seed",
        action="store_true",
        default=True,
        help="Pre-populate demo product, model, assets, pump-001, binding, and telemetry (default: True)",
    )
    parser.add_argument(
        "--no-seed",
        dest="seed",
        action="store_false",
        help="Start with an empty database without seeding demo data",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Start server on ephemeral port, seed data, verify view & storage status, print token, and exit 0",
    )
    args = parser.parse_args()

    bin_path = PROTOTYPE_DIR / "target" / "debug" / "iotfast_server"
    duckdb_lib_dir = PROTOTYPE_DIR / "target" / "duckdb-download" / "aarch64-apple-darwin" / "v2.0.0"

    env = os.environ.copy()
    if duckdb_lib_dir.exists():
        env["DYLD_LIBRARY_PATH"] = f"{duckdb_lib_dir}:{env.get('DYLD_LIBRARY_PATH', '')}"

    if not bin_path.exists():
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--manifest-path",
                str(PROTOTYPE_DIR / "Cargo.toml"),
                "--bin",
                "iotfast_server",
            ],
            check=True,
            env=env,
        )

    listen_port = 0 if args.self_test else args.port
    listen_addr = f"127.0.0.1:{listen_port}"
    token = build_apifox_universal_token(DEFAULT_SECRET)

    tmp_ctx = tempfile.TemporaryDirectory(prefix="iotfast-apifox-live-") if not args.data_dir else None
    data_dir = args.data_dir if args.data_dir else tmp_ctx.name  # type: ignore[union-attr]

    cmd = [
        str(bin_path),
        "--listen",
        listen_addr,
        "--data-dir",
        str(data_dir),
        "--auth-secret",
        DEFAULT_SECRET,
    ]
    if args.self_test:
        cmd.append("--stdio-stream")

    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE if args.self_test else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )

    try:
        assert proc.stdout is not None
        startup_line = proc.stdout.readline().strip()
        if not startup_line:
            err_out = proc.stderr.read() if proc.stderr else ""
            raise RuntimeError(f"iotfast_server exited before startup banner: {err_out}")
        info = json.loads(startup_line)
        actual_addr = info.get("addr", listen_addr)
        _, actual_port_str = actual_addr.split(":")
        actual_port = int(actual_port_str)
        if actual_port == 0 and not args.self_test:
            raise RuntimeError(
                f"TCP port {args.port} binding was blocked (e.g. sandbox restriction). Run outside sandbox to listen on 127.0.0.1:{args.port}."
            )
        display_port = actual_port if actual_port != 0 else args.port
        base_url = f"http://127.0.0.1:{display_port}"

        client = LocalServerClient(proc, info["base_url"], actual_port)

        if args.seed:
            seed_demo_data(client, token)

        if args.self_test:
            st_view, view = client.request_json(
                "GET", "/v1/devices/pump-001/view", token, now_ms=26_000
            )
            assert st_view == 200, f"GET /v1/devices/pump-001/view failed ({st_view}): {view}"
            assert view["device_id"] == "pump-001"
            assert view["properties"]["outlet_pressure"]["value"] == 1.65
            assert view["properties"]["rated_power"]["value"] == 55.0

            st_storage, storage = client.request_json(
                "GET", "/v1/storage/status", token, now_ms=26_000
            )
            assert st_storage == 200, f"GET /v1/storage/status failed ({st_storage}): {storage}"
            assert storage["business_metadata_store"]["engine"] == "sqlite"
            assert storage["device_timeseries_store"]["active_backend"] == "duckdb"

            st_plan, plan = client.request_json(
                "POST",
                "/v1/storage/query-plan",
                token,
                {
                    "resource_ids": ["res-pump-001-outlet-pressure"],
                    "start_ms": 0,
                    "end_ms": 4_102_444_800_000,
                    "bucket_ms": 60_000,
                },
                now_ms=26_000,
            )
            assert st_plan == 200 and plan["mode"] == "aggregate", f"Query plan check failed: {plan}"

            st_eval, eval_res = client.request_json(
                "POST",
                "/v1/derived-metrics/metric-specific-power:evaluate",
                token,
                {"device_id": "pump-001", "evaluated_at_ms": 26_000},
                now_ms=26_000,
            )
            if st_eval == 200:
                assert eval_res["metric_id"] == "metric-specific-power"
                assert eval_res["quality"] == "good"

            st_caps, caps_res = client.request_json(
                "GET",
                "/v1/storage/capabilities",
                token,
                now_ms=26_000,
            )
            if st_caps == 200:
                assert caps_res["active_backend"] == "duckdb"
                assert "backends" in caps_res

            st_ckpt, ckpt_res = client.request_json(
                "GET",
                "/v1/storage/checkpoints?backend=duckdb",
                token,
                now_ms=26_000,
            )
            if st_ckpt == 200:
                assert ckpt_res["target_backend"] == "duckdb"
                assert "has_unreconciled_holes" in ckpt_res

            st_bkp_v, bkp_v_res = client.request_json(
                "POST",
                "/v1/storage/backups/bkp-seed-001:verify",
                token,
                {},
                now_ms=26_000,
            )
            if st_bkp_v == 200:
                assert bkp_v_res["backup_id"] == "bkp-seed-001"
                assert bkp_v_res["verified"] is True

            print("================================================================================")
            print("IoTFast Apifox Local Live Server Self-Test: PASS")
            print(f"  Seeded Device View : pump-001 (outlet_pressure={view['properties']['outlet_pressure']['value']} MPa, rated_power={view['properties']['rated_power']['value']} kW)")
            print(f"  Storage Status     : SQLite ({storage['business_metadata_store']['journal_mode']}) + {storage['device_timeseries_store']['active_backend']}")
            if st_caps == 200 and st_ckpt == 200:
                print(f"  Storage Checkpoint : backend={ckpt_res['target_backend']}, contiguous_wal_pos={ckpt_res.get('contiguous_checkpoint_wal_pos')}, holes={len(ckpt_res.get('holes', []))}")
            if st_bkp_v == 200:
                print(f"  Backup Verified    : {bkp_v_res['backup_id']} (files={bkp_v_res['checked_files']}, devices={bkp_v_res['restored_devices']}, history={bkp_v_res['restored_history_observations']})")
            if st_eval == 200:
                print(f"  Derived Metric     : metric-specific-power = {eval_res['computed_value']} {eval_res['unit_code']}")
            print(f"  Apifox Bearer Token: {token}")
            print("================================================================================")
            return 0

        print("================================================================================")
        print("IoTFast 物模型与 UNS 数据管理引擎 — Apifox 本地联调服务已启动")
        print("================================================================================")
        print(f"1. 本地服务地址 (Base URL) : {base_url}")
        print(f"2. WebSocket 订阅地址      : ws://127.0.0.1:{display_port}/v1/subscriptions")
        print(f"3. 数据目录 (Data Dir)     : {data_dir}")
        print(f"4. 预置演示数据            : {'已注入 (prod-pump / pump-model-r1 / asset-room-1 / pump-001 / 1.65 MPa)' if args.seed else '未注入 (--no-seed)'}")
        print("--------------------------------------------------------------------------------")
        print("请将以下 Bearer Token 复制到 Apifox【根目录 -> Auth -> Bearer Token】：")
        print("")
        print(token)
        print("")
        print("--------------------------------------------------------------------------------")
        print("按 Ctrl+C 停止本地联调服务...")

        def handle_sigint(_sig: int, _frame: Any) -> None:
            proc.terminate()

        signal.signal(signal.SIGINT, handle_sigint)
        signal.signal(signal.SIGTERM, handle_sigint)
        while proc.poll() is None:
            time.sleep(0.2)
        return 0
    finally:
        if proc.poll() is None:
            if args.self_test and proc.stdin is not None:
                try:
                    proc.stdin.write(json.dumps({"op": "shutdown", "cid": 9999}) + "\n")
                    proc.stdin.flush()
                except Exception:
                    pass
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
        if tmp_ctx is not None:
            tmp_ctx.cleanup()


if __name__ == "__main__":
    sys.exit(main())
