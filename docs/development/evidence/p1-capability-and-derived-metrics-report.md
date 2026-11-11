# P1 存储适配器能力边界与物模型衍生指标（OEE / 能耗）契约验证报告

- **任务卡 ID**：`P1-CAPABILITY-METRICS`
- **关联规范**：
  - [外部 TSDB 共性接口与差异能力设计](../../specifications/tsdb-common-extension-interface.md)
  - [衍生指标、OEE 与能耗算法和设备模型结合设计](../../specifications/derived-metrics-oee-energy.md)
  - [综合技术设计规范 §21 / §23 / §28](../../architecture/design-specification.md)
- **修改范围（Write Scope）**：
  - `prototype/tests/p1_capability_and_derived_metrics_test.rs`
  - `docs/development/evidence/p1-capability-and-derived-metrics-report.md`
- **禁止范围遵守情况**：未修改 `prototype/src/**`、`Cargo.toml` 或任何其他文件。

---

## 1. 验证命令与执行结果

```bash
cargo test --locked --manifest-path prototype/Cargo.toml --test p1_capability_and_derived_metrics_test
```

- **退出码**：`0`
- **通过测试数**：`2 passed; 0 failed`
  1. `p1_storage_capability_boundaries_and_cross_version_unit_guard` ... `ok`
  2. `p1_derived_metrics_oee_and_specific_energy_over_thing_model` ... `ok`

---

## 2. 存储适配器能力边界与历史查询防线对账表（测试 1）

| 验证维度 | 规范条款 | 验证输入 / 场景 | 实测返回结果 | 结论 |
|---|---|---|---|---|
| 默认本地存储目标与一致性级别 | `tsdb-common-extension-interface.md` §1, §6.2 | 对 `res-pump-001-outlet-pressure` 执行 `query_history` | `target_id = "local-duckdb"`，`consistency = "checkpoint_consistent"` | 通过 |
| 事件时间完整性诚实披露（禁止借 WAL 位置冒充水位） | `R07` / `NB07` | 写入 3 条持久化遥测后查询 `coverage` | `contiguous_wal_position = Some(...)`，`event_time_completeness = "unknown"`，`event_time_complete_through_ms = None` | 通过 |
| 跨物模型版本 `raw` 历史保真 | `C10` / `NB08` | `r1` (`kPa`) 写入 `410.0`, `430.0`；激活 `r2` (`MPa`) 写入 `0.45`；执行 `mode = "raw"` 查询 `[100_000, 140_000)` | 前 2 条保留 `model_revision_id = "pump-model-r1-kpa"`、`unit_code = Some("kPa")`；第 3 条保留 `model_revision_id = "pump-model-r2-mpa"`、`unit_code = Some("MPa")` | 通过 |
| 跨物模型版本不兼容单位 `aggregate` 拒算 | `NB08` / `C09` | 跨 `kPa` 与 `MPa` 区间 `[100_000, 140_000)` 执行 `mode = "aggregate"` (`avg`/`min`/`max`) | 严格拒绝并返回 `422 CROSS_VERSION_INCOMPATIBLE`（绝不混算不同量纲） | 通过 |
| 同版本同单位区间 `aggregate` 正常聚合 | `NB08` | 在单版本 `r1` (`kPa`) 区间 `[100_000, 120_000)` 执行 `mode = "aggregate"` | 返回 1 个聚合桶，`count = 2`，`avg = Some(420.0)` | 通过 |
| 投递缺口（`delivery_gap`）诚实暴露 | `NB10` / `I4` | 登记 `[112_000, 118_000)` 缺口 `gap-pump-001-01`（原因：`gateway_buffer_overflow_isolated`）后查询 `[100_000, 140_000)` | `partial = true`，`known_gaps` 精确返回该缺口的 `gap_id`、`resource_id`、`start_ms`、`end_ms` 与 `reason` | 通过 |

---

## 3. 物模型衍生指标（OEE / 区间能耗 / 单位产品能耗）血缘与边界对账表（测试 2）

### 3.1 物模型属性与绑定配置

在 `line-unit-model-r1` 物模型中同时定义了动态采集属性（`collected`）与静态配置属性（`static`），并通过 `BindingService::create_binding_draft_with_statics` 激活 `line-01-unit-bind-r1`：

| 属性标识符 (`identifier`) | 属性来源 (`source_kind`) | 数据类型 | 单位 (`unit_code` / `unit_symbol`) | 绑定 / 配置值 |
|---|---|---|---|---|
| `energy_total` | `collected`（网关采集） | `float64` | `kWh` / `kWh` | Modbus `40100`，`factor = 0.1` |
| `active_power` | `collected`（网关采集） | `float64` | `kW` / `kW` | Modbus `40102`，`factor = 0.1` |
| `production_count` | `collected`（网关采集） | `int64` | `pcs` / `件` | Modbus `40200`，`factor = 1.0` |
| `ideal_cycle_min` | `static`（静态配置） | `float64` | `min_per_pcs` / `min/件` | `0.5`（在 `DeviceBusinessView` 中状态为 `bound_static` / `static_configured`） |
| `rated_power` | `static`（静态配置） | `float64` | `kW` / `kW` | `60.0`（在 `DeviceBusinessView` 中状态为 `bound_static` / `static_configured`） |

### 3.2 正常窗口计算与血缘对账

| 衍生指标 | 规范参考 | 输入观测 / 静态属性与业务上下文 | 计算结果与状态 | 血缘追溯 (`MetricLineageRef`) |
|---|---|---|---|---|
| 区间耗电量 (`energy_consumption`) | `derived-metrics-oee-energy.md` §6.1, §6.2 | 期初电表 `10,250.0 kWh` (`200_000ms`, `good`)、期末电表 `10,730.0 kWh` (`228_800ms`, `good`) | `value = 480.0`, `unit = "kWh"`, `quality = "good"`, `calculation_status = "final"` | 引用期初与期末 2 条 `res-line-01-unit-energy-total` 的真实 `observation_id`、`line-unit-model-r1` 与 `line-01-unit-bind-r1` |
| 单位产品能耗 (`specific_energy`) | `derived-metrics-oee-energy.md` §6.1, §6.2 | 区间耗电量 `480.0 kWh`、合格产量 `800 件` (`good`) | `value = 0.60`, `unit = "kWh/件"`, `quality = "good"`, `calculation_status = "final"` | 同上，保留电表首末观测 ID 与模型/绑定修订号 |
| 综合设备效率 (`oee`) | `derived-metrics-oee-energy.md` §5.1, §5.2, §8 | 计划生产时间 `480 min`、实际运行 `400 min`、物模型静态属性 `ideal_cycle_min = 0.5 min/件`、采集总产量 `720 件`、良品 `684 件` | `A = 83.33%`, `P = 90.00%`, `Q = 95.00%`, `OEE = 71.25%` (`value = 0.7125`, `unit = "ratio"`, `calculation_status = "final"`) | 同时包含 `source_kind = "static"` (`res-line-01-unit-ideal-cycle-min`) 与 `source_kind = "collected"` (`res-line-01-unit-production-count` + `observation_id`) |

### 3.3 异常与坏质量防线验证

| 异常场景 | 规范要求 | 测试输入 | 实测防线行为 |
|---|---|---|---|
| 电表回卷 / 未登记清零（负差值） | §6.1 / §10：不能对负差值简单取绝对值 | 期初 `10,730.0 kWh` -> 期末 `10,100.0 kWh` (`delta = -630.0`) | 拒绝输出正值，`value = None`，`calculation_status = "uncalculable"`，`quality = "uncertain"`，`error_reason = "NEGATIVE_METER_DELTA_ROLLOVER_OR_RESET"` |
| 输入含 `bad` 质量采样 | §4：输入 `bad` 按策略拒算或输出 `uncertain` | 上报 `quality.state = "bad"` (`CRC_ERROR` / `SENSOR_DISCONNECTED`) 的电表与产量观测 | 拒绝输出正常数值，`value = None`，`calculation_status = "uncertain"`，`quality = "uncertain"`，`error_reason = "INPUT_OBSERVATION_QUALITY_NOT_GOOD"` / `"INPUT_QUALITY_NOT_GOOD"` |
| 分母为 `0`（合格产量为 `0` 或计划时间为 `0`） | §4 / §5.1 / §10：无计划时间或分母为 0 不除零，返回不可计算及原因 | `good_output_count = 0` 或 `planned_production_min = 0.0` | `value = None`，`calculation_status = "uncalculable"`，`error_reason = "ZERO_OR_NEGATIVE_GOOD_OUTPUT_DENOMINATOR"` / `"INVALID_OR_ZERO_DENOMINATOR"` |

---

## 4. Stage 6 边缘适配器能力注册表与衍生指标 HTTP 接口及 SQLite 持久化

在 Stage 6 扩展闭环中，边缘协议适配器能力声明与物模型衍生指标定义已纳入 SQLite 业务元数据存储（`<data_dir>/business_metadata.sqlite`，使 SQLite OLTP 业务目录表扩展至 **14 张表**：`product`、`model_revision`、`model_item`、`asset`、`device`、`resource`、`resource_path`、`source_gateway`、`source_device_grant`、`binding_revision`、`point_binding`、`asset_relation`、`edge_adapter`、`derived_metric`），并通过 `iotfast_server` 暴露标准 HTTP REST 接口：

| 领域 | SQLite 业务表 | HTTP 路由 | 核心能力 |
|---|---|---|---|
| 边缘适配器能力注册表 | `edge_adapter` (`adapter_id` PK) | `POST /v1/adapters`<br/>`GET /v1/adapters`<br/>`POST /v1/adapters/{adapter_id}:validate-binding` | 注册边缘协议（如 Modbus TCP/RTU、OPC UA、Sparkplug B）支持的值类型、`gateway_ack` 能力与离线补传能力；对照适配器能力校验绑定草稿 |
| 物模型衍生指标引擎 | `derived_metric` (`metric_id` PK) | `POST /v1/derived-metrics`<br/>`GET /v1/derived-metrics`<br/>`POST /v1/derived-metrics/{metric_id}:evaluate` | 定义绑定在产品/物模型上的衍生指标（`energy_consumption` / `specific_energy` / `oee`）；在线结合 `collected` 历史观测与 `static` 静态铭牌属性执行窗口评估并返回 `observation_id` 血缘快照 |

---

## 5. 边界声明与未完成项（遵循 AGENTS.md 诚实披露原则）

1. **外部 TSDB 真实数据库生产集群实测仍属 P1 后续独立门禁（`C05`）**：
   - 本地已通过 `DuckDbTimeSeriesAdapter` 与可插拔 `TimeSeriesStorageAdapter`（`tdengine` / `timescaledb` / `clickhouse`）验证了统一历史响应的 `coverage`（`R07`）、跨版本单位防线（`NB08`）、`delivery_gap` 披露（`NB10`）以及原生 SQL 下推生成（见 [存储分层验证报告](storage-sqlite-duckdb-tdengine-report.md)）。
   - 对接真实外部 TDengine (`taosd`)、TimescaleDB、ClickHouse 生产集群的高可用与压测尚未在现场环境执行，**不将本地适配器契约测试等同于外部 TSDB 生产集群签收**。
2. **异步流式重算与 `metric_result_revision` 历史物化表仍属 P2 演进项**：
   - 当前已实现 `derived_metric` 定义持久化（SQLite `derived_metric` 表）与 `POST /v1/derived-metrics/{metric_id}:evaluate` 在线确定性评估及血缘快照输出；由迟到回补数据自动触发的后台重算流水线与 `metric_result_revision` 物化历史表留待后续阶段实施。
