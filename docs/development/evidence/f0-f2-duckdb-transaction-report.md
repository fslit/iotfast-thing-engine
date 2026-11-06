# F0 & F2 技术可行性验证证据报告：Rust + DuckDB 构建、单事务原子性与幂等恢复

状态：**已验证（通过）**  
依据文档：[P0 技术可行性验证计划](../feasibility-plan.md)、[数据库表结构设计](../../specifications/database-schema-design.md)、[服务身份、可信数据范围与 D00 公共契约设计](../../specifications/service-identity-and-scope-contract.md)  
机器可读证据：[`feasibility/f0_f2_duckdb_tx/evidence.json`](../../../feasibility/f0_f2_duckdb_tx/evidence.json)

---

## 1. 编号、日期、执行人、环境和版本 / 提交 SHA

| 项目 | 实测记录 |
|---|---|
| **验证编号** | `F0`（Rust、DuckDB crate 和引擎在目标系统上构建、启动）+ `F2`（原子事务、应用去重与重放） |
| **执行日期** | `2026-11-06`（UTC `2026-11-06T01:30:00Z`） |
| **执行人** | `f0-f2-duckdb-tx-agent`（自动化验证代理） |
| **仓库提交 SHA** | `840c36f` |
| **宿主机系统 / 内核** | `Darwin (macOS 27.0)` / `Darwin Kernel Version 27.0.0 (RELEASE_ARM64_T6000)` |
| **CPU / 架构** | `Apple M1 Max` / `arm64` (`aarch64-apple-darwin`) |
| **文件系统** | `APFS (case-insensitive, journaled)` |
| **Rust 编译器** | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| **Cargo 版本** | `cargo 1.98.1 (797e8a9bc 2026-08-05)` |
| **`duckdb` crate 版本** | `1.20000.1`（`libduckdb-sys = 1.20000.1`，按照 `duckdb-rs` 1.5+ 版本编码规则 `1.MAJOR_MINOR_PATCH.x`，`1.20000.1` 精确对应 DuckDB `2.0.0`） |
| **链接与特性配置** | 默认采用 `libduckdb-sys/default`（`capi-v1` + `download-lib` 动态链接官方固定版本 `https://install.duckdb.org/v2.0.0/duckdb-shared-libs-osx-universal.tar.gz`），同时在 `Cargo.toml` 中保留可选 `bundled` feature 支持源码静态编译 |
| **SQL `SELECT version()` 实测返回** | **`v2.0.0`** |
| **SQL `pragma_version()` 源码 Commit** | **`d54cbbe3df`**（完整 SHA：`d54cbbe3df17e8e5ce68312f483b4155836112aa`） |
| **与目标版本关系** | 完全匹配《总体设计》与《可行性计划》要求的 **DuckDB 2.0.0** 目标版本 |
| **开源许可核查** | `duckdb` crate、`libduckdb-sys` crate 与 DuckDB 核心引擎均为 **MIT License** |

---

## 2. 假设、输入数据来源、规模及安全隔离

1. **安全与环境隔离**：
   - 全程仅使用合成模拟遥测报文（租户 `ten-8iic`、模拟网关 `gw-smt-01`、模拟回流焊炉 `dev-reflow-oven-01`、温区属性 `zone1_temp` / 资源 `res-oven01-zone1-temp`），未连接任何真实网关或生产设备，不包含真实网络地址或密钥。
   - 每次测试均通过 `tempfile::TempDir` 在宿主机临时文件系统下创建全新、隔离的空目录，测试结束后自动清理临时数据库文件。
2. **D00 确定性标识与摘要规则落地**：
   - `observation_id`：严格按 `service-identity-and-scope-contract.md` 第 4.1 节实现：`"obs-" + Base32LowerNoPad(SHA-256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])`。
   - `payload_hash`：严格按 `sha256-jcs-v1` 规范化计算，排除 `replayed`、`received_at_ms`、`ingest_id` 与 `wal_position`，确保网关超时重传（`replayed` 从 `false` 变为 `true`）时业务摘要恒定不变。
3. **验证表模型范围**：
   - 覆盖 `database-schema-design.md` 第 6、7、8、11 节定义的四张核心表：`applied_messages`、`history_observation`、`last_persisted_value`、`latest_value`。

---

## 3. 测试步骤和可复现命令 / 配置

### 3.1 可复现命令

```bash
CARGO_TARGET_DIR=/tmp/iotfast-target-f0f2 \
  cargo test --manifest-path feasibility/f0_f2_duckdb_tx/Cargo.toml -- --nocapture
```

运行上述命令会自动执行全部 9 个单元测试用例，并重新生成 `feasibility/f0_f2_duckdb_tx/evidence.json`。

### 3.2 验证步骤设计

1. **F0（构建、冷启动与跨连接重启持久化）**：
   - 在全新空临时目录打开 `<temp_dir>/f0_cold_start.duckdb`（验证打开前文件不存在）；
   - 执行 DDL 初始化 `applied_messages`、`history_observation`、`last_persisted_value`、`latest_value`；
   - 在单事务内写入首条工程值观测 `245.6` 并执行 `CHECKPOINT`，随后释放 `Connection` 关闭数据库；
   - 校验磁盘数据库文件已落盘（实测大小 `2,633,728` 字节）；
   - 重新打开同一数据库文件，校验 `SELECT version()` 为 `v2.0.0`，且 `history_observation`、`latest_value`、`applied_messages` 数据与关闭前完全一致。
2. **F2 Case 1（正常工程值单事务写入与禁止二次换算）**：
   - 模拟网关上报 `value_f64 = 245.6`（附带绑定元数据 `binding_factor_metadata = 0.1`）；
   - 在单一 DuckDB 事务内原子写入 `history_observation`、`last_persisted_value`、`latest_value` 与 `applied_messages`；
   - 验证落库值严格为 `245.6`（未二次乘以 `0.1` 变成 `24.56`）。
3. **F2 Case 2（重复投递幂等判定）**：
   - 使用相同 `message_key = ("ten-8iic", "gw-smt-01", "epoch-20261106-01", "msg-20261106-0001")` 与相同测量值 `245.6`，但将 `replayed` 设为 `true`、`received_at_ms` 延后、传入新的候选 `ingest_id = "ing-0002-unused"`；
   - 验证 `payload_hash` 与 `observation_id` 保持不变，事务识别为 `Duplicate` 并返回首次 `ingest_id = "ing-0001"`，`history_observation` 行数仍为 `1`。
4. **F2 Case 3（同 key 篡改内容冲突拦截）**：
   - 使用相同 `message_key`，但将点位值篡改为 `255.0`；
   - 验证计算出的 `payload_hash` 变化，返回 `IDEMPOTENCY_CONFLICT`（HTTP `409`），事务不提交，数据库内 `history_observation` 与 `latest_value` 保持原值 `245.6`。
5. **F2 Case 4（同毫秒不同 `observation_id` 共存、稳定游标分页与 `source_seq` 数值序）**：
   - 对同一 `resource_id = "res-oven01-zone1-temp"`、同一毫秒 `source_at_ms = 1793869205000`，分别写入两条消息：`source_seq = "999"`（值 `246.0`）与 `source_seq = "1000"`（值 `246.5`）；
   - 验证两条同毫秒记录因 `observation_id` 不同而在 `history_observation` 中完整共存；
   - 以 `LIMIT 1` 和游标 `(source_at_ms, observation_id)` 逐页遍历，验证按 `ORDER BY source_at_ms ASC, observation_id ASC` 多次查询结果完全确定、无漏无重；
   - 验证 `latest_value` 按无符号 64 位整数比较 `source_seq`（`1000 > 999`，而非字符串字典序 `"999" > "1000"`），正确保留 `246.5`。
6. **F2 Case 5（事务提交前中断/回滚、子进程 `SIGKILL` 强杀与重放对账）**：
   - 构造独立发送清单（5 条消息 `msg-batch-001` .. `msg-batch-005`）：
     - 先正常提交 `msg-batch-001`、`msg-batch-002`；
     - **5a 进程内未提交回滚**：在事务内写入 `msg-batch-003` 的 `history_observation` 与 `last_persisted_value`（验证事务内可见），随后在写入 `applied_messages` / `COMMIT` 前中断并释放事务，验证回滚后残留行数为 `0`；
     - **5b 子进程未提交 `SIGKILL`（信号 9）**：启动独立子进程打开同一数据库，开启事务写入 `msg-batch-003` 的全部表但不执行 `COMMIT`，父进程收到就绪信号后立即发送 `SIGKILL` (`kill -9`) 强杀子进程；重新打开数据库验证 `msg-batch-003` 在 `history_observation` 和 `applied_messages` 中脏数据行数均为 `0`；
     - **5c 子进程提交后 `SIGKILL`（信号 9）**：启动独立子进程写入并 `COMMIT` `msg-batch-004`，在正常关闭连接/检查点前立即发送 `SIGKILL` (`kill -9`)；重新打开数据库验证 DuckDB 自动重放内部 `.wal`，`msg-batch-004` 已恢复并存在 `1` 条记录；
     - **全量重放与对账**：按独立发送清单从头重放全部 5 条消息（`replayed = true`），验证 `msg-batch-001/002/004` 命中幂等跳过（`3` 次 duplicate），`msg-batch-003/005` 正常补齐写入（`2` 次 newly applied），最终与清单逐条核对：`applied_messages = 5`、`history_observation = 5`、缺失 `0`、重复 `0`、冲突 `0`。

---

## 4. 期望、实际、原始日志或报告位置

| 用例 | 期望结果 | 实测结果（摘自 `evidence.json`） | 耗时 | 状态 |
|---|---|---|---|---|
| **F0 构建与重启持久化** | `SELECT version()` 返回 `v2.0.0`；空目录建库写入 `245.6` 关闭重开后读回一致 | `reopened_version = "v2.0.0"`，文件大小 `2,633,728 B`，`reopened_history_count = 1`，`reopened_latest_value_f64 = 245.6`，`observation_id = "obs-tfx3jwlgl6v5whg7qbmmbmbany"` | `33 ms` | **PASS** |
| **F2 Case 1 正常工程值写入** | 网关上报 `245.6`（倍率 `0.1`）落库为 `245.6`，不二次换算；单事务更新 4 张表 | `persisted_history_value_f64 = 245.6`，`persisted_latest_value_f64 = 245.6`，`persisted_deadband_last_value = "245.6"`，`no_double_scaling_verified = true` | `10 ms` | **PASS** |
| **F2 Case 2 重复投递幂等** | 同 key 且 `replayed=true` 识别为 `duplicate`，返回原 `ingest_id = "ing-0001"`，历史仍 1 条 | `outcome_kind = "duplicate"`，`returned_ingest_id = "ing-0001"`，`payload_hash_unchanged_with_replayed_true = true`，`history_observation_count = 1` | `1 ms` | **PASS** |
| **F2 Case 3 同 key 篡改冲突** | 同 key 温度改 `255.0` 返回 `IDEMPOTENCY_CONFLICT (409)`，原值 `245.6` 不被覆盖 | `outcome_code = "IDEMPOTENCY_CONFLICT"`，`http_status = 409`，冲突后 `history_value_after_conflict_f64 = 245.6`，`history_observation_count = 1` | `2 ms` | **PASS** |
| **F2 Case 4 同毫秒多观测与分页** | 同毫秒 `1793869205000` 两条不同 `observation_id` 共存，按 `(source_at_ms, observation_id)` 稳定分页；`source_seq` `"1000" > "999"` | `same_ms_record_count = 2`，分页序列 `["obs-z5ds...", "obs-5lzn...", "obs-ez5y..."]` 稳定确定；`latest_value_source_seq = "1000"`（`246.5`） | `27 ms` | **PASS** |
| **F2 Case 5 崩溃回滚与重放对账** | 未提交回滚与 `SIGKILL` 零脏行；提交后 `SIGKILL` 自动从 WAL 恢复；重放 5 条清单后零缺失、零重复 | `in_process_rollback_dirty_rows = 0`，`pre_commit_sigkill_signal = 9`（脏行 `0`），`post_commit_sigkill_signal = 9`（重放前恢复 `1` 行），重放命中 `3` 重复 + `2` 新写，最终 `5/5` 对账 `missing = 0, duplicate = 0, conflict = 0` | `186 ms` | **PASS** |

- **验证代码位置**：[`feasibility/f0_f2_duckdb_tx/src/lib.rs`](../../../feasibility/f0_f2_duckdb_tx/src/lib.rs)
- **机器可读证据位置**：[`feasibility/f0_f2_duckdb_tx/evidence.json`](../../../feasibility/f0_f2_duckdb_tx/evidence.json)

---

## 5. 结果、限制与后续任务

- **综合判定**：**通过（PASS）**。`F0` 与 `F2` 在 `aarch64-apple-darwin` + `rustc 1.98.1` + `duckdb 1.20000.1`（运行时引擎 `v2.0.0`）下全部通过实测，可支撑进入 `D01`（工程骨架）、`D06`（幂等与可靠状态机）与 `D07`（DuckDB 存储层实现）。
- **已识别限制与工程注意事项**：
  1. **构建与链接方式**：`duckdb = "1.20000.1"` 默认通过 `libduckdb-sys` 的 `download-lib` 机制下载官方预编译的 `v2.0.0` 动态库（包含编译期自动复制至 `OUT_DIR` 供 Cargo 运行器加载）；若目标生产环境为离线构建或要求单二进制静态分发，应在生产打包配置中显式启用 `--features bundled`（耗时较长但无外部动态库依赖），并在目标 Linux 发行版上复测 `F0`。
  2. **单写者文件锁约束**：DuckDB 数据库文件在同一时刻仅允许单进程读写连接（跨进程并发打开读写连接会触发文件锁冲突），符合总体设计“单 Rust 进程管理本地 DuckDB 2.0.0 数据文件”的架构假设。
  3. **进程强杀与硬件断电边界**：本报告中的 `SIGKILL` 验证了操作系统级进程崩溃下的 DuckDB 事务原子性与 `.wal` 重放恢复，但不等同于物理断电或磁盘缓存未刷盘测试；应用层 `accepted_durable` 的耐久边界仍需结合 `F3`（应用层 WAL `fsync` 故障注入）共同保障。
