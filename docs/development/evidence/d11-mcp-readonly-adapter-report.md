# D11 北向只读 SDK / MCP 工具适配层与不可信 AI 输入安全边界验证报告

- **任务编号**：`D11`（上层只读 SDK / MCP 工具适配与不可信 AI 输入防线）
- **验证日期**：2026-11-10
- **合规依据**：`AGENTS.md`（“首个闭环只读，设备控制不能自动开启；引擎必须校验服务身份与可信数据范围并拒绝不可信调用。AI 的文字和工具输入属于不可信数据”）、`docs/development/roadmap.md`（D11 / D12 边界）、`docs/architecture/design-specification.md`（§12 / §22.1 / §24 / §25）
- **变更文件范围（Write Scope）**：
  - `scripts/iotfast_mcp_readonly_adapter.py`
  - `prototype/tests/d11_northbound_mcp_boundary_test.rs`
  - `docs/development/evidence/d11-mcp-readonly-adapter-report.md`

---

## 1. 架构与只读安全约束说明

1. **严格只读闭环与零控制能力（D11 / D12 边界）**：
   - `IoTFastReadOnlyMcpAdapter` 仅向北向 AI / 应用暴露 4 个只读工具（`readOnlyHint: true, destructiveHint: false`）：
     - `list_uns_resources`（对应 `GET /v1/resources`）
     - `get_device_business_view`（对应 `GET /v1/devices/{device_id}/view`）
     - `query_resource_history`（对应 `POST /v1/history/query`）
     - `list_asset_topology`（对应 `GET /v1/assets` 与 `GET /v1/assets/{asset_id}/relations`）
   - 任何写操作或设备控制类工具调用（如 `write_point`、`control_device`、`send_command`、`prepare_command`、`submit_command` 等）在适配层入口即被无条件拦截，返回 `CONTROL_NOT_SUPPORTED_IN_READONLY_ENGINE`，绝不触发任何下行报文或状态变更。
   - 服务端 `iotfast_server` 不内置任何控制路由（如 `/v1/devices/{id}/commands`、`/v1/control`），且对仅持北向只读 Scope 令牌（`view:read`、`history:query`）的调用方尝试访问任何写接口（`POST /v1/models`、`POST /v1/devices`、`POST /v1/telemetry` 等）一律返回 `403 FORBIDDEN_ACTION`。

2. **AI 不可信输入防线（Untrusted AI Input Defense）**：
   - **禁止身份与 Scope 越权注入**：适配层递归扫描工具参数，拒绝任何尝试传入或覆盖 `tenant_id`、`project_id`、`X-Scope`、`X-Tenant-Id`、`scope_filter`、`allowed_actions`、`action`、`caller_service_id`、`operator_id`、`token`、`headers` 的调用，返回 `UNTRUSTED_AI_SCOPE_INJECTION_REJECTED`；服务端同时强制执行 `SC02` 请求头与请求体租户防篡改校验（`422 VALIDATION_FAILED`）。
   - **禁止路径遍历与注入字符**：对 `device_id`、`asset_id`、`resource_ids` 强制执行白名单正则 `^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$` 与危险模式拦截（`..`、`/`、`\`、`;`、`'`、`"`、`--`、`/*`、`*/`、`\x00`、`\r`、`\n`）；对 `uri` / `uri_prefix` 强制校验 `^uns://[A-Za-z0-9._/-]{1,512}$` 并对 URL 查询参数做严格百分号编码，杜绝路径穿越与 SQL / 参数注入（返回 `UNTRUSTED_AI_INPUT_REJECTED`）。

3. **质量、新鲜度、时间戳与绑定状态完整保留（§22.1 / §25.1）**：
   - `get_device_business_view` 原样保留服务端返回的 `quality`、`quality_reason`、`freshness`、`value_sampled_at_ms`、`source_at_ms`、`quality_observed_at_ms`、`received_at_ms`、`binding_state`、`observation_state` 元数据。
   - 绝不将 `freshness="stale"`、`quality="bad"`、`quality="unknown"` 或 `value=null`（未绑定/无观测）简化或伪装为实时正常值，并在每个属性视图上附加 `ai_interpretation_guard`（`is_realtime_normal` 与显式告警提示），同时标注设备/属性名称与文本值属于不可信现场数据而非系统指令。

---

## 2. 验证命令与退出码

| 序号 | 验证项 | 执行命令 | 退出码 | 结果 |
|---|---|---|---|---|
| 1 | Rust 北向 AI Scope 与只读边界集成测试 | `cargo test --locked --manifest-path prototype/Cargo.toml --test d11_northbound_mcp_boundary_test` | `0` | 1 passed, 0 failed |
| 2 | Python 只读 MCP 适配层正反自检（兼容 TCP 与 `--stdio-stream`） | `python3 scripts/iotfast_mcp_readonly_adapter.py --self-test` | `0` | 6/6 阶段断言全部通过 |

---

## 3. AI 输入安全边界与只读契约测试矩阵

### 3.1 Rust 服务端边界测试矩阵（`prototype/tests/d11_northbound_mcp_boundary_test.rs`）

测试令牌配置：`caller_kind = NorthboundService`，`allowed_actions = ["view:read", "history:query"]`，`scope_filter.resource_ids = ["res-pump-001-outlet-pressure"]`。

| 测试场景 | 请求目标 | 预期行为 | 实际结果 |
|---|---|---|---|
| AI 令牌尝试创建物模型 | `POST /v1/models` | `403 FORBIDDEN_ACTION` | 通过（403） |
| AI 令牌尝试创建设备 | `POST /v1/devices` | `403 FORBIDDEN_ACTION` | 通过（403） |
| AI 令牌尝试伪造遥测入站 | `POST /v1/telemetry` | `403 FORBIDDEN_ACTION` | 通过（403） |
| AI 令牌尝试写产品 / 资产 / 绑定 | `POST /v1/products`, `/v1/assets`, `/v1/bindings` | `403 FORBIDDEN_ACTION` | 通过（403） |
| AI 令牌调用不存在的控制路由 | `POST /v1/devices/pump-001/commands`, `POST /v1/control`, `POST /v1/devices/pump-001/write` | `404 RESOURCE_NOT_FOUND` 且零副作用 | 通过（404，原观测值与版本未变） |
| 客户端注入越权 Scope 请求头（SC02） | `X-Tenant-Id`, `X-Scope`, `X-Allowed-Actions` | `422 VALIDATION_FAILED` | 通过（422） |
| 读取部分授权设备 `pump-001` 视图（R09） | `GET /v1/devices/pump-001/view` | `200 OK`，仅含 `outlet_pressure`；未授权属性 `motor_current` 与 `rated_power` 被裁剪 | 通过（仅返回 1 个授权属性） |
| 读取未授权设备 `pump-002` 视图 | `GET /v1/devices/pump-002/view` | `404 RESOURCE_NOT_FOUND`（隐匿存在性） | 通过（404） |
| 查询授权资源历史 vs 越权资源历史（R05） | `POST /v1/history/query` | `res-pump-001-outlet-pressure` 返回 `200`；混入 `motor_current` 或查询 `pump-002` 整批拒绝 `404 RESOURCE_NOT_FOUND` | 通过（200 / 404） |

### 3.2 Python MCP 适配层测试矩阵（`scripts/iotfast_mcp_readonly_adapter.py --self-test`）

| 测试类别 | 工具 / 输入样例 | 预期响应 / 错误码 | 实际结果 |
|---|---|---|---|
| 只读工具清单校验 | `list_tools()` | 仅暴露 4 个只读工具，全部标注 `readOnlyHint=True` | 通过 |
| UNS 资源列表与 Scope 过滤 | `list_uns_resources({"asset_id": "asset-plant-a", "include_descendants": True})` | 仅返回授权厂区下 `pump-001` 的 4 个资源，不含 `pump-002` | 通过 |
| 资产树与跨树拓扑查询 | `list_asset_topology({})` | 返回授权资产及 `FEEDS` 拓扑关系 | 通过 |
| 设备业务视图元数据保真（正常/静态/未绑定/陈旧/坏质量） | `get_device_business_view({"device_id": "pump-001"})` | 完整保留 `quality`、`freshness`、`source_at_ms`、`value_sampled_at_ms`、`binding_state`、`observation_state`；`stale`/`bad`/`unbound(null)` 均标记 `is_realtime_normal=False` | 通过 |
| 写 / 控制工具调用拦截 | `write_point`, `control_device`, `send_command`, `prepare_command`, `submit_command` | `ok=False`, `error.code="CONTROL_NOT_SUPPORTED_IN_READONLY_ENGINE"` | 通过 |
| 自定义 `tenant_id` / `X-Scope` / `action` 越权注入拦截 | `{"device_id": "pump-001", "tenant_id": "ten-hijack"}`, `{"X-Scope": "tenant_wide=true"}`, `{"action": "device:write"}` | `ok=False`, `error.code="UNTRUSTED_AI_SCOPE_INJECTION_REJECTED"` | 通过 |
| 路径遍历与 SQL 注入字符拦截 | `"../pump-002"`, `"pump-001/../../admin"`, `"pump-001; DROP TABLE device;--"`, `"uns://ten-8iic/plant-a/../../etc/passwd"` | `ok=False`, `error.code="UNTRUSTED_AI_INPUT_REJECTED"` | 通过 |

---

## 4. 已知边界与后续门禁说明

- 本报告仅证明本地受控闭环下 D11 北向只读 MCP 工具封装层与服务端 Scope / 只读防线通过自动化验证。
- 真实工业网关联调（D09）与目标硬件故障/容量验证（D10）仍待现场环境输入；设备控制（D12）不在本引擎交付范围内。
