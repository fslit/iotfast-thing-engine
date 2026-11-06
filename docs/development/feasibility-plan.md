# P0 技术可行性验证计划

状态：本地核心技术可行性（F0、F1、F2、F3、F6 及 D00/ST-04）已通过独立验证工程实测并留存证据；真实网关联调（F4）、现场规模负载（F5）与硬件物理掉电测试尚未执行。以 [完整设计](../architecture/design-specification.md) 为设计依据；实际版本、测试环境、结果和失败原因记录在 `docs/development/evidence/` 下。

## 1. 验证目标与顺序

| 编号 | 验证 | 成功时可进入 | 当前状态 | 证据报告 |
|---|---|---|---|---|
| F0 | Rust、DuckDB crate 和引擎在目标系统上构建、启动 | D01 | **已验证（macOS arm64，DuckDB v2.0.0）** | [F0 & F2 验证报告](evidence/f0-f2-duckdb-transaction-report.md) |
| F1 | 模拟网关工程值入站到 latest 和历史（含 D00 确定性 ID、JCS 摘要、状态分离与 Scope） | D05/D07 | **已验证（13 项契约测试含 PR #2 R01–R09 + 7 组 Rust/Python 黄金向量 100% 一致）** | [F1 & D00 验证报告](evidence/f1-d00-contract-verification-report.md) |
| F2 | 原子事务、应用去重与重放（含子进程 `SIGKILL` 前后恢复对账） | D06/D07 | **已验证（9 项事务与崩溃恢复测试通过）** | [F0 & F2 验证报告](evidence/f0-f2-duckdb-transaction-report.md) |
| F3 | durable ACK 与故障恢复（F01–F05、硬水位背压 `T07`、非连续检查点 `ST-04`） | 可靠模式试点 | **已验证（本地文件 `sync_all` 故障注入通过；物理断电待目标硬件实测）** | [F3、ST-04 & F6 验证报告](evidence/f3-f6-wal-and-subscription-report.md) |
| F4 | 网关真实接口与版本标识 | 真实设备试点 | 待网关资料 | — |
| F5 | 点位规模、写入、查询与空间 | 规模部署 | 待目标指标 | — |
| F6 | 快照与订阅衔接（含并发更新边界 `S` 与溢出 `resync_required`） | D08 | **已验证（并发 160 次更新 0 漏点，溢出强制重同步通过）** | [F3、ST-04 & F6 验证报告](evidence/f3-f6-wal-and-subscription-report.md) |

未通过时保留失败证据，设计替代方案并重新评审；不得用模拟器或单机原型通过代替真实设备联调（F4）、规模压测（F5）或目标 Linux 硬件断电测试。

## 2. F0 构建验证

记录目标 CPU / 系统 / 文件系统、Rust 工具链、Cargo.lock、DuckDB crate 特性、链接方式和运行时引擎版本，分别在每个目标平台执行构建及最小读写。检查发行包依赖、启动日志、崩溃输出和许可义务。成功要求：在目标环境从空数据目录启动、写入、重启后读取；锁定组合可重复构建。DuckDB 2.0.0 为目标，不因文档名称或 crate 版本推定实际引擎版本。

- **当前实测结论（macOS arm64）**：`rustc 1.98.1` + `duckdb = "1.20000.1"`（`libduckdb-sys = "1.20000.1"`）构建通过，SQL `SELECT version()` 实测返回 `v2.0.0`（commit `d54cbbe3df`），空目录建库、写入 `245.6`、关闭重开读回一致。目标生产 Linux 环境的静态 `bundled` 构建在进入部署前按同步骤复测。详见 [F0 & F2 报告](evidence/f0-f2-duckdb-transaction-report.md)。

## 3. F1 数据和契约验证

使用模拟器生成设备、模型、绑定和一组带稳定消息 ID 的点位。网关上报工程值 245.6，历史和 latest 应为 245.6；坏质量、未知点位和绑定不匹配按规范拒绝 / 标记。保存请求、响应和查询结果；不得连接生产设备。明确 source_time_origin、网关设备身份、字段类型和数据来源。

- **当前实测结论**：在 `feasibility/f1_d00_contract/` 中完成 `T01–T06`、`T08`、`T10`、`SC01–SC09`、`NB03–NB04`、`NB07–NB09` 共 13 项契约测试（含 PR #2 的 `R03`、`R05`、`R08`、`R09` 专项验证），并由 Rust 与 Python 3 双端独立校验 `feasibility/f1_d00_contract/test_vectors.json` 中的 7 组黄金向量（含 PR #2 受限探针 `GV-07-PR2-PROBE-SAMPLE`，共 14 个点位 `observation_id` 与 `sha256-jcs-v1` 摘要 100% 匹配）。详见 [F1 & D00 报告](evidence/f1-d00-contract-verification-report.md)。

## 4. F2 数据库事务与幂等

对同消息重复投递和同 key 不同内容，验证去重与冲突。将历史、过滤状态和 applied_messages 放在同一事务，分别在事务提交之前和之后强杀进程；恢复后与独立发送清单按 message_id / 属性核对，记录缺失、重复、冲突。验证实际 DuckDB 版本支持所需隔离、唯一性及事务行为；未通过则不能承诺该恢复方案。

- **当前实测结论**：在 `feasibility/f0_f2_duckdb_tx/` 中实测验证单事务更新 `applied_messages` + `history_observation` + `last_persisted_value` + `latest_value`、重复投递幂等（`replayed=true` 返回原 `ingest_id`）、同 key 改值冲突（`409 IDEMPOTENCY_CONFLICT`）、同毫秒不同 `observation_id` 稳定分页，以及进程内未提交回滚和子进程 `SIGKILL`（提交前/提交后）恢复对账（5 条独立清单对账 0 缺失、0 重复、0 冲突）。详见 [F0 & F2 报告](evidence/f0-f2-duckdb-transaction-report.md)。

## 5. F3 WAL 和耐久边界

为每个请求记录发送、响应和原始摘要；分别在日志写入前、写入未同步、同步后 ACK 前、数据库提交前后、checkpoint 前后注入故障。重启重放并与清单核对。磁盘满、日志尾部截断、中间段损坏和只读文件系统分别测试；只接受 durable ACK 的样本参与耐久保证统计。强杀进程不等于断电；断电测试需独立环境、现场许可和硬件缓存策略说明。返回状态为 volatile 时不宣称重启可恢复。

- **当前实测结论**：在 `feasibility/f3_f6_wal_and_stream/` 中完成 33 字节固定宽度 `wal_position`、48 字节帧头（CRC32 + SHA-256）、`File::sync_all` 持久边界、F01–F05 故障点注入恢复、硬水位 `503 DURABILITY_BACKPRESSURE` 背压（`T07`）以及 `ST-04 / A06` 非连续检查点与 `delivery_gap` 披露验证。详见 [F3、ST-04 & F6 报告](evidence/f3-f6-wal-and-subscription-report.md)。

## 6. F4 网关对接

先取得示例消息、版本信息、认证机制、源时间、质量、ID / 序列、离线缓存及绑定确认能力。列出能力差距：稳定 ID / 持久重试缺失时只能按 volatile 或经已验证的持久桥接接入。真实设备地址、倍率和字节序由现场确认，测试前签字；网关侧转换后不再乘倍率。模拟器、实验设备和生产现场三类结果分别记录。

## 7. F5 容量与性能

输入：目标设备数、点数、采样分布、峰值、磁盘配额、保留时间、允许延迟、恢复时间与最长断网时间，由项目方确认。分别测稳态、峰值、冷启动、补传、查询并发和磁盘水位；记录点 / 秒、每条消息点数、批次、ACK 类型、P50/P95/P99、内存、CPU、WAL 大小、磁盘、缺失 / 重复及恢复速度。若无明确目标，只报告测量值，不判定生产达标。

## 8. F6 订阅连续性

在持续更新期间并发生成快照并订阅：建立一致边界，核对边界之后的有序事件；缓冲溢出时必须报告 resync_required。断线后重新获取快照；不能将实时订阅当作保证连续的历史通道。

- **当前实测结论**：在 `feasibility/f3_f6_wal_and_stream/` 中实测验证并发 160 次点位更新与质量超时事件下锁定 `boundary_seq=40` 的五步快照—增量衔接（客户端副本与服务端最新 `resource_revision` 100% 一致），以及有界缓冲溢出时确定性返回 `resync_required(code="BUFFER_OVERFLOW")` 并立即终止流。详见 [F3、ST-04 & F6 报告](evidence/f3-f6-wal-and-subscription-report.md)。

## 9. 证据记录模板

- 编号、日期、执行人、环境和版本 / 提交 SHA：
- 假设、输入数据来源、规模及安全隔离：
- 测试步骤和可复现命令 / 配置（脱敏）：
- 期望、实际、原始日志或报告位置：
- 结果：通过 / 失败 / 阻塞；限制与后续任务：

不要在仓库提交密钥、客户设备地址或生产数据；脱敏后的数据仍需经项目规范审查。
