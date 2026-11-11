# D00.3 机器契约（JSON Schema / OpenAPI 3.1）与正反样例自动验证证据报告

- **任务 ID**：`D00.3`（机器契约与 OpenAPI / JSON Schema 产物及自动验证）
- **状态**：已完成本地机器契约静态与向量验证（退出码 `0`）
- **关联决议**：`Q01–Q12`（见 [规范收敛、决策与现场输入台账](../specification-decisions.md)）、`C01–C05`
- **权威规范依据**：
  - [P0 遥测入站接口契约](../../specifications/p0-telemetry-contract.md)
  - [服务身份、可信数据范围与 D00 公共契约](../../specifications/service-identity-and-scope-contract.md)
  - [北向标准接口、实时订阅与历史查询契约](../../specifications/northbound-api-and-subscription-contract.md)
  - [`prototype/src/server/mod.rs`](../../../prototype/src/server/mod.rs)

---

## 1. 交付产物与 Write Scope 核对

本任务严格限制在 `D00.3` 专属 Write Scope 内，未修改任何 Rust 源码或共享配置文件：

| 序号 | 交付物路径 | 类型 | 覆盖契约与冻结决议对齐说明 |
|---|---|---|---|
| 1 | [`docs/specifications/schemas/telemetry-v1.schema.json`](../../specifications/schemas/telemetry-v1.schema.json) | JSON Schema (Draft 2020-12) | 严格定义 `iotfast.telemetry.v1` 请求体与 `$defs/IngestReceipt`、`$defs/IngestStatusRecord`：全层级 `additionalProperties: false`（Q03）、裸 JSON 标量值 `number \| integer \| boolean \| string`（拒绝 `null` 与 `TypedPointValue` 枚举包装，Q03/SC09）、南向 `source_time_origin` 仅允许 `["device_clock", "protocol_timestamp", "gateway_sample"]`（拒绝 `external_source`，Q03）、双维状态 `status` + `history_status`（Q12） |
| 2 | [`docs/specifications/schemas/source-state-v1.schema.json`](../../specifications/schemas/source-state-v1.schema.json) | JSON Schema (Draft 2020-12) | 严格定义 `iotfast.source_state.v1` 状态事件与回执：必填 `observed_at_ms`、`source_time_origin`、`source_seq`、`connection_state`、`point_states`、`delivery.replayed`（Q04/C02） |
| 3 | [`docs/specifications/schemas/device-view-v1.schema.json`](../../specifications/schemas/device-view-v1.schema.json) | JSON Schema (Draft 2020-12) | 严格定义 `DeviceBusinessView` 与 `$defs/DeviceShadowView`：覆盖 `binding_state`（`bound \| bound_static \| unbound`）、`observation_state`（`observed \| static_configured \| no_observation`）、`freshness`（`fresh \| stale \| static \| unknown`）及测量时间与质量故障时间分离字段（C02/NB03/NB04/R09） |
| 4 | [`docs/specifications/schemas/history-query-v1.schema.json`](../../specifications/schemas/history-query-v1.schema.json) | JSON Schema (Draft 2020-12) | 严格定义 `HistoryQueryRequest`（8 个必填顶层字段：`resource_ids, range, mode, aggregation, quality_states, order, limit, cursor`）与 `$defs/HistoryQueryResponse`（含 `coverage`、`known_gaps`、`items`、`buckets`、`next_cursor`，对齐 Q08/NB07–NB10） |
| 5 | [`docs/specifications/openapi-v1.json`](../../specifications/openapi-v1.json) | OpenAPI 3.1.0 | 完整覆盖 `iotfast_server` 全部 **49 条 HTTP/WS 路径**（**57 个操作**：`28 GET` + `29 POST`，**63 个组件 Schema**，对应 Apifox 目录 `01`–`11`：含容器与连通性探针 `/health`、`/v1/health`、`/healthz`、`/readyz`、`/v1/ping`、`/v1/ingest/ping`、控制面 `/v1/products`、`/v1/models*`、`/v1/assets*`、`/v1/devices*`、`/v1/sources/grants`、`/v1/bindings*`、边缘适配器 `/v1/adapters*`、衍生指标 `/v1/derived-metrics*`、存储分层与 `ST-02`/`ST-04`/`D10.1` `/v1/storage/status`、`/v1/storage/timeseries-adapter`、`/v1/storage/query-plan`、`/v1/storage/capabilities`、`/v1/storage/checkpoints`、`/v1/storage/checkpoints:reconcile`、`/v1/storage/backups`、`/v1/storage/backups/{backup_id}:verify`、南向 `/v1/telemetry`、`/v1/source-state`、双维入站状态 `GET /v1/ingestions/{ingest_id}`、北向 `/v1/resources`、`/v1/history/query`、`/v1/subscriptions`）以及 WebSocket 帧 Schema（严格区分 `WsSubscriptionTerminatedFrame` 与 `WsResyncRequiredFrame`，Q05） |
| 6 | [`scripts/verify-machine-contracts.py`](../../../scripts/verify-machine-contracts.py) | Python 3 标准库验证脚本 | 零第三方依赖，执行严格 JSON 重复 key 检查（Q03）、Draft 2020-12 Schema 结构校验、OpenAPI 49 路径 / 57 操作与 `$ref` 完整性校验、黄金向量正例校验及 Q01–Q12 / `SD-STORAGE-01` / `ST-02` / `ST-04` / `D10.1` 正反例拦截测试 |
| 7 | [`docs/development/evidence/d00-3-machine-contract-verification.md`](d00-3-machine-contract-verification.md) | 验证证据报告 | 本文档 |

---

## 2. 执行命令与原始输出

### 2.1 验证命令

```bash
python3 scripts/verify-machine-contracts.py
```

### 2.2 退出码与实际输出

- **退出码**：`0`
- **标准输出**：

```text
OK: Verified 4 JSON Schemas, 1 OpenAPI 3.1 contract (58 route/$ref checks), 7 golden vector checks, and 66 total positive/negative machine contract assertions.
```

---

## 3. 正反样例覆盖矩阵（对齐 Q01–Q12）

| 用例编号 | 目标 Schema / 契约 | 类别 | 输入特征 | 预期结果 | 实际结果 | 关联决议 / 向量 |
|---|---|---|---|---|---|---|
| `GV-01` .. `GV-07` | `telemetry-v1.schema.json` | 正例 | `f1_d00_contract` 黄金向量（单点/多点、`float64`/`int64`/`bool`/`text`、`-0.0`、`device_clock`/`gateway_sample`） | 通过 Schema 校验 | **PASS** | Q03, T01, SC04, SC05 |
| `POS-TEL-BASELINE` | `telemetry-v1.schema.json` | 正例 | 标准南向遥测报文（裸标量 `245.6`） | 通过 Schema 校验 | **PASS** | Q03, T01 |
| `NEG-TEL-01-UNKNOWN-TOP-FIELD` | `telemetry-v1.schema.json` | 反例 | 顶层注入未知字段 `"unknown_top_field": "forbidden"` | 拒绝（`additionalProperties: false`） | **PASS（已拦截）** | Q03 |
| `NEG-TEL-02-UNKNOWN-POINT-FIELD` | `telemetry-v1.schema.json` | 反例 | `points[0]` 注入未知字段 `"unit": "Cel"` | 拒绝（`additionalProperties: false`） | **PASS（已拦截）** | Q03 |
| `NEG-TEL-03-DUPLICATE-JSON-KEY` | Wire JSON 解析器 | 反例 | 同一 JSON 对象内包含重复 key `"spec"` | 拒绝（`DuplicateKeyError`） | **PASS（已拦截）** | Q03 |
| `NEG-TEL-04-ENUM-WRAPPED-VALUE` | `telemetry-v1.schema.json` | 反例 | `points[0].value` 传入内部枚举包装对象 `{"Float64": 245.6}` | 拒绝（只允许裸标量） | **PASS（已拦截）** | Q03 |
| `NEG-TEL-05-NULL-POINT-VALUE` | `telemetry-v1.schema.json` | 反例 | `points[0].value` 传入 `null` | 拒绝（SC09 禁止遥测 `null` 值） | **PASS（已拦截）** | Q03, SC09 |
| `NEG-TEL-06-EXTERNAL-SOURCE-ORIGIN` | `telemetry-v1.schema.json` | 反例 | 南向遥测 `source_time_origin` 传入 `"external_source"` | 拒绝（南向仅允许 `device_clock \| protocol_timestamp \| gateway_sample`） | **PASS（已拦截）** | Q03 |
| `NEG-TEL-07-LEADING-ZERO-SEQ` | `telemetry-v1.schema.json` | 反例 | `source_seq` 传入带前导零字符串 `"01001"` | 拒绝（违反 `^(0\|[1-9][0-9]{0,19})$`） | **PASS（已拦截）** | Q03, SC06 |
| `NEG-TEL-08-MISSING-BINDING-REV` | `telemetry-v1.schema.json` | 反例 | 缺失必填字段 `binding_revision_id` | 拒绝（`required` 校验失败） | **PASS（已拦截）** | Q03 |
| `POS-SS-01-POINT-FAULT` | `source-state-v1.schema.json` | 正例 | 携带具体点位错误证据 `modbus_read_timeout` 的 `iotfast.source_state.v1` | 通过 Schema 校验 | **PASS** | Q04, C02, T06, SC08 |
| `POS-SS-02-PURE-DISCONNECT` | `source-state-v1.schema.json` | 正例 | 仅网关断连 `connection_state="disconnected"` 且 `point_states=[]` | 通过 Schema 校验 | **PASS** | Q04, C02, SC08 |
| `NEG-SS-01-MISSING-TIME-ORIGIN` | `source-state-v1.schema.json` | 反例 | 状态事件缺失 `source_time_origin` | 拒绝（Q04 必填项） | **PASS（已拦截）** | Q04 |
| `NEG-SS-02-MISSING-DELIVERY` | `source-state-v1.schema.json` | 反例 | 状态事件缺失 `delivery.replayed` | 拒绝（Q04 必填项） | **PASS（已拦截）** | Q04 |
| `NEG-SS-03-INVALID-CONN-STATE` | `source-state-v1.schema.json` | 反例 | `connection_state` 传入非法枚举 `"link_down"` | 拒绝（枚举校验失败） | **PASS（已拦截）** | Q04 |
| `POS-VIEW-01-FULL` | `device-view-v1.schema.json` | 正例 | 同时包含 `bound`+`observed`（故障时间与采样时间分离）、`bound_static`+`static_configured`、`unbound`+`no_observation`（`value: null`）的完整设备视图 | 通过 Schema 校验 | **PASS** | C02, NB03, NB04 |
| `NEG-VIEW-01-INVALID-BINDING-STATE` | `device-view-v1.schema.json` | 反例 | `binding_state` 传入 `"auto_bound"` | 拒绝（仅允许 `bound \| bound_static \| unbound`） | **PASS（已拦截）** | NB03 |
| `NEG-VIEW-02-INVALID-OBS-STATE` | `device-view-v1.schema.json` | 反例 | `observation_state` 传入 `"default_zero"` | 拒绝（仅允许 `observed \| static_configured \| no_observation`） | **PASS（已拦截）** | NB03 |
| `POS-HIST-REQ-01` | `history-query-v1.schema.json` | 正例 | 包含全部 8 个必填顶层字段的受限历史查询请求 | 通过 Schema 校验 | **PASS** | Q08, NB07 |
| `NEG-HIST-REQ-MISSING-*` (×8) | `history-query-v1.schema.json` | 反例 | 逐一省略 `resource_ids, range, mode, aggregation, quality_states, order, limit, cursor` 中任一顶层字段 | 全部拒绝（8 项必填检查） | **PASS（8/8 拦截）** | Q08 |
| `NEG-HIST-REQ-SQL-INJECTION-FIELD` | `history-query-v1.schema.json` | 反例 | 注入未知 SQL 字段 `"where_sql": "1=1; DROP TABLE resource;"` | 拒绝（`additionalProperties: false`） | **PASS（已拦截）** | Q03, Q08 |
| `POS-HIST-RESP-01` | `history-query-v1.schema.json#/$defs/HistoryQueryResponse` | 正例 | 含 `coverage`（`event_time_completeness="unknown"`）与裸标量 `items` 的历史响应 | 通过 Schema 校验 | **PASS** | Q08, R07, NB10 |
| `POS-INGEST-STATUS-DURABLE-Q12` | `telemetry-v1.schema.json#/$defs/IngestStatusRecord` | 正例 | `GET /v1/ingestions/{ingest_id}` 双维状态（`status="accepted_durable"`, `history_status="committed"`） | 通过 Schema 校验 | **PASS** | Q12 |
| `NEG-INGEST-STATUS-MISSING-HISTORY-STATUS-Q12` | `telemetry-v1.schema.json#/$defs/IngestStatusRecord` | 反例 | 缺失 `history_status` 字段 | 拒绝（双维状态必填） | **PASS（已拦截）** | Q12 |
| `POS-WS-SUBSCRIPTION-TERMINATED-Q05` | `openapi-v1.json#/components/schemas/WsSubscriptionTerminatedFrame` | 正例 | 凭证过期或权限撤销帧 `type="subscription_terminated", reason="CREDENTIAL_EXPIRED"` | 通过 Schema 校验 | **PASS** | Q05, SC10 |
| `POS-WS-RESYNC-REQUIRED-Q05` | `openapi-v1.json#/components/schemas/WsResyncRequiredFrame` | 正例 | 增量缓冲溢出帧 `type="resync_required", reason="BUFFER_OVERFLOW"` | 通过 Schema 校验 | **PASS** | Q05, T09, NB06 |
| `NEG-WS-RESYNC-WITH-CREDENTIAL-EXPIRED-Q05` | `openapi-v1.json#/components/schemas/WsResyncRequiredFrame` | 反例 | 将 `CREDENTIAL_EXPIRED` 混入 `resync_required` 帧 | 拒绝（Q05 严格区分两类帧） | **PASS（已拦截）** | Q05, SC10 |

---

## 4. 已知边界与后续衔接

1. 本验证脚本（`scripts/verify-machine-contracts.py`）仅使用 Python 3 标准库，可在任何无外网开发机或 CI 容器中独立运行。
2. 当 `D00.4` 扩充 `prototype/feasibility/f1_d00_contract/vectors/d00_golden_vectors.json` 后，`scripts/verify-machine-contracts.py` 会自动加载并校验新增黄金向量。
