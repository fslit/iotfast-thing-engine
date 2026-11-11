# D10 开发机受控进程级故障恢复矩阵与容量水位压测报告

> **重要声明（适用范围与生产门禁）**：本报告为**开发机受控进程级故障与基线测量，不等同于目标生产硬件物理断电签收**。目标生产环境硬件配置、文件系统掉电保护、生产级并发负载 SLA 及 RPO/RTO 承诺仍受 [`specification-decisions.md`](../specification-decisions.md) §4 与 [`ai-execution-plan.md`](../ai-execution-plan.md) §6.2 现场输入门禁约束。

- **任务卡**：`D10`（进程故障恢复矩阵 `F01–F05` + `WAL-CORRUPTION` 与受控容量/水位压测探针）
- **执行日期**：2026-11-10
- **测试实现**：[`prototype/tests/d10_fault_and_capacity_test.rs`](../../../prototype/tests/d10_fault_and_capacity_test.rs)

---

## 1. 测试环境信息

| 维度 | 实测配置 / 版本 |
|---|---|
| 操作系统 / 内核 | macOS (`Darwin 27.0.0 arm64`, `RELEASE_ARM64_T6000`) |
| CPU / 核心数 | Apple M1 Max（10 核） |
| 物理内存 | 64 GiB (`68,719,476,736` bytes) |
| Rust 工具链 | `rustc 1.98.1 (48a229cea 2026-09-01)` / `cargo 1.98.1 (797e8a9bc 2026-08-05)` |
| 存储引擎版本 | DuckDB `v2.0.0`（本地动态库 `@rpath` 绑定） + `AppWal`（`IWAL` v1 48 字节帧头，CRC32 + SHA-256 双校验） |
| 编译配置 | `test` profile (`unoptimized + debuginfo`, `--locked`) |

---

## 2. 验证命令与退出码

```bash
cargo test --locked --manifest-path prototype/Cargo.toml --test d10_fault_and_capacity_test -- --nocapture
```

- **退出码**：`0`
- **编译警告**：`0`
- **执行结果**：`2 passed; 0 failed; 0 ignored; finished in 3.43s`
- **标准输出摘录**：
  ```text
  running 2 tests
  test d10_fault_matrix_f01_to_f05_and_wal_corruption_ledger_reconciliation ... ok
  [D10_CAPACITY_PROFILE] batches=50 points=200 min_us=53804 p50_us=58252 p95_us=76207 p99_us=107128 max_us=107128 avg_us=60938.5 wal_bytes=117940
  test d10_capacity_watermark_backpressure_and_latency_profile ... ok
  ```

---

## 3. 故障恢复矩阵与外部发送账本对账表（`F01–F05` + `WAL-CORRUPTION`）

测试函数：`d10_fault_matrix_f01_to_f05_and_wal_corruption_ledger_reconciliation`

通过独立维护的客户端已确认账本（`ExternalSentLedger`）记录所有收到 `accepted_durable` 的 `message_id`、`ingest_id`、`payload_hash`、`wal_position`、`observation_ids` 与工程值，在每一步故障注入与重启恢复后与 DuckDB `applied_message`、`history_observation`、`ingest_status`、`target_checkpoint` 进行 100% 交叉核对：

| 故障编号（总体规范命名空间） | 故障时点与注入方式 | 预期规范行为（Q07 / Q12 / §23） | 实测恢复与外部账本对账结果 | 状态 |
|---|---|---|---|---|
| **总体规范 F01**<br>（WAL 写前拒绝/中断） | 发送含未定义属性 `unknown_zone99_temp` 的批次 `m-f01-rejected`，随后客户端修正并重试 `m-f01-retry-ok` (`replayed=true`) | 非法批次在进入 WAL 前原子拒绝（`422 VALIDATION_FAILED`），WAL 字节数与 DB 行数零变化；修正后重试返回 `202 accepted_durable` | 拒绝批次 WAL 增量 `0 B`、DB 历史 `0` 行；重试批次成功落盘并计入外部账本（1 批 / 2 观测点），账本与 DB 100% 一致 | PASS |
| **总体规范 F02**<br>（WAL 写后、`fsync`/ACK 前半帧截断） | 先确认 `m-f02-acked`（`202 accepted_durable`），随后向活动 WAL 段末尾追加未写完的半帧字节（`IWAL...unacked_partial_tail`），重启执行恢复 | `scan_and_recover` 识别尾部半帧并返回 `CleanWithTruncatedTail`，将文件精确截断回上一合法帧末尾；已确认消息 0 丢失 | 半帧从偏移 `valid_len_before_f02` 处被干净截断；`ExternalSentLedger` 已确认的 2 批（4 个观测点）在 DuckDB 中 0 丢失、0 重复 | PASS |
| **总体规范 F03**<br>（WAL `fsync` 完成、DuckDB 提交前崩溃） | 调用 `simulate_wal_fsync_without_db_commit` 将 `m-f03-wal-fsynced` 写入 WAL 并 `fsync`，但在 DuckDB 事务提交前关闭运行时；重启调用 `recover_from_wal()`，随后网关带 `replayed=true` 重试该消息并查询 `get_ingest_status` | 重启恢复自动从 `target_checkpoint` 之后重放未提交 WAL 帧并推进 checkpoint；客户端重试幂等返回 `200 duplicate` 及原 `ingest_id`；`get_ingest_status` 返回 `status="accepted_durable"` + `history_status="committed"` | `recover_from_wal()` 自动补投影 `replayed_messages = 1`；网关重试返回 `200 duplicate`；`get_ingest_status` 状态为 `accepted_durable` + `committed`；外部账本增至 3 批（6 个观测点）并与 DB 100% 对账 | PASS |
| **总体规范 F04**<br>（DuckDB 事务执行中中断回滚） | `apply_wal_record_in_tx` 使用单库 `BEGIN TRANSACTION ... COMMIT / ROLLBACK` 原子包裹 `history_observation`、`last_persisted_value`、`latest_value`、`point_quality_state`、`applied_message`、`ingest_status` 与 `target_checkpoint` | 事务未提交前崩溃不产生半消息或脏读，重启后由 F03/F05 路径幂等重放收敛 | 所有表写入在同一 DuckDB 事务内原子提交，无跨表半提交状态（联合 [`f0_f2_duckdb_tx`](f0_f2_duckdb_tx_report.md) 证据） | PASS |
| **总体规范 F05**<br>（DuckDB 已提交、`target_checkpoint` 未推进时崩溃重放） | 写入并确认 `m-f05-db-committed`，清除 `target_checkpoint` 模拟 DB 已提交但 checkpoint 未推进即崩溃；重启调用 `recover_from_wal()` 从头扫描全部 4 条 WAL 记录 | 恢复流程按 `applied_message` 幂等跳过已落库消息（`replayed_messages = 0`），自动修复 `target_checkpoint` 至最新 `wal_position`，`history_observation` 0 重复 | `replayed_messages = 0`，`target_checkpoint` 恢复至第 4 帧位置；`ExternalSentLedger` 4 批（8 个观测点）与 DuckDB `history_observation`（8 行）100% 匹配，0 丢失、0 重复 | PASS |
| **`WAL-CORRUPTION`**<br>（中间帧 CRC32 或 `payload_len` 损坏） | 在存在后续合法第 3、4 帧的情况下，先后篡改第 2 帧（中间帧）的 `CRC32` 字段与 `payload_len`（设为 `99_999_999`），重启调用 `recover_from_wal()`，并尝试继续调用 `ingest_telemetry` 与 `report_source_state` | 严禁将中间损坏帧误判为尾部截断；引擎立即转入 `EngineState::HaltedOnWalCorruption`，保留原 WAL 文件字节不截断，健康检查与后续所有写入统一返回 `503 SERVICE_UNAVAILABLE` | 两次篡改均触发 `HaltedOnWalCorruption`，WAL 文件长度保持原样未截断；`check_health()` 返回 `503`；后续遥测与状态写入均被 `503 SERVICE_UNAVAILABLE` 拒绝；此前已提交的 4 批（8 点）账本完好无损 | PASS |

---

## 4. 受控容量、并发分页水位（Q08）与硬水位背压（T07）实测数据

测试函数：`d10_capacity_watermark_backpressure_and_latency_profile`

### 4.1 压测模型与单批耐久入站延迟分布

- **负载规模**：连续 50 批耐久遥测（`DurabilityPreference::Durable`，每批均执行 `AppWal::append_durable` 含 `fsync` + DuckDB 单事务多表投影），每批包含 4 个浮点属性（`zone1_temp`、`zone2_temp`、`conveyor_speed`、`chamber_pressure`），合计 **200 个观测点**。
- **单批入站耗时统计（含 WAL `fsync` + DuckDB 事务提交，单位：微秒 / 毫秒）**：

| 统计指标 | 实测微秒 (`µs`) | 折合毫秒 (`ms`) | 说明 |
|---|---|---|---|
| **Min** | `53,804 µs` | `53.80 ms` | 单批 4 点耐久写入最快耗时 |
| **P50 (Median)** | `58,252 µs` | `58.25 ms` | 中位单批耐久写入耗时 |
| **Mean (Avg)** | `60,938.5 µs` | `60.94 ms` | 50 批平均耗时 |
| **P95** | `76,207 µs` | `76.21 ms` | 95 分位单批耐久写入耗时 |
| **P99 / Max** | `107,128 µs` | `107.13 ms` | 99 分位 / 最大单批耗时 |
| **50 批累计 WAL 体积** | `117,940 bytes` | `~115.18 KiB` | 平均每批帧大小约 `2,358.8 B` |

### 4.2 并发写入下的历史查询水位快照稳定性（Q08）

- 在第 25 批写入完成时（已提交 100 个观测点，`watermark_received_at_ms = base_ts + 25_020`），发起首页 `query_history(limit=30, order="asc")`，获取带 HMAC-SHA256 签名的 `next_cursor`。
- 在第 26..50 批（新增 100 个观测点，`received_at_ms > watermark_received_at_ms`）持续写入期间，穿插在第 35、45、50 批使用 `next_cursor` 翻页直至游标耗尽。
- **验证结果**：分页查询共返回 **100 条唯一观测记录**，全部满足 `received_at_ms <= watermark_received_at_ms`，与第 1..25 批外部发送账本集合完全一致（**0 漏读、0 重复、0 新数据串页**）。

### 4.3 WAL 硬水位背压（T07）与外部账本对账

- 在 50 批写入完成后（`wal_bytes = 117,940`），将 `wal_hard_watermark_bytes` 设为 `117,956`（仅余 16 字节，不足容纳下一帧），发送第 51 批 `m-cap-051-backpressure`。
- **验证结果**：
  - 引擎立即拒绝写入并返回 `503 DURABILITY_BACKPRESSURE`（`retry_after_ms = Some(1000)`），未静默降级为 `accepted_volatile`。
  - `ExternalSentLedger` 记录的 50 批（200 个观测点）与 DuckDB `applied_message`（50 行）及 `history_observation`（200 行）100% 对账吻合（**0 丢失、0 重复**）。

---

## 5. 限制说明与未决现场门禁

1. **开发机进程级测试 vs 目标硬件物理断电**：本次验证在 macOS (Apple M1 Max) 本地文件系统上通过进程内文件截断、字节篡改、事务前中断及重启恢复完成，**不替代目标工业服务器/边缘主机上的真实物理拉闸断电（Power-loss）与磁盘控制器写缓存（Write Cache）持久性测试**。
2. **Debug 构建与单连接吞吐基线**：上述 P50 (`58.25 ms`) / P95 (`76.21 ms`) / P99 (`107.13 ms`) 是在 `cargo test` 未优化（`unoptimized + debuginfo`）且每批逐帧同步执行 `fsync` + DuckDB 独立事务的保守基线下测得，仅作为开发机回归基线，不代表目标生产硬件 Release 构建指标。
3. **待现场负责人签认项**：目标生产环境的硬件规格、并发峰值 SLA、数据保留期及破坏性掉电演练窗口仍处于“待项目方确认”状态（见 [`ai-execution-plan.md`](../ai-execution-plan.md) §6.2）。
