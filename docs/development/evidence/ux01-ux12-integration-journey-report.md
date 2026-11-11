# U01–U08 首次接入旅程与 UX01–UX12 数据状态呈现矩阵 API 事实回归验证报告

- **任务 ID**：`UX-JOURNEY`
- **日期**：2026-11-10
- **规范基线**：
  - `docs/product/integration-experience.md`（2026-11-10 设计基线：第 2 节 `U01–U08` 首次接入旅程、第 4 节 `UX01–UX12` 数据状态呈现矩阵、第 5 节交互/错误/重试契约、第 7 节可用性验收与交接）
  - `docs/specifications/northbound-api-and-subscription-contract.md`
  - `docs/specifications/p0-telemetry-contract.md`
  - `docs/specifications/service-identity-and-scope-contract.md`
  - `docs/development/acceptance.md`
- **验证类型声明（职责边界）**：
  - **本报告属于引擎 API 事实层（HTTP/1.1 REST + RFC 6455 WebSocket 客户端）自动化回归测试**，验证引擎在 `U01–U08` 首次接入旅程与 `UX01–UX12` 全部 12 类数据状态下的结构化输出事实、错误码、字段路径及禁止行为防护。
  - **上层管理台页面交互与视觉可用性边界**：根据 `docs/product/integration-experience.md` 第 1、3、7 节与 `docs/development/acceptance.md` 的规定，P0 引擎不交付前端页面工程；上层管理台页面级验收（如键盘走完向导、非颜色视觉标识并存、窄屏/缩放不丢质量与时间列、浏览器端表单状态恢复等）由上层平台项目单独执行与签收，在本报告第 4 节中显式标记为**“未执行（待上层项目页面单独签收）”**，绝不以引擎 API 事实测试或文档审查冒充页面可用性测试。
- **修改文件范围（Write Scope）**：
  - `prototype/tests/ux_journey_and_matrix_test.rs`
  - `docs/development/evidence/ux01-ux12-integration-journey-report.md`
- **执行命令与退出码**：
  ```bash
  cargo test --locked --manifest-path prototype/Cargo.toml --test ux_journey_and_matrix_test -- --nocapture
  # 退出码: 0 (1 passed; 0 failed)
  ```

---

## 1. U01–U08 首次接入旅程 API 事实对账表

测试入口：[ux_journey_and_matrix_test.rs](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L83-L995)

| 旅程步骤 | 前置条件与测试输入 | 预期 API 事实（正例 + 失败防护） | 实际验证结果 | 状态 |
|---|---|---|---|---|
| **U01 环境与身份** | 启动 `ServerHandle::spawn`；调用 `GET /health`；分别以无 Token、伪造签名 Token、携带客户端自报 `X-Role: admin` / `X-Tenant-Id: ten-hijack` 请求 `GET /v1/products` | 1. `/health` 返回 `200 OK`、`state="Running"`、`healthy=true`、`duckdb_version="v2.0.0"`、`data_dir_writable=true`、`wal_dir_writable=true`。<br>2. 缺失或伪造签名 Token 拒绝为 `401 UNAUTHENTICATED`。<br>3. 客户端自报 `X-Role` / `X-Tenant-Id` 拒绝为 `422 VALIDATION_FAILED`（`CLIENT_REPORTED_SCOPE_FORBIDDEN`）。 | 全部断言一致通过 | **通过** |
| **U02 发布模型** | 使用授权 `ManagementService` 身份创建产品 `prod-temp-sensor`；先提交 `min_value(100.0) > max_value(-40.0)` 的非法草稿；再创建合法草稿 `temp-model-r1`（含 `temperature`、`calibration_offset`、`maintenance_tag`）、`:validate`、`:publish`；重复 `:publish`；创建兼容扩展版本 `temp-model-r2` 并调用 `POST /v1/models:compare` | 1. 非法草稿返回 `422 VALIDATION_FAILED` 并指出 `min_value > max_value`。<br>2. 合法草稿校验返回 `200 OK`（`valid=true`、`compatibility="initial"`），发布返回 `200 OK`（`status="published"`、`model_hash="sha256:..."`）。<br>3. 对已发布版本再次发布返回 `409 REVISION_CONFLICT`（不可变性保护）。<br>4. `:compare` 返回 `compatibility="compatible_extension"`、`added_properties=["ambient_humidity"]`。 | 全部断言一致通过 | **通过** |
| **U03 实例化** | 创建资产树 `asset-plant-1` -> `asset-zone-a` / `asset-zone-b`；在 `asset-zone-a` 实例化两台同模型设备 `sensor-a01` 与 `sensor-a02`，在 `asset-zone-b` 实例化 `sensor-b01`；尝试在同一资产下重复创建相同 `device_key="sensor-a01"` | 1. 两台设备均返回 `201 Created`，各自生成稳定且互不串 ID 的 `resource_id`（`res-sensor-a01-temperature` 与 `res-sensor-a02-temperature` 集合零交集）。<br>2. 同资产路径冲突请求拒绝为 `409 REVISION_CONFLICT`，不篡改已有设备。 | 全部断言一致通过 | **通过** |
| **U04 来源绑定** | 授予 `gw-ux-01` 对 `sensor-a01` / `sensor-a02` 的采集授权；创建包含 `gateway_measurement`（`temperature`）与 `static_config`（`calibration_offset=0.5`）的绑定 `bind-a01-r1`；执行 `:validate` 后先查 `DeviceView`；随后携带匹配 `config_hash` 的 `gateway_ack`（及 `sensor-a02` 的 `manual_stop_window`）执行 `:activate` | 1. `:validate` 返回 `200 OK`（`status="ready"`、`config_hash`），此时 `DeviceView.active_binding_revision_id` 仍为 `null`（校验通过不等于已激活）。<br>2. `:activate` 校验 `config_hash` / `manual_stop_window` 后返回 `status="active"`。<br>3. 激活绑定不伪造实时测量值（`temperature` 仍为 `no_observation` + `null`）。 | 全部断言一致通过 | **通过** |
| **U05 首次上报** | 1. 发送含 2 个点位的遥测批次（第 1 点合法 `23.5`，第 2 点为未知属性 `nonexistent_point`）。<br>2. 发送合法遥测 `msg-u05-first-ok`（`temperature=23.5`、`source_at_ms=100000`、`Prefer: durability=durable`）。 | 1. 含单点非法的批次整批返回 `422 VALIDATION_FAILED`，且 `sensor-a01` 的 `DeviceView` 保持 `no_observation` + `value=null`（零副作用）。<br>2. 合法上报返回 `202 Accepted`、`status="accepted_durable"`、非空 `ingest_id` 与 `wal_position`。 | 全部断言一致通过 | **通过** |
| **U06 查看与订阅** | 查询 `GET /v1/devices/sensor-a01/view?now_ms=105000`；建立 RFC 6455 WebSocket 连接 `GET /v1/subscriptions` 并发送 `subscribe`（`include_initial_snapshot=true`）；再上报 `msg-u06-second-ok`（`value=24.2`、`source_at_ms=106000`） | 1. `DeviceView` 返回 `value=23.5`、`unit_code="Cel"`、`quality="good"`、`freshness="fresh"`。<br>2. WebSocket 严格按序先收到 `snapshot_begin(S) -> snapshot_chunk(value=23.5) -> snapshot_end(S)`。<br>3. 新上报到达后立即收到 `delta` 帧（`value=24.2`、`stream_seq > S`）。 | 全部断言一致通过 | **通过** |
| **U07 历史确认** | 先调用 `GET /v1/ingestions/{first_ingest_id}` 确认状态；再以半开区间 `[100000, 106000)` 调用 `POST /v1/history/query` | 1. `/v1/ingestions/{id}` 返回 `200 OK`、`status="accepted_durable"`、`history_status="committed"`。<br>2. 半开区间 `[100000, 106000)` 精确返回 `source_at_ms=100000` 的 1 条记录（严格排除右边界 `106000`），且携带原 `model_revision_id="temp-model-r1"`、`binding_revision_id="bind-a01-r1"`、`unit_code="Cel"`。 | 全部断言一致通过 | **通过** |
| **U08 异常演练** | 依次演练：<br>1. 同 `(source_id, source_epoch, message_id)` + 同 payload 重试；<br>2. 同 key + 篡改 payload；<br>3. 乱序旧点回填（`source_at_ms=130000 < latest 166000`、`replayed=true`）；<br>4. 点位坏质量与网关断线恢复；<br>5. 订阅者凭证过期（`observed_at_ms > expires_at_ms`） | 1. 同 key 重试返回 `200 OK`、`status="duplicate"` 及原 `ingest_id`。<br>2. 同 key 异 payload 返回 `409 IDEMPOTENCY_CONFLICT`。<br>3. 乱序回填写入历史（`202`），但绝不覆盖较新的 `latest_value`（保持 `25.5 @ 166000`）。<br>4. 凭证过期时 WebSocket 收到 `subscription_terminated`（`reason="CREDENTIAL_EXPIRED"`、`action="reauthenticate_and_resubscribe"`）。 | 全部断言一致通过 | **通过** |

---

## 2. UX01–UX12 数据状态呈现矩阵 API 事实逐项验证表

| 验收编号 | 数据状态场景 | 规范要求：必须呈现的 API 事实 | 规范要求：禁止呈现的防护断言 | 对应测试断言位置与结果 | 状态 |
|---|---|---|---|---|---|
| **UX01** | 从未上报 | `observation_state="no_observation"`、`value=null`、`quality="unknown"`、`freshness="unknown"`；静态点位区分 `binding_state="bound_static"`，未绑定点位区分 `binding_state="unbound"` | 严禁将缺失值返回为 `0`、`false` 或将状态标为正常观测 | `sensor-a01` 首次上报前及 `sensor-a02` 全程验证 `value.is_null() == true`（[L497-L547](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L497-L547)） | **通过** |
| **UX02** | `good + fresh` | 同时返回 `value`（`23.5`）、`unit_code`（`"Cel"`）、`unit_symbol`（`"°C"`）、`quality="good"`、`freshness="fresh"`、`value_sampled_at_ms=100000` | 严禁丢失单位、质量或采样时间戳 | `now_ms=105000` 查询 `sensor-a01` 视图逐字段断言通过（[L638-L654](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L638-L654)） | **通过** |
| **UX03** | `good + stale` | 超过 `stale_threshold_ms`（`30000ms`）后，`freshness="stale"`、`quality="good"`，保留最后有效 `value=24.2` 与原采样时间 `value_sampled_at_ms=106000` | 严禁把旧值采样时间篡改为当前查询时间（`150000`） | `now_ms=150000` 查询验证 `freshness=="stale"` 且 `value_sampled_at_ms==106000`（[L782-L804](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L782-L804)） | **通过** |
| **UX04** | 点位 `bad / uncertain` | `source_state` 上报点位 `bad` 后，返回 `quality="bad"`、`quality_reason="sensor_open_circuit"`、`freshness="stale"`，同时保留最后有效测量 `value=24.2`（`value_sampled_at_ms=106000`）与质量事件时间 `quality_observed_at_ms=155000` | 严禁因网关重连（`connection_state="connected"`、`point_states=[]`）自动把点位 `quality` 恢复为 `"good"` | 验证点位 `bad` 保留最后测量值，且网关重连后点位 `quality` 仍为 `"bad"`，直到新有效测量到达才恢复（[L809-L905](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L809-L905)） | **通过** |
| **UX05** | 网关 `disconnected` | 网关上报 `connection_state="disconnected"`（无点位故障）时，设备视图 `source_connection_state="disconnected"`、点位 `freshness="stale"` | 无点位故障证据时，严禁把所有测量点位的 `quality` 篡改为 `"bad"` | 验证 `source_connection_state=="disconnected"`、`freshness=="stale"` 且 `quality=="good"`（[L910-L962](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L910-L962)） | **通过** |
| **UX06** | 未来时间异常 | `source_at_ms (300000) > now_ms (165000) + max_future_skew_ms (60000)` 时拒绝为 `422 CLOCK_SKEW_EXCEEDED`，返回 `request_id` 与字段路径 `points[0].source_at_ms` | 严禁异常未来时钟刷新 `latest_value` | 验证返回 `422 CLOCK_SKEW_EXCEEDED`，且 `latest` 保持 `25.0 @ 160000`（[L967-L1012](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L967-L1012)） | **通过** |
| **UX07** | `accepted_volatile` | `Prefer: durability=volatile` 上报返回 `202` 且 `status="accepted_volatile"`，`GET /v1/ingestions/{id}` 亦返回 `status="accepted_volatile"` | 严禁将易失接收伪标为 `"accepted_durable"` | 验证响应与 `/v1/ingestions/{id}` 状态均为 `"accepted_volatile"`（[L1017-L1060](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L1017-L1060)） | **通过** |
| **UX08** | `accepted_durable + history pending` | `GET /v1/ingestions/{id}` 显式分离 `status`（`"accepted_durable"`）与 `history_status`（`"pending"` / `"committed"`） | 严禁在 `history_status="pending"` 时宣称历史已可查询或将 durable ACK 等同于历史已落库 | 验证 `status="accepted_durable"` 与 `history_status="pending"` / `"committed"` 独立表达（[L718-L751](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L718-L751)） | **通过** |
| **UX09** | 历史为空且查询成功 | 查询已授权但无历史观测的 `res-sensor-a02-temperature`，返回 HTTP `200 OK`、`items=[]`、`partial=false`、`known_gaps=[]` | 严禁将空结果报错为 `404` / `500` 或伪造默认数据点 | 验证返回 `200 OK`、`items=[]`、`partial=false`（[L1065-L1084](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L1065-L1084)） | **通过** |
| **UX10** | 历史 `partial` / 覆盖未知 | 存在重叠 `delivery_gap` 时返回 `200 OK`、保留已有 4 条观测、`partial=true`、`known_gaps` 列出缺口原因，`coverage.event_time_completeness="unknown"`、`event_time_complete_through_ms=null` | 严禁因存在 `contiguous_wal_position` 而宣称事件时间完整（`complete`），严禁补零插值掩盖缺口 | 验证 `partial==true`、`known_gaps.len()==1`、`event_time_completeness=="unknown"`（[L1089-L1130](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L1089-L1130)） | **通过** |
| **UX11** | 订阅重连 / `resync` | 慢消费超出 `max_pending_events=2` 时立即推送 `resync_required`（`reason="BUFFER_OVERFLOW"`、`action="reconnect_and_resubscribe_with_snapshot"`）；重连后重新执行 `snapshot_begin -> snapshot_chunk -> snapshot_end` 完整快照替换 | 严禁静默丢弃增量帧或将半个快照当作完整状态 | 验证 WebSocket 溢出收到 `resync_required(BUFFER_OVERFLOW)` 并完成重连全量快照恢复（[L1217-L1275](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L1217-L1275)） | **通过** |
| **UX12** | 无权限或不存在 | 使用仅授权 `asset-zone-a` 的 Token 访问真实存在但越权的 `sensor-b01`、不存在的 `sensor-does-not-exist`、以及混有越权 `resource_id` 的批量历史查询，统一返回 `404 RESOURCE_NOT_FOUND` 且 `details=[]`；资产树目录列表自动过滤越权节点 | 严禁返回 `403` 暴露越权资源真实存在、名称或未授权资源数量 | 三类越权/不存在场景均返回一致的 `404 RESOURCE_NOT_FOUND` 且 `details=[]`（[L1314-L1372](file:///Users/fengsl/code/work/iotfast-thing-engine/prototype/tests/ux_journey_and_matrix_test.rs#L1314-L1372)） | **通过** |

---

## 3. 运行环境与执行证据

- **操作系统**：macOS (`darwin`)
- **DuckDB 引擎版本**：`v2.0.0`（内嵌绑定编译）
- **测试执行输出**：
  ```text
  running 1 test
  test u01_to_u08_integration_journey_and_ux01_to_ux12_state_matrix_verification ... ok

  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.68s
  ```

---

## 4. 未覆盖项与上层交接清单（按规范第 7 节如实标记）

| 验收类别 | 状态 | 说明 |
|---|---|---|
| 引擎 API / WS 事实契约（`U01–U08` & `UX01–UX12`） | **通过** | 已由 `prototype/tests/ux_journey_and_matrix_test.rs` 全链路 HTTP/1.1 + RFC 6455 WebSocket 实测验证通过。 |
| 上层管理台页面视觉、键盘导航、窄屏/缩放列保留、浏览器端向导状态恢复 | **未执行（待上层项目单独签收）** | 本仓库为无前端页面的数据引擎；根据 `docs/product/integration-experience.md` 第 7 节，页面可用性测试由上层管理台项目在具备 UI 实现后单独执行与签收。 |
| 现场控制闭环（人工确认、联锁、物理反馈） | **不适用（非引擎范围）** | P0 数据引擎严格只读，不提供任何设备控制接口。 |
