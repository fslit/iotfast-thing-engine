# M1 工作记录（未完成）

任务 `task_cf842168c554`，dispatch `ctx_54c1ec8d5434`，日期 2026-11-10。
允许修改 prototype manifest、源码、测试（不含 M2 smoke）、示例、演示 fixture、必要运行产物忽略规则及本证据；禁止修改计划、status、其他文档、M2/M3 交付物。保留既有 dirty 工作区及模块重组，不清理、重置或提交。

## 已执行

- `git status --short`：退出 0；工作区已有大量修改、删除及未跟踪文件，包含原型模块重组和 M0 计划，均予保留。
- `cargo build --manifest-path prototype/Cargo.toml`：退出 0；使用现有 target 缓存，不能证明隔离构建或实际 DuckDB 版本。
- `cargo test --manifest-path prototype/Cargo.toml`：退出 0；现有测试通过，使用现有 target，不能证明 M1 行为。
- `uname -a`：退出 0；Darwin 27.0.0、arm64。
- `rustc --version`：退出 0；rustc 1.98.1 (48a229cea 2026-09-01)。
- `cargo --version`：退出 0；cargo 1.98.1 (797e8a9bc 2026-08-05)。

## 未完成

未实现 `dev_server`、固定 fixture、HTTP、安全边界及异步接入；未修改代码。未执行隔离 target 构建、实际 DuckDB `SELECT version()`、服务启动、HTTP 验收或最终 diff 检查；DuckDB 实际版本未知。M1 未达验收，不能作为 M2 启动依据；需继续由实现 owner 完成计划并实测。

## 重试工作记录（进行中，端口阻塞）

任务 `task_cf842168c554`，dispatch `ctx_5f4f0d72e3bd`。保留上次失败记录；本次仅新增 `prototype/src/bin/dev_server.rs` 启动骨架，检查 loopback、开发令牌、空数据目录及令牌有效期，并在未就绪时 fail closed；固定目录、完整认证范围、遥测、异步队列、latest 和历史尚未实现，不能作为 M2 服务。禁止修改 plan、status、其他文档、M2/M3 文件；运行产物仅用 `/tmp` 临时目录。

- `git status --short`：退出 0，原有 dirty 模块重组及其他改动保留。
- `CARGO_TARGET_DIR="$(mktemp -d /tmp/iotfast-m1-target.XXXXXX)" cargo build --manifest-path prototype/Cargo.toml`：退出 0，隔离 target 构建。
- `cargo build --manifest-path prototype/Cargo.toml --bin dev_server`：退出 0，首次骨架编译。
- `cargo test --manifest-path prototype/Cargo.toml`：退出 0；仅既有测试及当前骨架编译，不证明 M1 行为。
- 隔离空目录、随机开发令牌及 `IOTFAST_DEV_BIND=127.0.0.1:18080` 下执行 `cargo run --manifest-path prototype/Cargo.toml --bin dev_server`：退出 1；实际 `SELECT version()` 输出 `v2.0.0`，随后 `Error: Os { code: 48, kind: AddrInUse, message: "Address already in use" }`，未监听、未执行 HTTP 测试。按任务端口冲突停止条件请求协调者裁决。
- OS / Rust：沿用本文件前述实测 Darwin arm64 / rustc 1.98.1 / cargo 1.98.1；本次实际 DuckDB 引擎 `v2.0.0`。

未执行最终构建与 HTTP 功能验收、最终 diff 检查；该骨架不是交付实现，不报告 completed。协调者裁决端口冲突后仍需完成全部固定 fixture、严格协议、隔离授权、异步原子处理、查询及相应测试。

## 端口裁决后的进展及工作区冲突

协调者授权另选空闲 loopback 端口，首次 18080 失败保留。`lsof -nP -iTCP:18081 -sTCP:LISTEN` 退出 1（无监听）；使用新临时目录启动 18081 成功，`SELECT version()` 实际由 `prototype/src/bin/dev_profile/mod.rs` 中 `db.query_row("SELECT version()", [], ...)` 执行并输出 `v2.0.0`；实际运行库 `otool -L prototype/target/debug/dev_server` 显示 `@rpath/libduckdb.dylib`，历史构建输出定位 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`，但未核实该次进程实际解析路径。此前隔离 target build 是添加 tiny_http 之前的构建，不代表当前代码隔离构建。

现已初步加入 HTTP、固定模型/绑定、验证、异步队列、latest 和 raw 查询，尚非验收通过；手工 curl 测到 health 200、无凭据 401、单设备 bad 工程值 202/view 200、raw 200，未完成全矩阵。`cargo build --manifest-path prototype/Cargo.toml --bin dev_server` 和 `cargo test --manifest-path prototype/Cargo.toml` 各退出 0，测试不是新功能覆盖。进程已 Ctrl-C，退出 130。

执行 `cargo fmt --manifest-path prototype/Cargo.toml -- prototype/src/bin/dev_server.rs prototype/src/bin/dev_profile/mod.rs` 退出 0，却对整个 crate 格式化，意外修改既有未跟踪重组文件 `prototype/src/core/contract.rs`、`prototype/src/ingest/{wal,storage,validate}.rs`、`prototype/src/catalog/{asset,binding}.rs`、`prototype/src/query/northbound.rs`，以及原先 status 未列出的五个既有 tests 文件。未对这些文件主动 reset 或覆盖；重组文件原先未跟踪，无法从 Git 安全恢复其用户原始内容，现询问协调者后续处理。当前存在未解决缺陷：tiny_http 不提供接入 body 读取超时配置，尚不满足协议读取时间限制；JSON 深度和独立验收亦待完成。本 attempt 不可称完成或交 M2。

协调者指示保留现场继续，不将事故视为授权整库格式整理。执行以下命令退出 0，将现状（非格式化前原稿）复制到 `/tmp/iotfast-m1-fmt-snapshot.LT0tZW`：

```sh
snapshot=$(mktemp -d /tmp/iotfast-m1-fmt-snapshot.XXXXXX)
for file in prototype/src/core/contract.rs prototype/src/ingest/wal.rs prototype/src/ingest/storage.rs prototype/src/ingest/validate.rs prototype/src/catalog/asset.rs prototype/src/catalog/binding.rs prototype/src/query/northbound.rs prototype/tests/d01_health_and_d00_test.rs prototype/tests/d05_d06_ingest_and_wal_test.rs prototype/tests/d07_history_and_latest_test.rs prototype/tests/d08_northbound_and_subscription_test.rs prototype/tests/review_fixes_test.rs; do mkdir -p "$snapshot/$(dirname "$file")"; cp -p "$file" "$snapshot/$file"; done
```

上述七个源码文件最初为未跟踪，无可验证原始快照，不能证明语义等价。五个测试文件初始 status 干净，其 Git 原版可比对：`git diff --numstat -- <上述五个测试路径>` 退出 0，分别为 +8/-2、+20/-35、+23/-6、+16/-4、+416/-146；`git diff -w --stat -- <上述五个测试路径>` 退出 0，仍显示五文件变化，换行及尾逗号等格式变化不能以 `-w` 排除语义变化，未进行完整语义等价验证。没有执行恢复，后续不再运行全 crate 格式化。

## 恢复实现与自证（2026-11-10）

任务 `task_cf842168c554`，dispatch `ctx_37bc7b2c19a2`。本次在既有服务上补充固定 `source_device_grant` 记录，使用独立的 `[valid_from_ms, valid_to_ms)` 按服务端当前时间逐次查询，拒绝发生在去重之前；缺省请求标识改为进程内唯一生成。新增 `prototype/examples/orca_m1_verify.py`，用随机令牌、空 `/tmp` 目录、空闲 loopback 端口启动真实进程并通过 curl 验证；未修改 plan、status 或独立验收文件。保留上面格式化事故和失败记录。

本次修改路径：`prototype/src/bin/dev_profile/mod.rs`、`prototype/examples/orca_m1_verify.py`、本证据文件；之前 M1 的 `prototype/src/bin/dev_server.rs` 和同目录服务实现仍为交付的一部分。`git status --short` 退出 0，既有 dirty 文件保留；不得认为格式化事故已解决。

环境：`uname -a` 退出 0，Darwin 27.0.0 arm64；`rustc --version` 退出 0，1.98.1 (48a229cea 2026-09-01)；`cargo --version` 退出 0，1.98.1 (797e8a9bc 2026-08-05)。进程启动实际 `SELECT version()` 输出 `v2.0.0`；本次使用现有 target 与动态库缓存，不声称干净构建。

| 命令 | 结果 |
|---|---|
| `cargo build --manifest-path prototype/Cargo.toml` | 退出 0，`Finished dev profile` |
| `cargo test --manifest-path prototype/Cargo.toml --bin dev_server` | 退出 0，2 项通过：grant 边界、受控事务失败回滚及停止准入、模拟 pending 上限 429 |
| `cargo test --manifest-path prototype/Cargo.toml` | 退出 0，全部现有测试及上述 2 项通过 |
| `python3 prototype/examples/orca_m1_verify.py` | 退出 0，输出 `PASS health, auth, scope, atomic rejection, durable, dedup, conflict, device isolation, bad quality, sample time, ordering, raw history`；stderr `DuckDB SELECT version(): v2.0.0`；发送 SIGINT 后进程退出 |
| `git diff --check -- prototype/Cargo.toml prototype/Cargo.lock prototype/src/bin prototype/examples docs/development/evidence/orca-mvp-m1.md` | 退出 0；这些新文件未跟踪，Git 常规 diff 不覆盖其内容 |
| `git diff --no-index --check /dev/null prototype/src/bin/dev_profile/mod.rs` 和同命令检查 `prototype/examples/orca_m1_verify.py` | 各退出 1，因文件与空文件不同；均无空白错误输出。不是测试失败，也不是已跟踪 diff 的覆盖证明 |

可复现：先执行上表 build，然后执行 `python3 prototype/examples/orca_m1_verify.py`。脚本实际启动 `prototype/target/debug/dev_server`，设置三个随机 64 位十六进制令牌、自动选择 `127.0.0.1` 空闲端口、创建空 `/tmp` 数据目录；macOS arm64 使用 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0` 为动态库路径。脚本退出时关闭进程及清理自己的临时目录，不涉及用户或生产数据。手动启动可按 lead 验证 §3 配置同样环境、替换空闲端口和新空目录。

覆盖限度：grant 上下界是针对真实 DuckDB 查询的注入时间单元测试，非等待真实 HTTP 时间到期；token 一小时到期、实际 HTTP 队列满和 10000 消息上限未实测。DB 回滚测试在内存 DuckDB 通过预置重复 receipt 强制事务提交路径报错，验证历史/latest 无残留及新请求 503；非独立进程或 HTTP 层故障注入。未做隔离 target 的当前代码干净构建、真实设备、并发性能、崩溃恢复或生产部署验证。M2/M3 独立验收仍待执行。

## M1-R2：早响应请求体排空修复（2026-11-10）

任务 `task_cf842168c554`，dispatch `ctx_1457e75b01a3`。本轮由应急模型 `auto/kimi-k3.1-pro` 执行，**不是 Terra 验证**；Terra attempt 的 provider 失败由协调者另行保留。仅修改 `prototype/src/bin/dev_profile/mod.rs`，新增 `prototype/examples/orca_m1_early_reject_verify.py`，并追加本证据；未修改 M2 smoke / runbook / evidence、plan、status、wire 或既有断言，未 reset / clean / checkout / commit，未运行整 crate `cargo fmt`。

根因是 `serve()` 在认证后的动作、Prefer、Content-Type 等前置拒绝中，尚未消费请求 body 就写出响应并关闭 socket；客户端仍发送 body 时可能收到 RST，丢失预期 403 / 422 / 503。修复在统一 `respond()` 写响应前调用 `Request::drain_body()`：仅对唯一合法 `Content-Length <=65536` 的请求，在原请求 5 秒 deadline 内有界读取剩余 body；`body_read` 防止正常已读 body 被重复读取。非法、缺失或超限长度不做无界读取。认证、动作、校验顺序、状态码、错误信封及接收副作用语义未改，不记录 body 或 secret。

### 真实回归

新增 `python3 prototype/examples/orca_m1_early_reject_verify.py`，从仓库根执行；使用真实 `prototype/target/debug/dev_server`、自动选择的空闲 `127.0.0.1` 端口、三个随机 64 位十六进制令牌和独立新建空 `/tmp/iotfast-m1-early-reject-*` 目录。脚本通过原始 socket 完整读取状态行、headers 和 Content-Length 指定的错误体，任何连接重置 / 提前关闭都会失败，不将 reset 计为通过；退出时仅回收自身 PID 并清理自己的临时目录。输出记录命令、实际端口、临时目录策略、DuckDB 实际版本和失败详情，脚本最终打印自身 `exit_code`。

最终回归退出 0：同一有效 reader token 发送合法 telemetry body 50 次，50/50 均读到 `403 FORBIDDEN_ACTION` 与统一错误体；另一次先发送 headers、延迟 100 ms 再发送 body，也读到同样 403；随后 gateway token 的延迟 body durable 请求读到 `503 DURABILITY_BACKPRESSURE`。总计上述 52 个真实请求 **0 次 ConnectionReset**，实际启动日志为 `DuckDB SELECT version(): v2.0.0`。

### 命令、退出码与环境

| 命令 | 结果 |
|---|---|
| 首次 `cargo build --manifest-path prototype/Cargo.toml --bin dev_server` | 退出 101；新 `drain_body()` 同一表达式同时可变 / 不可变借用 buffer，精确错误 `E0502` at `src/bin/dev_profile/mod.rs:90:69`。拆出 `read_length` 后修复，未隐藏此失败 |
| 首次构建修复后的 build + 新回归 | build 退出 0；回归退出 1，脚本解析响应头终止空行时 `ValueError: not enough values to unpack`，不是服务 reset / 状态断言失败；跳过空头行后重跑 |
| 最终 `cargo build --manifest-path prototype/Cargo.toml --bin dev_server` | 退出 0 |
| 最终 `python3 prototype/examples/orca_m1_early_reject_verify.py` | 退出 0，结果如上 |
| `cargo test --manifest-path prototype/Cargo.toml` | 退出 0，16 项测试通过、0 失败、0 doc-tests |
| `python3 prototype/examples/orca_m1_verify.py` | 退出 0；原有 health / auth / scope / 原子拒绝 / durable / 去重 / 冲突 / 两设备隔离 / bad quality / 采样时间 / 顺序 / raw history 均通过，stderr 实测 `v2.0.0` |
| `git diff --check -- prototype/src/bin/dev_profile/mod.rs prototype/examples/orca_m1_early_reject_verify.py docs/development/evidence/orca-mvp-m1.md` | 退出 0；新未跟踪文件另以 Python 检查末尾换行、尾部空白和脚本语法，退出 0 |

环境：macOS 27.0 (26A428)、Darwin arm64；rustc `1.98.1 (48a229cea 2026-09-01)`；cargo `1.98.1 (797e8a9bc 2026-08-05)`；Python `3.14.7`；真实进程 `SELECT version()` 为 DuckDB `v2.0.0`。本轮沿用现有 target 与动态库缓存，不是干净隔离构建。

### 保留边界与后续

M2 最终两次独立 smoke 由 M2 随后重跑，本轮未修改或代跑其脚本。真实令牌 / grant 1 小时到期、HTTP 429 容量、独立进程 DB 故障注入、干净隔离构建仍未验证；真实网关、生产安全、性能、崩溃恢复与可靠 SLA 仍不在本 profile 验证范围内。
