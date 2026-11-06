# F3、ST-04 与 F6 技术可行性验证证据报告（应用层 WAL、非连续检查点与实时订阅边界衔接）

## 1. 编号、日期、执行人、环境和版本 / 提交 SHA

- **验证编号**：`F3`（应用层 WAL 持久边界、故障注入 F01–F05 与硬水位背压 `T07`）、`ST-04 / A06`（多目标非连续检查点与授权缺口披露）、`F6`（WebSocket 快照—增量一致边界衔接与慢消费者溢出恢复 `T09` / `NB05` / `NB06`）
- **执行日期**：2026-11-06
- **执行主体**：F3 & F6 独立验证代理（Rust 原型与自动化故障注入套件）
- **仓库基线提交 SHA**：`840c36f1b485266a595b6730aa5852b1b56ce126`
- **硬件与操作系统环境**：
  - OS：`Darwin 27.0.0 arm64`（macOS，APFS 本地文件系统）
  - Rust 工具链：`rustc 1.98.1 (48a229cea 2026-09-01)` / `cargo 1.98.1 (797e8a9bc 2026-08-05)`
  - 核心依赖版本（锁文件 `feasibility/f3_f6_wal_and_stream/Cargo.lock`）：
    - `serde = 1.0.229`、`serde_json = 1.0.154`
    - `crc32fast = 1.5.4`、`sha2 = 0.10.9`、`hex = 0.4.3`
    - `tempfile = 3.27.0`

---

## 2. 假设、输入数据来源、规模及安全隔离

### 2.1 输入数据来源与安全隔离
- 全部测试数据均由本地确定性模拟器在临时隔离目录（`tempfile::TempDir`）中生成，使用虚构租户 `tenant-8iic`、虚构设备 `oven-02` / `pump-001` 及工程值（如 `245.6 °C`、`0.40..1.12 MPa`）。
- **不包含任何真实设备地址、客户凭据或生产数据**，未连接任何外部物理设备或生产网络。
- 默认 `normalization_owner = "gateway"`，所有测试点位均验证服务端原样持久化网关上报的工程值，不执行二次倍率换算。

### 2.2 已验证事实与环境边界区分
- **已验证事实**：
  1. 固定宽度 33 字节 `wal_position`（`"{segment_no:016x}:{record_offset:016x}"`）的字典序比较与数值 `(segment_no, record_offset)` 二元组全序关系 100% 等价。
  2. 48 字节二进制帧头（`magic = *b"IWAL"`、`version = 1`、`flags`、`payload_len`、`CRC32`、`SHA-256`）加上 `File::sync_all()` / `File::sync_data()` 及父目录 `sync_all()` 的持久边界、F01–F05 故障点注入恢复状态机、硬水位 `503 DURABILITY_BACKPRESSURE` 背压（绝不静默降级为 `accepted_volatile`）。
  3. 非连续投递状态（`101 committed`、`102 inflight -> unknown`、`103 committed`）下连续检查点停留在 `101`、重启不重复投递 `103`、查回对账或显式授权登记 `delivery_gap` 后推进至 `103` 并在查询结果中披露 `known_gaps`。
  4. 多线程并发高频更新（160 次实时测量与质量超时事件）期间注册 WebSocket 订阅并锁定边界 `S=40`，客户端经历 `snapshot_begin` → `snapshot_chunk` → `snapshot_end(boundary_seq=40)` → `delta(stream_seq > 40)` 后与服务端最新 `resource_revision` 100% 一致；慢消费者缓冲溢出时确定性输出 `resync_required(code="BUFFER_OVERFLOW")` 并立即终止流。
- **尚未覆盖的物理边界（不作过度宣称）**：
  - 本次 F01–F05 采用应用层确定性故障点注入与持久文件状态模拟进程级中断，**不等于物理掉电测试**；真实工业现场的目标 Linux 文件系统（ext4/xfs 挂载参数）、NVMe/SSD 写缓存（`F_FULLFSYNC` / 掉电保护电容）仍需在目标硬件上独立实测。

---

## 3. 测试步骤和可复现命令 / 配置

### 3.1 代码与证据位置
- 验证工程目录：`feasibility/f3_f6_wal_and_stream/`
  - `src/wal.rs`：应用层分段 WAL、48B 帧头 + CRC32/SHA-256、`WalPosition`、F01–F05 故障注入与恢复扫描器、`T07` 软/硬水位背压控制
  - `src/checkpoint.rs`：`ST-04 / A06` 多目标投递状态机、非连续检查点前缀计算、重启 `inflight -> unknown` 归一化、远端查回对账与显式授权 `delivery_gap` 披露
  - `src/subscription.rs`：`F6` 有序状态域（`stream_epoch` + 单调 `stream_seq`）、五步快照—增量衔接状态机、作用域隔离、无值 `null/unknown` 保持、慢消费者溢出 `resync_required`
  - `tests/f3_f6_verification.rs`：9 项端到端自动化验证用例与 `evidence.json` 生成器
- 机器可读结果文件：`feasibility/f3_f6_wal_and_stream/evidence.json`

### 3.2 可复现执行命令

```bash
CARGO_TARGET_DIR=/tmp/iotfast-target-f3f6 cargo test \
  --manifest-path /Users/fengsl/code/work/iotfast-thing-engine/feasibility/f3_f6_wal_and_stream/Cargo.toml \
  -- --nocapture
```

---

## 4. 期望、实际结果与分项实测明细

### 4.1 F3 应用层 WAL、持久边界、故障注入（F01–F05）与硬水位背压（`T07`）

| 编号 / 场景 | 注入条件与测试输入 | 期望结果 | 实测结果 | 状态 |
|---|---|---|---|---|
| `WalPosition` & 帧校验 | 跨段与跨位宽偏移（`0x9` vs `0xa`、`0xff` vs `0x100` 等）及 48B 帧头编码 | 固定 33 字节小写十六进制格式，字符串比较与数值顺序完全一致；非法格式拒绝 | 全部边界对比较一致，非法长度/大写字符被确定性拒绝；空观测帧大小 527B（含 48B 帧头） | **PASS** |
| **F01** WAL 写入前崩溃 | 注入 `F01BeforeWalWrite`，随后重启并以原 `message_key`（`replayed=true`）重试 | 崩溃时 WAL 为 0 字节；重试后返回 `202 accepted_durable`，历史仅 1 条 `245.6` | WAL 初始 0B，重启扫描 0 条；重试后返回 `202 accepted_durable`，历史精确 1 条 `245.6`，`SyncAll` 与 `SyncData` 均验证通过 | **PASS** |
| **F02** 尾部半条记录截断恢复 | `msg-f02-01`（`245.6`）成功 durable 落盘后，`msg-f02-02`（`246.8`）写入帧头 + 32B 残缺负载未 `fsync` 即崩溃 | 重启识别尾部不完整帧，安全 `set_len` 截断至 `msg-f02-01` 末尾，保留 `msg-f02-01` 无损；`msg-f02-02` 重试补齐 | 恢复报告输出 `TruncatedIncompleteTail`，文件长度精确回退至 `msg-f02-01` 帧尾；客户端原 key 重试 `msg-f02-02` 后历史包含 `[245.6, 246.8]` | **PASS** |
| **F03** WAL 已 `fsync`、ACK 前崩溃 + 幂等/冲突 | 注入 `F03AfterFsyncBeforeAck`；重启后客户端以原 key 重试（`T03`），以及同 key 篡改温度为 `255.0`（`T04`） | 重启自动将 WAL 记录重放至 DB 并恢复去重索引；原 key 重试返回 `200 duplicate` 与原 `ingest_id`；篡改负载返回 `409 IDEMPOTENCY_CONFLICT` | 重启重放 1 条记录；原 key 重试返回 `200 duplicate` 及相同 `ing-...`；同 key 改值 `255.0` 返回 `409 IDEMPOTENCY_CONFLICT`，原值 `245.6` 未被覆盖 | **PASS** |
| **F04** DB 未提交回滚 / DB 已提交但 checkpoint 滞后 | `msg-f04-01` 在 DB 提交后、checkpoint 更新前崩溃（`F04b`）；`msg-f04-02` 在 WAL 落盘后、DB 提交前崩溃（`F04a`） | 重启从滞后的 `contiguous_wal_position` 扫描：`msg-f04-01` 命中 `applied_messages` 跳过不重复入库，`msg-f04-02` 补重放，checkpoint 推进至 `msg-f04-02` | `scanned_records=2`、`skipped_already_applied=1`、`replayed_records=1`，最终 `contiguous_wal_position` 推进至 `msg-f04-02`，历史精确 2 条无重复 | **PASS** |
| **F05** 分段文件中间记录字节损坏 | 连续写入 3 条记录后，翻转中间第 2 条记录偏移 `+60` 处的字节 | 恢复器重放第 1 条后，在第 2 条处检测到 CRC32/SHA-256 校验失败，**立即停止自动跨越**，发出 `MiddleRecordCorrupted` 告警并置 `HaltedOnWalCorruption`，不重放第 3 条 | `replayed_records=1`，`final_checkpoint` 停留在第 1 条，发出 `MiddleRecordCorrupted` 告警，状态转入 `HaltedOnWalCorruption`，新入站请求返回 `503 WAL_CORRUPTION_HALTED` | **PASS** |
| **T07** 硬水位背压拒绝 | 配置软水位 1 帧、硬水位 2 帧；连续发送 3 条请求 `Prefer: durability=durable` | 第 2 条超软水位返回 `202` + `WAL_SOFT_WATERMARK_EXCEEDED`；第 3 条超硬水位返回 `503 DURABILITY_BACKPRESSURE`（`retryable=true`），绝不降级为 `accepted_volatile` | 第 2 条返回 `202` 带软水位告警；第 3 条返回 `503 DURABILITY_BACKPRESSURE`（`retryable=true`），已确认的 2 条记录完好保留 | **PASS** |

### 4.2 ST-04 / A06 多目标非连续检查点与缺口隔离披露

| 验证步骤 | 输入条件 | 期望结果 | 实测结果 | 状态 |
|---|---|---|---|---|
| 出现中间未决空洞 | `101` (`0000000000000001:0000000000000065`) `Committed`、`102` (`...:0000000000000066`) `Inflight`、`103` (`...:0000000000000067`) `Committed` | `contiguous_wal_position` 保持在 `101`，绝不越过 `102` 推进到 `103` | `contiguous_seq_no = 101`，`contiguous_wal_position = "0000000000000001:0000000000000065"` | **PASS** |
| 进程重启恢复 | 重新打开持久状态文件 | `102` 从 `Inflight` 转为 `Unknown`；`103` 保持 `Committed`；待重试队列仅含 `[102]`，不重复投递 `103`；WAL 回收水位停留在 `101` | `d102.state == Unknown`、`d103.state == Committed`，`items_requiring_recovery_or_retry == [102]`，安全回收水位为 `101` | **PASS** |
| 路径 A：远端查回对账成功 | 按 `obs-102` 查询远端并核对 `payload_hash` 一致 | `102` 转为 `Committed`，`contiguous_wal_position` 从 `101` 推进至 `103`，查询 `partial=false`、`known_gaps=[]` | 检查点立即推进至 `0000000000000001:0000000000000067 (seq 103)`，返回 3 条完整记录 | **PASS** |
| 路径 B：显式授权隔离为 `delivery_gap` | 无审批人时调用隔离；随后传入 `approved_by="ops-lead@8iic"` 与原因授权隔离 `102` | 无审批人调用被拒绝；授权后生成持久 `DeliveryGapRow`，检查点推进至 `103`，历史查询返回 `partial=true`、`warnings=["DELIVERY_GAP_PRESENT"]` 并披露 `gap-20261106-102` | 未授权调用返回 `Err`；授权后检查点推进至 `103`，查询响应精确披露 `gap-20261106-102`（含 `obs-102` 与 `wal_position`） | **PASS** |

### 4.3 F6 WebSocket 快照与实时订阅无缝衔接（`T09`、`NB05`、`NB06`）

| 验证步骤 | 输入条件 | 期望结果 | 实测结果 | 状态 |
|---|---|---|---|---|
| **NB05** 并发高频更新下锁定边界 `S` 衔接 | 预热 40 次更新后注册订阅锁定 `boundary_seq = 40`（分片 `chunk_size = 3`）；同时后台线程并发推送 160 次点位更新与质量超时事件（含越权设备 `pump-002` 更新） | 客户端按序收到 `snapshot_begin` → `snapshot_chunk` → `snapshot_end(boundary_seq="40")` → 160 条 `delta(stream_seq > 40)`；越权设备不可见；未绑定属性保持 `value=null, quality="unknown"`；最终客户端副本与服务端最新状态 100% 一致 | 全部 160 条增量帧满足 `stream_seq > 40` 且 `resource_revision` 严格递增；`pump-002` 零泄漏；`maintenance_note` 保持 `value=None`；通信超时点位保留原 `value_sampled_at_ms` 并更新 `quality="bad"`；最终状态与服务端 100% 相等 | **PASS** |
| **T09 / NB06** 慢消费者缓冲区溢出与重连恢复 | 订阅配置 `max_pending_deltas = 16, coalesce_slow_consumer = false`；消费前连续产生 24 条更新 | 服务端确定性输出 `resync_required(code="BUFFER_OVERFLOW")` 并立即关闭订阅流，绝不静默丢帧；客户端重新订阅后完整收敛到服务端最新状态 | 第 1 帧即返回 `ResyncRequired { code: "BUFFER_OVERFLOW", .. }`，后续 `poll_next_frame` 返回 `None`（`is_terminated == true`）；发起新订阅后与服务端状态 100% 一致 | **PASS** |

### 4.4 执行耗时统计（来自 `evidence.json`）
- **F3（WAL + F01–F05 + T07，含真实文件 `sync_all`）**：`535.31 ms`
- **ST-04 / A06（非连续检查点 + 重启持久化 + 缺口披露）**：`74.14 ms`
- **F6（多线程并发快照衔接 + 溢出 `resync_required` 恢复）**：`0.62 ms`
- **总执行时间**：`610.07 ms`（9/9 测试全部通过，0 警告）

---

## 5. 结果判定、限制与后续任务

- **综合判定**：**通过（PASS）**——应用层 WAL 帧格式、固定宽度 `wal_position`、F01–F05 故障点恢复状态机、`T07` 硬水位背压、`ST-04 / A06` 非连续检查点与缺口披露、以及 `F6` 快照—增量无缝衔接与溢出 `resync_required` 机制均已通过可重复自动化实测验证。
- **已知限制与边界说明**：
  1. 本原型在 `src/wal.rs` 与 `src/checkpoint.rs` 中使用原子临时文件重命名 + `File::sync_all()` 模拟本地持久数据库事务边界，以便在纯 Rust 环境下无 C++ 编译锁争用地确定性验证 F01–F05 与 ST-04 状态机；与真实 DuckDB 事务表（`applied_message`、`history_observation`、`target_checkpoint`）的联合 SQL 事务已在 `F0/F2` 验证项中独立验证，进入 `D06/D07` 正式工程实现时需将本状态机直接接入 DuckDB 事务连接。
  2. 进程级故障点注入不替代目标工业主机掉电测试；上线前需在目标 Linux 环境完成文件权限（`0600`/`0700`）、磁盘配额及掉电缓存策略核验。
- **后续衔接任务**：
  - 将 `WalPosition`、48B 帧头编解码器与 `RecoveryReport` 状态机并入 `D06`（应用层 WAL、ACK、幂等与重放模块）。
  - 将 `NonContiguousCheckpointCoordinator` 与 `DeliveryGapRow` 披露结构并入 `D07 / ST-04`（存储适配器与检查点推进器）。
  - 将 `LiveSubscriptionHub` 的 `boundary_seq` 锁定与有界增量缓冲状态机并入 `D08`（北向 WebSocket `/v1/subscriptions` 实现）。
