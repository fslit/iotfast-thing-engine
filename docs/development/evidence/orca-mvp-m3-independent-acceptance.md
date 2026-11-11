# Orca 本地只读最小 MVP（`orca-mvp-dev-v1`）M3 独立验收证据

- 日期：2026-11-10（Asia/Shanghai）
- HEAD：`c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`（main；工作区为既有 dirty 重组，本文件仅新增本证据，未修改任何源码、Cargo 清单、既有他人文档）
- 角色：独立验收工程（M3）。既不参与实现，也不是 M2 测试者；以只读方式构建、启动、真实 HTTP 验证，不改代码。
- 执行模型：**DeepSeek V4.1 Flash**（本会话）。**不是 gpt-6.1-terra 验证**——Terra 端点不可用，故未以 Terra 名义给任何结论。

## 0. 范围声明（必读）

**本验收只针对演示范围内的本地只读最小 profile，不是生产验收、不是完整 P0、不是 D00–D10 全量验收。** 明确**不覆盖**：UI/控制面/设备控制、外部 TSDB、WebSocket/订阅、真实网关与点表签认、TLS/mTLS/JWT 生产认证、性能/容量 SLA、崩溃恢复/断电无损、跨会话去重、动态配置。下文所有“PASS”仅在 `orca-mvp-dev-v1` 冻结契约（[M0 计划](../orca-mvp-plan.md)）的演示子集内成立。

## 1. 交叉验证来源（非单一来源）

Orca 原生 M2（[orca-mvp-m2.md](orca-mvp-m2.md)）因模型 provider 路由 404/503 及客户端传输问题**未完成**，不能作为独立放行依据。本 M3 结论依赖四方交叉验证，而非任一单方自述：

1. 实现者 M1 自证（[orca-mvp-m1.md](orca-mvp-m1.md)）；
2. Lead 独立只读验证（[lead-mvp-independent-verification.md](lead-mvp-independent-verification.md)，含 §5 未验证项、§7 关于“DB 失败→503”的注记）；
3. 独立 DeepSeek M2 完整 24 组 HTTP 断言 24 PASS（[orca-mvp-m2-independent-deepseek.md](orca-mvp-m2-independent-deepseek.md)）；
4. 本次 M3 抽验（本文件）：重新构建/测试、干净实例真实 HTTP 闭环、启动闸门、代码核对。

四方对同一冻结契约给出相互一致的主要结论；本文件对关键闭环**自行动手复跑**，不照抄上述结论。

## 2. 环境

| 项目 | 值 |
|---|---|
| 平台 | macOS 27.0 (Build 26A428)，Darwin arm64 |
| rustc / cargo | 1.98.1 (48a229cea 2026-09-01) / 1.98.1 (797e8a9bc 2026-08-05) |
| Python | 3.14.7 |
| 服务二进制 | `prototype/target/debug/dev_server` |
| 运行时 DuckDB 引擎 | `v2.0.0`（进程启动即 `SELECT version()`，见每次启动 stderr 首行 `DuckDB SELECT version(): v2.0.0`） |
| 动态库 | `@rpath/libduckdb.dylib`，无 `LC_RPATH`；直接执行需 `DYLD_LIBRARY_PATH=$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0`（历史缓存，非系统安装） |

## 3. 构建与测试（真实退出码）

| 命令 | 退出码 | 结果 |
|---|---|---|
| `cargo build --manifest-path prototype/Cargo.toml --bin dev_server` | **0** | `Finished dev profile ... in 0.35s`。**复用现有 target 缓存，不构成干净隔离构建证据** |
| `cargo test --manifest-path prototype/Cargo.toml` | **0** | 16 个测试全部通过、0 失败、0 doc-tests。分布：lib 0；`dev_server` 单测 2；集成 `d01`–`d08` 各 1（共 7）；`review_fixes_test` 7 |

`dev_server` 的两个单测为本 M3 关注的代码级证据：`grant_has_inclusive_start_exclusive_end_and_matches_source_and_device`、`failed_commit_rolls_back_and_stops_new_admission`（本次 `cargo test` 真实运行通过）。

## 4. 干净实例启动（实测）

```sh
M3_DIR=$(mktemp -d /tmp/iotfast-m3.XXXXXX)          # 全新空目录，0700
export IOTFAST_DEV_DATA_DIR="$M3_DIR"
export IOTFAST_DEV_BIND=127.0.0.1:18401             # 启动前 lsof 确认空闲；未触碰被 ssh 占用的 18080
export IOTFAST_DEV_INGEST_TOKEN=$(openssl rand -hex 32)
export IOTFAST_DEV_READER_TOKEN=$(openssl rand -hex 32)
export IOTFAST_DEV_ALL_TOKEN=$(openssl rand -hex 32)
DYLD_LIBRARY_PATH="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0" \
  ./prototype/target/debug/dev_server
```

- stderr 首行：`DuckDB SELECT version(): v2.0.0`。
- 数据目录实际生成 `engine.duckdb`（12288 B，`-rw-------` 0600）与 `engine.duckdb.wal`（DuckDB 自身 WAL）；**无应用层 `wal/` 目录**，与“仅声明 volatile”一致。
- 三个令牌为 `openssl rand -hex 32` 生成的 64 位十六进制、互不相同；**真实值未写入本文件、日志或 URL**。
- 每个断言组均在**单次干净实例**上执行；本 harness 首次运行时 3 条校验因我方用例时间戳设计错误而失败（见 §5 注），修正后以**全新空目录 + 全新随机令牌**重启重跑，下表数据取自该次干净运行。

## 5. 关键闭环抽验（干净实例真实 HTTP，20/20 PASS，0 FAIL）

方法：Python `http.client` 对 `127.0.0.1:18401` 上的真实进程发起请求，逐项记录状态码与关键字段。`ingest` 的异步提交用有界轮询（≤50 次、间隔 100 ms）确认。

| ID | 请求 | 预期 | 实测关键字段 | 结果 |
|---|---|---|---|---|
| H1 | `GET /health`（无认证） | 200，profile/volatile，不含秘密 | 200 `{"status":"ok","profile":"orca-mvp-dev-v1","durability":"volatile"}`，仅 3 键，无 token/tenant/scope/path | PASS |
| A1 | 无凭据 `POST /v1/telemetry` | 401 `UNAUTHENTICATED` | 401 `UNAUTHENTICATED` | PASS |
| I1 | `gateway-demo`+demo-a=245.6 good | 202 `accepted_volatile`+`pending`+ingest_id | 202 `status=accepted_volatile, history_status=pending, ingest_id=ing-1` | PASS |
| I2 | 轮询同 key | 有界次数内 `history_status=committed` | 第 1 次轮询即 200 `duplicate, history_status=committed` | PASS |
| D1 | 相同 body 再发 | 200 `duplicate`，同 ingest_id | 200 `status=duplicate, ingest_id=ing-1, history_status=committed` | PASS |
| D2 | 同 `message_id` 改 value（245.6→246.1） | 409 `IDEMPOTENCY_CONFLICT` | 409 `IDEMPOTENCY_CONFLICT` | PASS |
| U1 | `Prefer: durability=durable` | 503 `DURABILITY_BACKPRESSURE`，retryable/retry_after | 503 `DURABILITY_BACKPRESSURE, retryable=true, retry_after_ms=1000` | PASS |
| U2 | durable 拒绝后**同 key** 正常请求 | 202（durable 拒绝无副作用、不占 key） | 202 `accepted_volatile, ingest_id=ing-2` | PASS |
| V1 | `GET /v1/devices/demo-a/view`（ALL） | value=245.6（不二次乘 0.1），good/fresh/float64/Cel，采样时间=发送值 | 200 `value=245.6, quality=good, freshness=fresh, observation_state=observed, data_type=float64, unit={Cel,°C}, value_sampled_at_ms==发送值` | PASS |
| V2 | 间隔 1.1s 再 GET | 采样时间不被 GET 刷新 | `value_sampled_at_ms` 与上次完全一致，value 仍 245.6 | PASS |
| V3 | demo-a/b 共享同模型，latest 隔离 | demo-a=245.6、demo-b=31.2 同时成立 | 202/202；demo-a=245.6、demo-b=31.2，两者 `model_revision_id=mr-demo-1`，互不影响 | PASS |
| V4 | 上报 demo-a=50.5 quality=bad（较新采样） | 实测值保留、quality 仍 bad | 200 `value=50.5, quality=bad, observation_state=observed` | PASS |
| H2 | `POST /v1/history/query` raw asc | 200，全 item 字段齐全，严格非降序 | 200 `mode=raw,next_cursor=null,partial=false`；items=3，含 `observation_id/resource_id/device_id/property/model_revision_id/binding_revision_id/source_at_ms/received_at_ms/source_time_origin/value_type/value/unit/quality/source_quality_code/replayed/record_kind`；asc 有序 | PASS |
| H3 | raw desc | 200，严格非升序 | 200，items=3，desc 有序 | PASS |
| H4 | `limit=1` 但结果 >1 | 422 `VALIDATION_FAILED`，`details.issue=RESULT_LIMIT_EXCEEDED`，无 items/cursor | 422 `VALIDATION_FAILED`，`details=[{field:"limit",issue:"RESULT_LIMIT_EXCEEDED"}]`，无 items | PASS |
| S1 | reader 读 demo-b；reader 混合两资源历史 | 各 404 `RESOURCE_NOT_FOUND` | view=404、mixed history=404，均 `RESOURCE_NOT_FOUND` | PASS |
| S2 | reader 查询自身授权资源 | 200 | 200，items=3 | PASS |
| S3 | 未知 `source_id=gateway-evil` | 403 `SOURCE_NOT_AUTHORIZED` | 403 `SOURCE_NOT_AUTHORIZED` | PASS |
| T1 | 未来 +120 s | 422 `CLOCK_SKEW_EXCEEDED` | 422 `CLOCK_SKEW_EXCEEDED` | PASS |
| T2 | demo-b 使用 demo-a 绑定（附） | 409 `REVISION_CONFLICT` | 409 `REVISION_CONFLICT` | PASS |

**统计：20 个断言组，20 PASS，0 FAIL**（清理后 `pgrep -f 'target/debug/dev_server'` 无残留）。

> 诚实记录：首次运行出现 V1/V2/V3 三条 FAIL，实测 `value=11.0` 而非 245.6。定位为**我方 harness 用例设计错误**——durable 恢复请求使用了比 245.6 更晚的 `source_at_ms`，服务据“严格较新才替换 latest”正确把 11.0 提升为 latest，属**正确行为而非缺陷**。将该恢复样本的采样时间改为早于 245.6 后，干净实例重跑全部通过。

## 6. 启动闸门抽验（Python 子进程 + 超时，7/7 PASS）

| ID | 配置 | 退出码 | 是否监听 | 结果 |
|---|---|---|---|---|
| G1 | `IOTFAST_DEV_BIND=0.0.0.0:18411` | 1 | 否 | PASS（`invalid loopback bind`） |
| G2 | 缺失 ingest 令牌 | 1 | 否 | PASS（`missing development token`） |
| G3 | 三个令牌相同 | 1 | 否 | PASS（`tokens must be distinct`） |
| G4 | 非空数据目录 | 1 | 否 | PASS（`data directory must be absolute and empty`） |
| G5 | 63 位十六进制令牌（附） | 1 | 否 | PASS（`invalid development token`） |
| G6 | 相对数据目录（附） | 1 | 否 | PASS（`data directory must be absolute and empty`） |
| G7 | 基线：loopback + 空目录 + 随机令牌 | 运行中 | 是（18417 可连） | PASS（可启动并监听） |

以上均以真实子进程启动、5 s 超时判定：非法配置在超时前非零退出且端口不监听；基线下正常监听。超时未退出的情形未被误判为通过。

## 7. 代码核对：DB 失败后全局 503 是否为缺陷

**结论：不是缺陷，是冻结契约要求的行为。** 依据（只读核对 [dev_profile/mod.rs](../../../prototype/src/bin/dev_profile/mod.rs)）：

- `commit()`（第 245–264 行）：DuckDB 事务 `tx.commit()` 失败时，`state.failed = true`（第 259 行），并把该 receipt 状态置 `failed`。
- `ingest()`（第 537–549 行）：先查重复 key（第 538–546 行，已知 key 仍返回 `duplicate`，与计划“已知 key 仍可返回 duplicate”一致），随后 `if locked.failed { return 503 DURABILITY_BACKPRESSURE }`（第 547–549 行），对**所有新消息**停止准入。
- [M0 计划 §5 第 6 条](../orca-mvp-plan.md)（第 118 行）明确要求：“DB 提交失败须回滚整个事务，登记 failed 并停止新消息准入（503），不得返回虚假 committed 或在 latest 留半批”。
- [Lead §7 注记](lead-mvp-independent-verification.md)（第 90–93 行）与本核对一致：这属契约行为；**不得在同一实例上先做故障注入再做普通断言**——否则后续 503 是测试顺序造成的假象，不是服务违约。正确做法是把“DB 失败→回滚→停止准入”作为最后断言且用独立实例。

**证据性质声明：以代码核对 + M1 单元测试为证据，未做 HTTP 注入。** 在不修改源码的前提下，无法通过 HTTP 在干净实例上真实触发底层 DuckDB 提交失败（`message_id`→`observation_id` 派生命令使正常请求不会产生主键冲突）。本 M3 **刻意未**在干净实例上做任何故障注入，也未据该行为判实现缺陷；`failed_commit_rolls_back_and_stops_new_admission` 单测在本次 `cargo test` 中通过，覆盖回滚/停止准入的内部逻辑，但**不等于** HTTP 层容量/回滚实测（见 §9）。

## 8. 代码核对：`source_device_grant` 的 `[valid_from_ms, valid_to_ms)` 逐请求校验

- 建表与写入（[dev_profile/mod.rs](../../../prototype/src/bin/dev_profile/mod.rs) 第 215–218 行）：`CREATE TABLE source_device_grant(source_id, device_id, valid_from_ms, valid_to_ms, PRIMARY KEY(source_id,device_id))`，为 `gateway-demo` × `demo-a`/`demo-b` 各写入 `[started, started + 3_600_000)`。
- 逐请求校验（第 173–179 行 `source_granted`）：SQL 为 `... WHERE source_id = ? AND device_id = ? AND valid_from_ms <= ? AND ? < valid_to_ms`，**半开区间 `[valid_from, valid_to)`**；调用处 `checked_at = now()`（第 447 行）在**每个 ingest 请求**按服务端当前时间查询（第 448 行），且 grant 检查发生在去重与结构校验之前。
- 单元测试（第 588–597 行 `grant_has_inclusive_start_exclusive_end_and_matches_source_and_device`）覆盖边界 `99=false、100=true、199=true、200=false` 及 source/device 不匹配，本次 `cargo test` 通过。这补上了 Lead §5 第 1 条早期指出的“无独立 grant 记录”差距；独立 DeepSeek M2 §7 第 4 条亦记录实现已含显式 grant 表并逐请求检查。
- **未实测**：真实时间到期点（`t >= valid_to_ms`，窗口 1 小时）未等待实测；`valid_from_ms` 尚未生效的 HTTP 端拒绝亦未实测。仅有代码核对 + 注入时间单测覆盖，属“真实时间到期未实测”。

## 9. 未覆盖项（单列，不得当作通过）

1. 令牌一小时到期边界（`started_at_ms + 3_600_000`）真实拒绝：未实测。
2. `source_device_grant` 的 `valid_to_ms` 真实时间到期拒绝：未实测（仅代码 + 注入时间单测）。
3. 容量/限流：10000 receipt、256 pending → 429 未在 HTTP 层触发。
4. DB 提交失败 HTTP 级真实注入 → 503 停止准入：仅代码核对 + M1 单测，未做 HTTP 注入。
5. 同 `message_id` 并发串行化/竞态：未做并发压力测试。
6. JCS `sha256-jcs-v1` 与 `observation_id` 黄金向量独立重算：未独立复算，依赖服务内部一致性（重复/冲突行为符合预期）。
7. 深度 >8 的纯嵌套样本单独隔离：仅有 `valid_depth` 上限 8 代码与 body 上限路径，未单独构造深样本。
8. 隔离/干净构建与实际动态库解析路径：复用 target 缓存，未核实进程实际解析的 .dylib 路径。
9. 崩溃恢复/断电无损/跨会话去重：范围外。
10. 真实网关/TLS/生产认证/性能/部署/UI/控制/外部 TSDB/WS：范围外。

## 10. 残留风险（按严重度排序）

| 严重度 | 风险 | 说明 |
|---|---|---|
| 中 | 运行时边界行为仅代码/单测级证据 | 令牌到期、grant 到期、容量 429、DB 失败 503 未在 HTTP 层实测；若边界实现有偏差不会被本次发现 |
| 中 | 构建可重复性 | 未做干净隔离构建；`dev_server` 依赖历史缓存 `libduckdb.dylib`（无 `LC_RPATH`），直接执行需显式 `DYLD_LIBRARY_PATH`；换机/清缓存后启动方式需按 runbook 保证 |
| 中 | M1 格式化事故的溯源风险 | M1 曾执行作用域为整 crate 的 `cargo fmt`，改动既有未跟踪重组源码与 5 个已跟踪测试文件，原始未跟踪内容无法由 Git 恢复、未证明语义等价。本次 `cargo test` 全绿只能证明当前树可编译可测，不等于与格式化前语义等价 |
| 低 | 传输层拒绝非统一 JSON | 慢 body/超大声明可能在写响应后关连接，部分客户端见 `ConnectionResetError`；契约只要求“耗尽内存前限制长度/读取时间”，记为行为说明 |
| 低 | 单机隔离边界 | 本地明文 loopback 演示，同机其他进程仍可能接触服务；非恶意本机用户隔离保证 |
| 范围外 | 生产/可靠性/性能/真实网关/TSDB/WS/UI/控制 | 均未实现或未验证，不因本 PASS 改变 |

## 11. 分级结论

**演示范围内功能：PASS-with-risks（可接受）。**

- **支撑证据**：构建退出 0、`cargo test` 16/16 退出 0（§3）；干净实例 20/20 闭环断言 PASS（§5）；启动闸门 7/7 PASS（§6）；DB 失败 503 与 grant 时间窗的代码核对符合契约（§7、§8）。工程值不二次换算、质量/新鲜度、两设备 latest 隔离、有界 raw 历史与超额拒绝、reader 越权 404、未知 source 403、未来时间 422、durable 显式拒绝且无污染，均有真实请求证据。
- **为什么不是纯 PASS**：§9 多项边界仅代码/单测级、未 HTTP 实测；构建非干净、库路径为历史缓存；M1 格式化事故的语义等价未证明。这些使结论保留风险，故定为 PASS-with-risks 而非 PASS。
- **不是 FAIL**：所有必测的演示闭环与契约关键条款均通过，未发现实现与冻结契约的冲突；已知“DB 失败后持续 503”属契约要求而非缺陷。

**边界重申**：本结论只覆盖 `orca-mvp-dev-v1` 本地只读演示子集；**不是生产验收、不是完整 P0、不含 UI/控制/外部 TSDB/WS**；不代表 D00–D10 或生产台账 Q01–Q12 关闭。

## 12. 清理

- 本 M3 启动的所有 `dev_server` 进程已按 PID 结束（SIGINT，必要时 SIGKILL），启动前未触碰被 ssh 占用的 18080。
- 本次使用的临时数据目录（`/tmp/iotfast-m3.*`）与启动闸门临时目录均已删除；harness 结果文件在 `/tmp/iotfast-m3-results.json`、`/tmp/iotfast-m3-gates.json`（非仓库文件）。
- 结束自查 `pgrep -f 'target/debug/dev_server'` 无残留。

## 13. Terra 续派独立验收（2026-11-10）

本节追加而不改写此前结论。任务 `task_59b589fdea8f` / `ctx_e3b158839d9f`，实际执行者为本次派发终端，模型由协调者记录为 `auto/gpt-6.1-terra`。本轮独立构建、启动、请求和查询，不把 M2 证据当成本轮实测；smoke 脚本与只读 DuckDB C API 类复用 M2 资产，独立探针请求、断言、进程和数据目录由本轮编写和创建，因此不是完全独立测试实现。此前另一任务的失败及其探针错误保留在 `docs/development/evidence/orca-mvp-m3.md`，不由本节改写。

### 环境、命令与证据

- 环境：Darwin 27.0.0 arm64；Rust `1.98.1 (48a229cea 2026-09-01)`；Cargo `1.98.1 (797e8a9bc 2026-08-05)`；Python `3.14.7`；Git `2.54.0 (Apple Git-157)`。服务启动日志及独立只读 DB 查询均显示 DuckDB `v2.0.0`，动态库 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`。
- `git status --short > /tmp/m3-terra-status.txt`：退出 0，记录既有 dirty 基线；不 reset、clean、checkout 或覆盖已有修改。
- `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server > /tmp/m3-terra-build.log 2>&1`：退出 0；复用当前 target，不是干净隔离构建。
- `python3 scripts/orca-mvp-smoke.py > /tmp/m3-terra-smoke1.log 2>&1`：退出 0，106/106，结果 `/tmp/iotfast-m2-e41jvu8f/results.json`。
- `python3 scripts/orca-mvp-smoke.py > /tmp/m3-terra-smoke2.log 2>&1`：退出 0，106/106，结果 `/tmp/iotfast-m2-pd9b5i0x/results.json`。结果文件内模型和任务字段是脚本硬编码的 M2 标识，不代表本次执行者。
- `python3 /tmp/m3-terra-probe.py > /tmp/m3-terra-probe.log 2>&1`：退出 0；完整可复现探针为该临时脚本，实际服务启动可执行文件 `./prototype/target/debug/dev_server`，空闲端口 `127.0.0.1:60599`，全新空目录 `/tmp/iotfast-m3-terra-78gu5rrl/data`，三组独立随机 64 位十六进制令牌仅注入环境；仅对本进程 SIGINT，服务退出 -2 是信号终止而非探针失败。日志和数据库保留在 `/tmp/iotfast-m3-terra-78gu5rrl`，不输出秘密。
- `git diff --check -- docs/development/evidence/orca-mvp-m3-independent-acceptance.md`：退出 0；该文件未跟踪，另以 Python 检查追加文件的尾随空白和末尾换行。

### 冻结契约核对

| 项目 | 结果 | 实际与限制 |
|---|---|---|
| 实际绑定和启动 | PASS | smoke 实测非 loopback 拒绝并不监听；本轮新服务使用 loopback 和新目录 |
| 认证与固定范围 | PASS（受限） | 独立错误令牌 401、伪造 Scope 422、reader 读 B 与混合历史 404、body 自报 scope 422；错误信封字段存在且不包含令牌 |
| reader 写入及 reset | PASS | 独立 20 次 reader POST telemetry 均 403 `FORBIDDEN_ACTION`，无 reset |
| 设备隔离和工程值 | PASS | 独立写 A=245.6、B=31.2，latest 分别原值，未二次换算 |
| ACK 和 durable | PASS（限定新消息） | 独立 durable 请求 503 `DURABILITY_BACKPRESSURE`，没有 `accepted_durable`；两条后续新消息 202 `accepted_volatile` + pending；独立 DB 恰好 2 行 |
| raw 历史 | PASS | 独立核对数值、采样时间、质量、单位、模型/绑定版本；升降序、`[start,end)` 和 null cursor 均符合，限制范围由 smoke 验证 |
| 拒绝的原子性、质量及乱序 | PASS（限定样本） | 两轮实际 smoke 覆盖非法整批拒绝、无历史副作用、质量/时间和乱序；不代表所有并发情形 |
| 独立只读 DuckDB 和授权窗口 | PASS | SIGINT 后 `READ_ONLY` 查询为 2 条 observation；`gateway-demo` 对 demo-a / demo-b 两条 grant，分别 `[1794289050818,1794292650818)`，窗口均 3600000ms |
| 未实现边界 | PASS（拒绝行为） | 独立请求 source-state、ingestion、shadow、subscriptions、control 均 501；smoke 检查未知路径及不支持能力 |
| WAL、旧目录恢复、生产 Q | 不适用 | 仅 volatile；不宣称 WAL 联合恢复或关闭 Q01–Q12 |
| 真实一小时到期、HTTP 429 容量、独立进程 DB 提交失败注入、干净隔离构建、真实网关/生产认证/恢复/性能 | 未执行 | 不计 PASS；不代表生产验收或完整 P0 |

本轮必需断言通过，未发现上述抽查与冻结契约的实质偏差；结论仅限 `orca-mvp-dev-v1` 合成本地只读 profile，不是生产安全、可靠性、性能签收，不涵盖 UI、设备控制或外部 TSDB。历史源码格式化事故和工作区既有 dirty 内容未由本轮证明语义等价。
