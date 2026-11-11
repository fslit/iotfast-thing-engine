# P0 核心数据引擎（D01–D08）测试结论总结

> **2026-11-11 全量收敛更新（D00.3–D11、存储分层 `SD-STORAGE-01`/`SD-STORAGE-02`、`ST-02`/`ST-04`/凭据脱敏、`D01.2`/`D10.1`、UX01–UX12、P1 能力）：**
> 2026-11-07 更正说明中列出的全部代码与契约缺口（WAL `payload_len`/`version` 损坏检测、`source_device_grant` `[valid_from_ms, valid_to_ms)` 半开区间校验、`accepted_volatile` 落库状态标记与 `GET /v1/ingestions/{ingest_id}` 双维状态查询、`resource_id` 权威查表防碰撞、历史查询 `watermark_received_at_ms` 游标稳定性、WebSocket `subscription_terminated` vs `resync_required` 线协议区分、SQLite 业务存储 `business_metadata.sqlite`（14 张 OLTP 表）+ 可插拔 `TimeSeriesStorageAdapter`、`ST-02` 能力矩阵握手、`ST-04` 非连续投递空洞水位线对账、外部 TSDB 凭据脱敏 `"***REDACTED***"`、`D01.2` 单实例目录独占锁 `engine.lock` 与 `stream_epoch`、`D10.1` 在线一致性快照备份与隔离沙箱恢复校验，以及完整 HTTP/1.1 REST + RFC 6455 WebSocket 守护进程 `iotfast_server` 与 HMAC-SHA256 Scope 令牌鉴权）均已修复并落地。
> 当前 `cargo test --locked --manifest-path prototype/Cargo.toml` 共 **30 项测试全部通过**（覆盖 15 个集成测试套件与 `dev_server` 单元测试），`f1_d00_contract` **13 项黄金向量测试通过**，`scripts/verify-machine-contracts.py` **49 条 HTTP/WS 路径（57 个操作、63 个组件 Schema）/ 58 项路由与 `$ref` 检查 / 66 项正反断言全部通过**，`scripts/iotfast_mcp_readonly_adapter.py --self-test` 与一键演示脚本 `python3 scripts/mvp-thing-model-demo.py`（8 步全链路）退出码均为 `0`。物理掉电（F03 硬件级）、二期外部 `taosd`（TDengine）集群实连（`SD-STORAGE-02`/`ST-06`）与真实工业网关现场接入（D09/D10）仍按门禁划分。
>
> **2026-11-07 更正（历史背景保留）：以下 §1–§6 为 2026-11-06 的历史记录，原结论过于乐观，不作为最终现场验收结论。** 原型根 crate 当时实际有 8 个集成测试文件、14 个测试（7 个任务测试及 7 个审查回归测试），而非 9 个套件、15 个测试。2026-11-06 合并后及后续目录调整后的 `cargo test` 均显示 14 个通过；这只能证明所执行的断言通过。本文 §1–§6 保留历史原文，2026-11-10 新增测试覆盖见文末 **§7**。
>
> **位置变更：** 原根 crate 和可行性项目现位于 `prototype/`。运行原型测试使用 `cargo test --manifest-path prototype/Cargo.toml`；独立向量验证使用 `cd prototype/feasibility/f1_d00_contract && cargo test && python3 verify_vectors.py`。历史文中的旧路径和命令保留以反映当时记录。

> 历史状态（已更正）：全部通过 | 分支：`feat/p0-core-data-engine` | 日期：2026-11-06

## 1. 测试概况

| 指标 | 数值 |
|---|---|
| 集成测试套件 | 9 个（d01–d08 + review_fixes） |
| 测试用例 | 15 个 |
| 通过 | 15 |
| 失败 | 0 |
| 引擎模块 | 14 个 `src/*.rs` |
| DuckDB 核心表 | 20 张 |
| 构建工具链 | Rust 1.98.1, `duckdb = "1.20000.1"` (v2.0.0) |

## 2. 各测试套件验证范围

### d01 — 工程骨架、D00 公共契约与 Scope 校验

| 用例 | 验证点 |
|---|---|
| DuckDB v2 健康检查 | `SELECT version()` 返回 `v2.0.0`，数据目录与 WAL 目录可写 |
| observation_id 确定性派生 | `obs-` + Base32LowerNoPad(SHA256(…)[0..16])，空串/NUL 拒绝 |
| payload_hash sha256-jcs-v1 | RFC 8785 JCS 规范化，`-0.0 → 0`，排除 `replayed`/`request_id` |
| WalPosition 编码 | 33 字节 `{016x}:{016x}` 字典序与整数序一致 |
| VerifiedScopeContext | fail-closed、tenant 隔离、URI 段边界匹配、空维度拒绝 |

### d02 — 不可变物模型生命周期

| 用例 | 验证点 |
|---|---|
| 模型草稿→发布→冻结 | `draft → published`（不可变），重复发布拒绝 |
| 模型版本对比 | `compare_revisions` 检测属性增删改与兼容性等级 |

### d03 — 资产树与 UNS 路径重定向

| 用例 | 验证点 |
|---|---|
| 设备实例化 | 稳定 `resource_id = res-{device_id}-{property}`，`canonical_uri` 生成 |
| 设备迁址 | 旧 URI 置 `redirected` + `redirect_until_ms`，新 URI 置 `canonical` |
| NB02 重定向锁 | 锁窗口内拒绝其他设备占用旧 URI |

### d04 — 网关绑定版本状态机

| 用例 | 验证点 |
|---|---|
| 绑定 draft→ready→active | 状态机完整流转，`gateway_ack` 校验 |
| 禁止二次倍率 | `normalization_owner = "gateway"` 时 `server_scale_factor` 必须为空 |
| 旧绑定废止 | `activate_binding` 将前一版本置 `superseded` |

### d05+d06 — 原子遥测校验与 WAL 持久化

| 用例 | 验证点 |
|---|---|
| 整条原子拒绝（T02/SC09） | 部分属性非法时整条 422，无副作用 |
| `value: null` 拒绝 | 返回 `422 VALIDATION_FAILED` |
| SC07 时钟偏移 | `source_at_ms > now + max_future_skew_ms` → `422 CLOCK_SKEW_EXCEEDED` |
| F02 WAL 尾部截断 | 不完整尾部帧安全截断，已有记录保留 |
| F05 WAL 中间帧损坏 | CRC32/SHA-256 校验失败 → `HaltedMiddleRecordCorrupted`，不截断 |
| T07 WAL 硬水位背压 | 超过 `wal_hard_watermark_bytes` → `503 DURABILITY_BACKPRESSURE` |
| R03 状态事件分离 | `iotfast.source_state.v1` 独立幂等域，不混入遥测去重 |

### d07 — 单事务入站、死区、乱序与 WAL 恢复

| 用例 | 验证点 |
|---|---|
| T01 工程值原样存储 | 245.6 → `latest_value = 245.6`，无二次换算 |
| T03/SC04 重放幂等 | `replayed: true` 不改变 `payload_hash`，返回 `200 duplicate` |
| T04/SC05 篡改检测 | 同 key 不同值 → `409 IDEMPOTENCY_CONFLICT` |
| SC06 source_seq u64 | `"999"` vs `"1000"` 按 u64 而非字典序比较 |
| 死区/心跳过滤 | `|Δv| < deadband` 且未过心跳 → 不写 `history_observation` |
| T05 乱序处理 | 旧时间戳写 history 但不覆盖 `latest_value` |
| R08 绑定切换后重试 | 已持久化 `message_key` + 一致 `payload_hash` → `200 duplicate` |
| WAL 崩溃恢复 | `simulate_wal_fsync_without_db_commit` → `recover_from_wal` 重放 |

### d08 — 北向业务视图、历史查询与实时订阅

| 用例 | 验证点 |
|---|---|
| DeviceView R09 属性裁剪 | Scope 授权不足时逐属性裁剪 |
| DeviceShadow | 返回 `revision` + 已裁剪属性 |
| 历史查询 R07 | `coverage.event_time_complete_through_ms = None`（未可证明） |
| 订阅 R04 严格快照 | `SnapshotBegin(S)` → `SnapshotChunk(≤S)` → `SnapshotEnd(S)` |
| Delta 仅 `> S` | `stream_seq ≤ boundary_seq` 的增量不下发 |
| NB06 缓冲溢出 | `pending_deltas ≥ max_pending_events` → `ResyncRequired(BUFFER_OVERFLOW)` |
| NB09 游标签名 | `(tenant_id, scope_hash, query_fp)` + SHA-256 签名绑定 |

### review_fixes — 独立审查修复验证

| 用例 | 对应审查项 | 验证点 |
|---|---|---|
| `test_c1_i1` | C1 + I1 | WAL magic/payload_len 损坏停机不截断；`EngineState::HaltedOnWalCorruption` 转换；后续写入 `503 SERVICE_UNAVAILABLE` |
| `test_c3` | C3 | 重启后 `next_ingest_seq` 从 DB 恢复，`ing-000002` 不碰撞；`stream_seq ≥ 1` |
| `test_i2` | I2 | source_state 时钟偏移 `422`；未知属性 `422`；旧 `observed_at_ms` 不覆盖新 quality |
| `test_i3` | I3 | replayed 在死区内被过滤；质量状态变化（`good → uncertain`）强制写 history |
| `test_i4` | I4 | `delivery_gap` 设 `partial = true`；`desc` 排序正确；`aggregate` 分桶 `count/avg` |
| `test_i5` | I5 | Tenant B 看不到 Tenant A 的 binding/model（`404 RESOURCE_NOT_FOUND`） |
| `test_i6` | I6 | 网关断连时快照 `freshness = "stale"`；凭据过期 → `ResyncRequired(CREDENTIAL_EXPIRED)` |

## 3. 可行性验证交叉引用

| 可行性项 | 验证方式 | 结果 |
|---|---|---|
| F0 DuckDB 事务 | `feasibility/f0_f2_duckdb_tx` + d07 单事务提交 | ✅ |
| F1 D00 契约 | `feasibility/f1_d00_contract`（13 Rust + 7 Python 黄金向量） | ✅ 100% 跨语言一致 |
| F2 WAL 尾部截断 | d05 `CleanWithTruncatedTail` | ✅ |
| F3 WAL 恢复 | d07 `recover_from_wal` + review_fixes C1 中间帧停机 | ✅ |
| F5 中间帧损坏 | review_fixes `test_c1_i1` | ✅ HaltedMiddleRecordCorrupted |
| F6 订阅衔接 | d08 `SnapshotBegin/Chunk/End(S)` + Delta `> S` + `BUFFER_OVERFLOW` | ✅ |
| ST-04 Scope 校验 | d01 URI 段边界 + d08 R09 属性裁剪 + review_fixes I5 跨租户 | ✅ |

## 4. 未覆盖项与已知限制

### 已延期修复（Minor，不影响正确性）

1. **Volatile 入站状态标记**：`accepted_volatile` 当前写入 `accepted_durable` 和哨兵 WAL position
2. **source_time_origin 枚举**：南向遥测允许 `"external_source"`，P0 应限制为 `device_clock/protocol_timestamp/gateway_sample`
3. **实体标识符格式校验**：控制面创建操作未强制 `validate_entity_id`，`resource_id` 拼接存在理论碰撞

### 超出 P0 范围（设计文档已标注）

- HTTP/WebSocket 传输层、mTLS/JWT 签名验证、进程监管
- 物理断电测试（F03）、真实工业网关对接（F4）、高基数负载测试（F5）
- 审计日志全覆盖（仅 `publish_model` 写入 `audit_log`，其他控制面操作待补）
- `validate_active(now_ms)` 在部分控制面入口未调用（I5 部分，需后续补全）

## 5. 执行命令

```bash
# 全量测试
cargo test

# 单套件
cargo test --test d07_history_and_latest_test
cargo test --test review_fixes_test

# 可行性交叉验证
cd feasibility/f1_d00_contract && cargo test && python3 verify_vectors.py
```

## 6. 结论（2026-11-06 历史记录）

P0 核心数据引擎 15 项集成测试全部通过，覆盖从 D00 确定性标识到北向订阅衔接的完整数据链路。3 项 Critical 和 6 项 Important 审查发现均已修复并通过 TDD 验证。3 项 Minor 延期处理，不影响 P0 闭环正确性。引擎可进入下一阶段（HTTP 传输层接入与集成测试）。

---

## 7. 2026-11-10 全量收敛（D00.3–D11、`SD-STORAGE-01`、UX01–UX12、P1）与 HTTP/WS 全链路验证

### 7.1 新增与扩展测试套件及专项证据报告

| 套件 / 脚本 | 用例数 | 核心验证点与对应证据报告 | 结果 |
|---|---:|---|---|
| [`tests/mvp_thing_model_server_test.rs`](../../../prototype/tests/mvp_thing_model_server_test.rs) | 1（含 13 步全链路断言） | HMAC Scope 令牌鉴权与伪造头 `422`（`Q02`）、物模型 `r1->r2->r3` 演进与 `:diff`、资产树与跨树拓扑关系（`FEEDS`）、历史 `canonical_uri` 过期后永久防抢占（`Q06`）、设备物模型平滑升级保留 `resource_id`（`Q11`）、`gateway_measurement + static_config` 属性引用与 `manual_stop_window` 激活（`Q09`）、`source_device_grant` 未到 `valid_from_ms` 拒绝 `403`（`Q02`）、耐久与易失入站双维状态查询 `GET /v1/ingestions/{id}`（`Q07`/`Q12`）、WebSocket 快照衔接 `delta` 与凭据过期 `subscription_terminated`（`Q05`）、`watermark_received_at_ms` 并发迟到稳定分页（`Q08`）、`EngineConfig::validate()` 容量水位校验（`Q10`） | ✅ PASS |
| [`tests/storage_sqlite_duckdb_tdengine_test.rs`](../../../prototype/tests/storage_sqlite_duckdb_tdengine_test.rs) | 4 | `SD-STORAGE-01`/`SD-STORAGE-02` 双存储分层 + `ST-02`/`ST-04`/凭据脱敏 + `D01.2`/`D10.1`：SQLite `business_metadata.sqlite`（14 张 OLTP 表，含 `edge_adapter`、`derived_metric`）独立灾备恢复 + DuckDB `engine.duckdb` 默认时序存储 + 可插拔 `TimeSeriesStorageAdapter`（`tdengine` `STABLE`/Subtable、`/rest/sql/{db}`、`timescaledb`、`clickhouse`）在线切换与查询下推 + `tdengine_config.password = "***REDACTED***"` 脱敏 + `ST-02` 能力矩阵握手（`/v1/storage/capabilities` 与 `strict_lossless: true` 守卫）+ `ST-04` 非连续投递空洞水位线状态机（`/v1/storage/checkpoints` 与 `:reconcile`）+ `D01.2` 单实例目录锁 `engine.lock` 与 `stream_epoch` + `D10.1` 在线一致性快照备份与隔离沙箱恢复校验（`/v1/storage/backups*`） — 见 [存储分层验证报告](storage-sqlite-duckdb-tdengine-report.md) | ✅ PASS |
| [`tests/d09_gateway_replay_fixture_test.rs`](../../../prototype/tests/d09_gateway_replay_fixture_test.rs) | 1 | 离线工业网关断连（`freshness="stale"` 不篡改 `quality`）、缓存乱序补传（`replayed=true` 写历史不倒退 `latest_value`）与绑定版本切换对账 Fixture — 见 [D09 离线网关回放报告](d09-offline-gateway-fixture-report.md) | ✅ PASS |
| [`tests/d10_fault_and_capacity_test.rs`](../../../prototype/tests/d10_fault_and_capacity_test.rs) | 2 | 开发态 F01–F05 故障矩阵、`WAL-CORRUPTION` 停机账本对账、软/硬水位背压（`503 DURABILITY_BACKPRESSURE`）与 P50/P95/P99 延迟基线 — 见 [D10 故障与容量基线报告](d10-dev-fault-capacity-report.md) | ✅ PASS |
| [`tests/d11_northbound_mcp_boundary_test.rs`](../../../prototype/tests/d11_northbound_mcp_boundary_test.rs) & [`scripts/iotfast_mcp_readonly_adapter.py`](../../../scripts/iotfast_mcp_readonly_adapter.py) | 1 + 6 步自检 | 上层只读 MCP 适配器、R09 属性裁剪、越权 404 隐匿、写/控制工具硬拒绝与 AI 注入防护 — 见 [D11 只读 MCP 适配器报告](d11-mcp-readonly-adapter-report.md) | ✅ PASS |
| [`tests/ux_journey_and_matrix_test.rs`](../../../prototype/tests/ux_journey_and_matrix_test.rs) | 1 | U01–U08 完整集成旅程与 UX01–UX12 异常状态矩阵端到端验证 — 见 [UX01–UX12 集成旅程报告](ux01-ux12-integration-journey-report.md) | ✅ PASS |
| [`tests/p1_capability_and_derived_metrics_test.rs`](../../../prototype/tests/p1_capability_and_derived_metrics_test.rs) | 3 | P1 可插拔时序存储能力边界、跨版本单位冲突守卫（`NB08`）、边缘适配器注册表与物模型派生指标（OEE / 单位能耗） — 见 [P1 能力与派生指标报告](p1-capability-and-derived-metrics-report.md) | ✅ PASS |
| [`scripts/verify-machine-contracts.py`](../../../scripts/verify-machine-contracts.py) | 66 项断言 | 4 份 JSON Schema 2020-12、OpenAPI 3.1（49 条路径 / 57 个操作 / 63 个组件 Schema / 58 项路由与 `$ref` 检查）与黄金向量机器契约校验 — 见 [D00.3 机器契约验证报告](d00-3-machine-contract-verification.md) | ✅ PASS (`exit 0`) |
| [`src/bin/dev_server.rs`](../../../prototype/src/bin/dev_server.rs) (unit tests) | 2 | `source_device_grant` `[valid_from_ms, valid_to_ms)` 半开区间边界、DuckDB 事务提交失败回滚并停止准入 | ✅ PASS |
| [`scripts/mvp-thing-model-demo.py`](../../../scripts/mvp-thing-model-demo.py) | 8 步端到端 | 编译并启动 `iotfast_server`，通过真实 HTTP/WebSocket 跑通产品 -> 模型版本演进 -> 资产树与拓扑 -> 设备平滑升级 -> 动态/静态数据引用 -> 耐久遥测与 WS 实时推送 -> 业务视图与历史查询 -> SQLite/DuckDB/TDengine 存储状态、凭据脱敏、`ST-02` 能力矩阵、`ST-04` 水位线对账与 `D10.1` 在线一致性快照备份沙箱恢复校验 | ✅ PASS (`exit 0`) |

### 7.2 最新可复现执行命令

```bash
# 1. 全量单元与集成测试（30 passed, 0 failed）
cargo test --locked --manifest-path prototype/Cargo.toml

# 2. D00 契约黄金向量测试（13 passed）与 Python 跨语言验证
cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml
python3 prototype/feasibility/f1_d00_contract/verify_vectors.py

# 3. D00.3 机器契约校验（49 条路径 / 57 个操作 / 63 个组件 Schema / 58 项路由与 $ref 检查 / 66 项正反断言）
python3 scripts/verify-machine-contracts.py

# 4. D11 上层只读 MCP 适配器自检
python3 scripts/iotfast_mcp_readonly_adapter.py --self-test

# 5. 端到端物模型、UNS 数据管理与存储适配器切换 MVP 演示脚本（8 步）
python3 scripts/mvp-thing-model-demo.py
```
