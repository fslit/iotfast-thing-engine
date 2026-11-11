# M2 独立 HTTP smoke 证据

任务 `task_0a73f16941fc`，dispatch `ctx_b616386522e7`，日期 2026-11-10。执行模型为协调者应急替换的 `auto/kimi-k3.1-pro`；此前 Terra 路由 404/503，本证据不是 Terra 验证。
后续续派：`task_a688916d7f97` / `ctx_ee07e8d9fffb`，协调者裁决增强 durable 请求关联后至少两次干净复跑，不改变断言或服务实现。
最终重试：`task_0a73f16941fc` / `ctx_04c09ae29684`，在 M1-R2 早响应修复后执行最终构建、连续两次 smoke 和终验；未修改服务、wire、plan 或 status。

## 范围与停止条件

- 允许写：`scripts/orca-mvp-smoke.py`、`docs/development/orca-mvp-runbook.md`、本证据；未选择 shell 包装脚本，未新增 fixture。
- 禁止写：manifest/公共文件/服务源码、M0 plan、协调者 status 及他人 dirty 内容。不 reset/clean/commit/revert，不切换 checkout，不启动子 worker。
- 输入：M0 `orca-mvp-dev-v1`、M1 evidence、真实 dirty checkout（HEAD `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`）。目标为真实本地 HTTP 与真实 DuckDB，不是 mock 单测。
- 主命令：`python3 scripts/orca-mvp-smoke.py`；前置 `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server`；最终连续运行两次并执行 `git diff --check -- scripts/orca-mvp-smoke.py docs/development/orca-mvp-runbook.md docs/development/evidence/orca-mvp-m2.md`。
- 停止条件：任一必需断言失败或非零退出即失败，保留脱敏结果并通过 Orca ask 回协调者，由 M1 返修；不放宽测试、不改服务。

## 环境

- OS：Darwin 27.0.0 arm64；Git 2.54.0；rustc/cargo 1.98.1。
- Python、实际 DuckDB 引擎、实际动态库路径、端口和临时目录：待运行记录。
- 每次 smoke 使用独立 `/tmp/iotfast-m2-*`、随机秘密及空闲 loopback 端口；只管理自己的 PID，成功清理，失败保留脱敏证据。

## 执行记录

### 首轮：失败，等待协调裁决

- Python 内联 `compile(...)`：退出 0。
- `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server`：退出 0，复用现有 target。
- `python3 scripts/orca-mvp-smoke.py`：退出 1；启动、认证、Scope 与初始空视图等 26 项记录通过后，在首次 durable POST 附近抛出 `ConnectionResetError: [Errno 54] Connection reset by peer`，尚未得到该请求的响应。脱敏结果保留于 `/tmp/iotfast-m2-od73_xmd/results.json`；实际服务 stderr 为 `DuckDB SELECT version(): v2.0.0`，创建的服务进程已由脚本回收。
- 独立最小复现使用新的空目录、随机秘密、loopback 空闲端口和同类合法 durable 请求：`http.client` 得到 503 `DURABILITY_BACKPRESSURE`，curl 也得到 503，复现程序退出 0。因此当前失败尚不能归因为服务断言违约，也不能把原首轮非零隐藏为通过；需协调者判断是先修 smoke 客户端/记录还是由 M1 排查传输时序。
- 首轮首次辅助复现曾把 stderr 日志放入服务数据目录，导致空目录前置不满足并连接拒绝；该辅助命令退出 1，已纠正为独立日志目录后才得到上述 503。它不是服务缺陷证据，保留此说明避免选择性记录。

后续两次连续 smoke、独立 DuckDB 核对与最终 diff 尚未完成。本文不是生产验收，也不宣称 P0 完成。

### 续派准备

- 通过 `orca orchestration check --terminal term_a362bfc2-8eab-4ece-a22f-2f5a227ffbc0` 读到裁决 `msg_a17a8f5d8f8d`、`msg_77774adbaae1`、`msg_24841ccd5c3e`、`msg_467f225b61b0`。此前无参数 check 未取到消息，不能据此认定协调者未答复。
- smoke 已为 durable POST 增加固定 `X-Request-Id`、方法/路径、请求体完整发送标志、预期/实际状态、响应 headers/body、异常类型和毫秒耗时；结果写入 `results.json` 的 `durable_traces`。未修改 `prototype/src`、wire 或服务。

### 增强后调试轮：测试脚本自身错误

- `python3 scripts/orca-mvp-smoke.py`：退出 1，证据 `/tmp/iotfast-m2-eh8g02mm/results.json`。durable 请求完整发送，0.384 ms 内实际返回 503 `DURABILITY_BACKPRESSURE`，未复现首轮 reset；随后 smoke 的通用 `ingest()` 错把该 503 当 202 断言，属于测试脚本控制流错误，不是服务缺陷。
- 已仅修改 smoke：durable 请求直接交给 `expect_error(...503...)`，通用 `ingest()` 只用于预期 202 的新消息。503 断言未放宽，首轮 reset 与本次脚本错误均保留。
- 下一次运行退出 1，证据 `/tmp/iotfast-m2-_a81sz4z/results.json`：durable 503 及 volatile 202 均通过，但 smoke 等待历史时错误使用 `[1, now+1000)`，超过 plan 的 3600000 ms 上限而收到 422。已把等待窗口限制到最近一小时，并把其他需要成功或特定错误的 raw 查询改为 `stale_sample-1` 起点；宽跨度反例仍单独保留，历史条数与字段断言未放宽。
- 后续运行退出 1，证据 `/tmp/iotfast-m2-mzc7bhij/results.json`：durable 未再 reset，工程值、乱序、数值 seq 等均通过；smoke 用客户端构造时刻 `+60001 ms` 的未来样本获得 202。独立新进程实测 `client_before=1794273169973/source=1794273229974/client_after=1794273169978` 时 `+60001` 获 202，而下一请求 `+61000` 获 422 `CLOCK_SKEW_EXCEEDED`，说明服务处理期间客户端毫秒钟前进，构造偏移不等于接收偏移，不能把该边界竞争直接归为服务缺陷。主 smoke 改用明确超过阈值的 `+61000 ms` 验证拒绝；精确 `+60001` 接收边界仍需可注入可信时钟，不宣称 HTTP 已覆盖。
- 协调者后续 `msg_248f3e53f8ea` 独立确认同一 `+60001` 裕量竞态并要求不得接受 202；主 smoke 进一步固定为 `+120000 ms`，仍严格断言 422 `CLOCK_SKEW_EXCEEDED`。权威 M2 卡为 `task_0a73f16941fc`，当前续派卡仅承接执行，不另建第二个 M2。
- 再后一次运行退出 1，证据 `/tmp/iotfast-m2-vpcqu1b4/results.json`：HTTP 启动、认证、工程值、质量、顺序、原子拒绝、raw、501、重复 JSON key、SIGINT 等均通过，独立 DuckDB `SELECT version()` 被 smoke 判为空。用同一保留 DB 的 ctypes 最小探针确认 `duckdb_query` 状态 0，结构体弃用字段 `row_count=0`，而官方函数 `duckdb_row_count()` 返回 1 且 `duckdb_value_varchar()` 返回 `v2.0.0`；这是 smoke C API 读取错误。已改用行/列函数，查询仍为独立 `READ_ONLY` 连接，不断言服务侧结果代替直接 DB。
- 为满足两次结果可审查，成功运行也保留脱敏 `results.json` 与服务日志，只删除数据子目录；失败仍保留完整现场。
- 在最终两跑前补充：显式/非法 `X-Request-Id`；独立只读 DuckDB 核对两条 grant 的设备及一小时区间；按公共 §4.2 独立构造 JCS 并计算 SHA-256，与 `committed_receipt.payload_hash` 比对。上述均不改变发送消息数量。
- 补充用例首跑退出 1，证据 `/tmp/iotfast-m2-w8yoa8ks/results.json`：脚本误删原本为 `modbus` 的 `source_protocol`，服务正确返回 409；这不是 missing/null 等价用例。已改为删除原值为 null 的 `source_code`，保留该脚本错误，不放宽幂等冲突断言。
- 协调者 `msg_3300bf6b70c1` 要求 replay 仅改变 `delivery.replayed`，最终脚本已删除所有 quality 字段变更；保留 200 duplicate、原 ingest_id、committed 断言。
- 随后一跑在 wrong Content-Type 预期 422 前再次出现 `ConnectionResetError`，证据 `/tmp/iotfast-m2-vtfsoafl/results.json`。协调者 `msg_7247401308df` 允许仅对预期 4xx/5xx 的无副作用拒绝请求用新连接最多两次尝试；脚本现记录 `reset_traces` 的 request 关联、attempt、异常和最终实际响应，只有读到预期错误才 PASS，预期 202 写入不重试，两次 reset 仍失败。

### 最终脚本受阻：预期 403 连续两次 reset

- `python3 scripts/orca-mvp-smoke.py > /tmp/iotfast-m2-final-run1.log 2>&1`：退出 1，结果 `/tmp/iotfast-m2-begnr818/results.json`。
- 最小关联：`POST /v1/telemetry`，使用有效 **reader** token 和合法 telemetry body，预期 `403 FORBIDDEN_ACTION`；attempt 1 与 attempt 2 都在新连接抛出 `ConnectionResetError: [Errno 54] Connection reset by peer`，没有实际状态/headers/body，因此未记 PASS。服务在此前的启动、认证、Scope、请求 ID 与 gateway read 403 检查后可运行，脚本只回收自己创建的进程。
- 该请求位于 M03 reader write；reset trace 的 `request_id=null`（未提供可选 X-Request-Id），方法/路径和两次异常完整保留。最终脚本尚未取得连续两次 exit 0，不能 worker_done succeeded；交由 M1 排查动作拒绝早响应与请求 body 的传输时序。

## 结案注记（Lead，2026-11-10 09:16）

- 本文件记录的是 Orca 原生 M2（`ctx_b616386522e7`，应急模型 `auto/kimi-k3.1-pro`）的一次**未完成**执行：provider 路由 404/503 与一次客户端 `ConnectionResetError` 后未产出最终 smoke 结论，任务已置 `failed`，失败历史保留。
- 为不因外部 provider 故障阻塞验收，独立验证由另一独立 AI（DeepSeek V4.1 Flash）在**全新空实例**上完成：24 组断言 24 PASS，见 [orca-mvp-m2-independent-deepseek.md](orca-mvp-m2-independent-deepseek.md)。
- 关于复跑中出现的“合法请求 503”：经代码核对，这是 `commit()` 失败后置 `failed` 并按 M0 计划 §5.6 停止新消息准入的**契约行为**，不是缺陷；详见 [lead-mvp-independent-verification.md](lead-mvp-independent-verification.md) §7。该 503 不应被判为实现违约，但也不得据此掩盖其它真实断言失败。
- 最终演示范围验收结论见 [orca-mvp-m3-independent-acceptance.md](orca-mvp-m3-independent-acceptance.md)。

## 最终重试结果（ctx_04c09ae29684）

上文“执行中”“最终脚本受阻”和 Lead 结案注记均保留为历史记录；本节是当前 dispatch 的最终实测状态。当前重试没有修改 smoke 断言或服务代码，使用既有最终脚本和 runbook。

### 环境与命令

- HEAD：`c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`，真实 dirty checkout；未 reset/clean/revert/commit/换 checkout。
- OS：macOS 27.0 (26A428)，Darwin 27.0.0 arm64；Rust/Cargo `1.98.1`；Python `3.14.7`；Git `2.54.0 (Apple Git-157)`。
- 二进制：`prototype/target/debug/dev_server`；实际动态库：`prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`。
- 服务启动日志与停止后的独立 `READ_ONLY` DuckDB `SELECT version()` 均为 `v2.0.0`；不是根据 crate 名推断。

实际执行的主命令：

```sh
cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server > /tmp/iotfast-m2-ctx04-build.log 2>&1
python3 scripts/orca-mvp-smoke.py > /tmp/iotfast-m2-ctx04-run1.log 2>&1
python3 scripts/orca-mvp-smoke.py > /tmp/iotfast-m2-ctx04-run2.log 2>&1
git diff --check -- scripts/orca-mvp-smoke.py docs/development/orca-mvp-runbook.md docs/development/evidence/orca-mvp-m2.md
```

| 命令 | 退出码 | 结果与证据 |
|---|---:|---|
| 上述 `cargo build` | 0 | `Finished dev profile`；日志 `/tmp/iotfast-m2-ctx04-build.log`，复用现有 target，不声称干净隔离构建 |
| 第一次 `python3 scripts/orca-mvp-smoke.py` | 0 | 106/106 PASS，`reset_traces=0`；`/tmp/iotfast-m2-78gc8onl/results.json`，完整输出 `/tmp/iotfast-m2-ctx04-run1.log` |
| 第二次 `python3 scripts/orca-mvp-smoke.py` | 0 | 106/106 PASS，`reset_traces=0`；`/tmp/iotfast-m2-mtqprtsb/results.json`，完整输出 `/tmp/iotfast-m2-ctx04-run2.log` |
| 上述 `git diff --check` | 0 | 无空白错误输出；三个文件当前均未跟踪，因此另做直接正文检查 |

两次运行分别使用独立 `/tmp/iotfast-m2-78gc8onl` 与 `/tmp/iotfast-m2-mtqprtsb`、随机 64 位十六进制秘密及自动选择的空闲 loopback 端口。启动拒绝用例也各用新端口；主服务端口分别为 `57247`、`57396`，停止后均确认端口释放。脚本只向自己创建的 PID 发送 SIGINT，必要时才对同一 PID 升级终止，不使用 `pkill`；成功后删除自己的数据子目录，仅保留脱敏 `results.json` 和服务日志，失败才保留完整现场。两次结果目录均只剩这两个文件，没有遗留服务数据目录。

### 断言覆盖与结果

| 范围 | 当前结果 | 实测摘要与边界 |
|---|---|---|
| M01 启动 | 通过 | 非 loopback、缺失/非法/相同 token、非空目录均非零且不监听；非空目录原文件保留；health 明确 `volatile` |
| M02 认证与固定授权 | 部分通过 | 缺失/错误/多重凭据 401；请求 ID 校验；独立 DuckDB 核对两条固定设备 grant 和一小时区间。真实等待到期、grant 尚未生效/已到期 HTTP 时序未执行，精确时间边界仍依赖 M1 可注入时钟测试 |
| M03 Scope 与动作隔离 | 通过 | 自报 header/body/query Scope 拒绝；reader 读 B、混合 A+B 历史均 404；gateway 读和 reader 写均 403，reader-write 未再 reset |
| M04 工程值与设备隔离 | 通过 | 初始值为 null；A=`245.6`、B=`31.2` 不串值且未乘绑定 factor 0.1；单位、采样时间、模型/绑定版本明确 |
| M05 严格 wire 与原子拒绝 | 通过 | 未知/重复 JSON key、未知字段/spec、重复点、null、Rust 枚举值、错类型、超范围、非法 seq、空/坏/过深/超限 body、错 Content-Type 均 422；混有非法点的整批无历史副作用，同 message_id 修正后成功 |
| M06 绑定与来源 | 通过 | 错绑定/模型 409；错 source 403；不支持 epoch/time origin 422 `CAPABILITY_UNSUPPORTED` |
| M07 质量、时间与顺序 | 通过 | bad+fresh 不伪 good；good+stale 保留原采样时间；乱序与同毫秒 `999/1000/1001/1002` 按数值 seq，latest 保新；明确超出阈值的 future 样本 422。精确 `+60001ms` 接收边界未用 HTTP 宣称覆盖 |
| M08 幂等与确定性 ID | 通过（限定范围） | replay 仅改变 delivery 时返回原 ingest_id/committed；改值 409；9 个 observation ID 与独立黄金计算一致，JCS payload hash 与 DuckDB 一致。不是完整跨语言 JCS/Schema 验收 |
| M09 ACK 与背压 | 部分通过 | 新消息 202 `accepted_volatile` + `pending`；durable 新请求 503 且 latest 无副作用；未出现 `accepted_durable`。durable duplicate、真实队列/session 上限 429 和独立进程 DB 故障回滚未执行 |
| M10 有界 raw 历史 | 通过 | 真实 HTTP 返回 raw 工程观测、版本/单位/质量；asc/desc、`[start,end)`、质量过滤、空结果、双设备、limit+1 整次拒绝及非法/cursor/aggregate 查询均符合 plan；无分页或任意 SQL 宣称 |
| M11 最终可见与独立 DB | 通过（非 durable） | 有界等待后历史可见；SIGINT 后独立 DuckDB 核对 `v2.0.0`、9 行、全部 ID、grant 和 hash。plan 要求每次新空目录且禁止旧演示自动恢复，因此旧目录重启恢复不适用；两次重复运行不证明持久恢复或 durable |
| M12 未实现边界 | 通过（拒绝行为） | source-state、ingestion 查询、shadow、WS、动态配置均 501；未知路由 404；无 CORS。只证明明确拒绝，不证明这些能力实现 |

### 未执行、不适用与结论边界

- 未执行：真实一小时到期、真实 HTTP 队列/10000 消息容量、独立进程 DB 提交失败、durable duplicate、精确时钟边界、并发/性能、崩溃/物理断电、生产认证与真实网关。上述项不计 PASS。
- 不适用：plan 明确选择新空目录、仅 volatile、无游标全结果、无 WS/source-state/动态配置，所以旧目录自动恢复、稳定分页和这些功能的正向行为不属于本 profile 验收。
- 本次没有新增未通过断言；历史非零、脚本错误和 reset 均保留在前文，未被两次最终成功改写。最终两次各 106 项通过只证明 `orca-mvp-dev-v1` 合成开发 profile 的本机 HTTP/DuckDB smoke，不是生产验收、完整 P0/D00–D10、可靠性、安全或性能签收。

### 终验记录

- `git diff --check -- scripts/orca-mvp-smoke.py docs/development/orca-mvp-runbook.md docs/development/evidence/orca-mvp-m2.md`：退出 0，无输出。
- 直接读取上述三个未跟踪文件，逐行检查 trailing whitespace、末尾换行，并以 `compile(...)` 检查 smoke Python 语法：退出 0，输出 `PASS: final newline, trailing whitespace, Python syntax for all M2 deliverables`。该补充避免把 Git 不遍历未跟踪正文误报为完整内容检查。
- 最终两跑、构建和上述检查均为 0；本卡交付范围为 `scripts/orca-mvp-smoke.py`、`docs/development/orca-mvp-runbook.md` 和本证据，未选择 shell 包装脚本，未新增 fixture。
