# Lead 独立验证：本地只读 MVP 实际运行结果

日期：2026-11-10 08:24（Asia/Shanghai）；HEAD `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`；工作区含既有未提交重组，本文件只记录验证，未修改实现源码、Cargo 清单或既有证据。

## 1. 为什么由 Lead 验证

- M1 唯一实现 owner（`task_cf842168c554` / dispatch `ctx_5f4f0d72e3bd`）自 2026-11-10T00:25Z 起无新输出；转录显示其模型服务连续 5 次重连后 `dial tcp: lookup www.llmname.com: no such host`，Orca 仍显示 `live/working` 但 `lastActivity` 已 5 小时。协调 session 同一时点 `state=done`。
- 因此 M1 未提交 `worker_done`、M2/M3 未解锁，但这不证明实现不可运行。Lead 用只读方式复跑构建、启动与接口矩阵，取得独立证据，并明确标出未覆盖项。
- 本文件不是 worker 自述，也不改动 `orca-mvp-status.md`（协调者 owner）。所有结论均可由下列命令重放。

## 2. 环境与构建

| 项目 | 结果 |
|---|---|
| 平台 | macOS 27.0 (26A428)，Darwin arm64 |
| rustc / cargo | 1.98.1 (48a229cea 2026-09-01) / 1.98.1 (797e8a9bc 2026-08-05) |
| `cargo build --manifest-path prototype/Cargo.toml --bin dev_server` | 退出 0（复用现有 target 缓存，不构成干净构建证据） |
| 运行时 DuckDB 版本 | 进程启动即执行 `SELECT version()`，输出 `v2.0.0`（见各实例 stderr 首行） |
| 动态库加载 | 二进制 install name `@rpath/libduckdb.dylib`；直接执行报 `Library not loaded`，需 `DYLD_LIBRARY_PATH=prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0`。该目录是历史缓存，不是系统安装 |
| 进程实际解析的库路径 | **未核实**；仅观察到注入该目录后可加载并运行 |

说明：`cargo run` 会由构建脚本提供搜索路径，直接执行二进制时必须显式注入；这不影响功能结论，但影响“可重复启动”的交付要求，需在运行说明中写明。

## 3. 启动命令与隔离环境

```sh
D=$(mktemp -d /tmp/iotfast-lead-smoke.XXXXXX)
export IOTFAST_DEV_DATA_DIR="$D"
export IOTFAST_DEV_BIND=127.0.0.1:18181
export IOTFAST_DEV_INGEST_TOKEN=$(openssl rand -hex 32)
export IOTFAST_DEV_READER_TOKEN=$(openssl rand -hex 32)
export IOTFAST_DEV_ALL_TOKEN=$(openssl rand -hex 32)
DYLD_LIBRARY_PATH="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0" \
  ./prototype/target/debug/dev_server
```

- 首次绑定 18080 失败（`AddrInUse`，协调者已裁决改用其他 loopback 端口），本次改用 18181/18182，未终止占用 18080 的进程。
- 数据目录内实际生成 `engine.duckdb`（12288 B，权限 0600）与 `engine.duckdb.wal`（DuckDB 自身 WAL）；**没有**应用层 `wal/` 目录，与“仅声明 volatile”一致。
- 目录权限被设为 0700；非空目录、相对路径、不存在路径均拒绝启动。

## 4. 已验证行为（全部为本次真实执行）

### 4.1 启动闸门（退出码均为 1，不监听）

| 输入 | 结果 |
|---|---|
| `IOTFAST_DEV_BIND=0.0.0.0:18199` | `invalid loopback bind` |
| `localhost:18199` / `[::1]:18199` | `invalid bind address` |
| `192.168.1.5:18199` | `invalid loopback bind` |
| `127.0.0.1:0` | `invalid loopback bind` |
| 缺失任一 token、63 位、非十六进制 | `invalid development token` |
| 三个 token 相同 | `tokens must be distinct` |
| 数据目录非空 / 相对 / 不存在 | `data directory must be absolute and empty` |
| 基线 `127.0.0.1:18199` + 空目录 + 随机 token | 正常启动并监听 |

### 4.2 端点与错误契约（合计 48 项断言通过，0 失败）

- `GET /health` → 200 `{"status":"ok","profile":"orca-mvp-dev-v1","durability":"volatile"}`，不含 token、租户或 Scope。
- 无凭据 / 错误 token → 401 `UNAUTHENTICATED`；ingest token 读视图 → 403 `FORBIDDEN_ACTION`。
- `POST /v1/telemetry` → 202 `accepted_volatile` + `history_status=pending` + 唯一 `ingest_id`；同内容重复 → 200 `duplicate` 且沿用原 `ingest_id`，`history_status` 随后为 `committed`；同 `message_id` 改内容 → 409 `IDEMPOTENCY_CONFLICT`。
- `Prefer: durability=durable` → 503 `DURABILITY_BACKPRESSURE`、`retryable=true`、`retry_after_ms=1000`，且该 `message_id` 未被占用（随后同 key 合法请求 202）。不静默降级。
- `GET /v1/devices/{id}/view`：`demo-a` 上报 20.5 返回 20.5（未二次换算）；`bad` 质量实测值原样携带且质量保持 `bad`；`freshness` 取最新样本计算；GET 不刷新采样时间；未观测设备返回 `observation_state=no_observation`、`value=null`、`quality=unknown`、`quality_reason=no_observation`、`binding_revision_id` 仍为静态绑定。
- 两设备共享同一模型且 latest 隔离：`demo-a=20.5`、`demo-b=-3.25` 同时成立。
- `POST /v1/history/query`：raw 模式返回 `items`，字段含 `observation_id/resource_id/device_id/property/value/quality/source_at_ms/received_at_ms/replayed/record_kind/binding_revision_id`；`asc`/`desc` 有序；`limit` 不足以容纳结果时 → 422 `VALIDATION_FAILED` 且 `details[].issue=RESULT_LIMIT_EXCEEDED`（须缩窗，不翻页）；`cursor` 非 null、`mode=aggregate` → 422 `CAPABILITY_UNSUPPORTED`；跨度 > 3600000 ms、`limit=0` → 422。
- 授权范围：reader 读 `demo-b` → 404 `RESOURCE_NOT_FOUND`；reader 混合查询两资源 → 整次 404；ALL token 可读/可查两资源。
- 拒绝路径：未知 `source_id` → 403 `SOURCE_NOT_AUTHORIZED`；绑定 revision 不符 → 409 `REVISION_CONFLICT`；未来 120 s → 422 `CLOCK_SKEW_EXCEEDED`；未知 epoch / 非 `gateway_sample` 来源 → 422 `CAPABILITY_UNSUPPORTED`；`source_seq` 带前导零或非数字 → 422；值超模型范围、字符串值、两点、重复属性、未知字段（含客户端自报 `scope`）→ 422 `VALIDATION_FAILED`；`X-Tenant-Id` 等伪造头 → 422；错误 `Content-Type` → 422。
- 幂等副作用：上述被拒请求未改变 latest、未写入历史、未占用 `message_id`（重复发送同 key 仍为 422，可继续用合法内容）。
- 路由：`/v1/source-state` 等已知未实现端点 → 501 `CAPABILITY_UNSUPPORTED`；未知路由 → 404；已知路由错误方法 → 501。
- 请求标识：合法 `X-Request-Id` 原样回显；含空格等非法值 → 422；缺失时返回服务端生成的 `request_id`。
- 传输层：声明 `Content-Length: 70000` 时，服务在读取 body 前直接 422；声明 100 字节但不再发送时，约 5 s 内关闭连接（读取时限生效，未耗尽内存）。

## 5. 未验证 / 偏离契约之处（不得当作通过）

1. **来源授权时间窗**：实现以固定绑定 + token 有效窗口（`[started_at_ms, started_at_ms+3600000)`）约束访问；没有 M0 计划 §3 要求的独立 `grant.valid_from_ms/valid_to_ms` 记录与逐请求上下界校验。当前等价性依赖“固定 fixture”，不是契约要求的显式授权的检查。需 M0/M1 决定补齐或正式收敛该条。
2. token 到期（1 小时）后拒绝：仅代码审查（`now() >= started && now() < started + 3_600_000`），未实测到期点。
3. `X-Request-Id` 缺失时返回常量 `req-dev`，非每请求唯一；不泄露信息，但弱于“服务端生成”。
4. 容量与失败路径：10000 receipt / 256 pending 的 429、DB 提交失败回滚与 `503` 停止准入，均未实测。
5. 慢 body 场景以关闭连接结束，未返回统一 JSON 错误体；M0 计划只要求“耗尽内存前限制长度/读取时间”，因此记录为行为说明而非违约。
6. M1 曾执行一次作用范围为整 crate 的 `cargo fmt`，修改了既有未跟踪重组文件与五个已跟踪测试文件；原始内容不可由 Git 恢复。本验证不声称这些文件语义等价，也未据此恢复任何文件。
7. 本次为单机、合成数据、loopback 明文验证；不含真实网关联调、并发/性能、崩溃恢复、生产认证与部署，均属明确范围外。

## 6. 结论

- **最小演示服务可构建、可启动、可完成闭环**：工程值接入 → 易失 ACK → latest 视图 → DuckDB raw 历史，且认证、固定资源映射、错误码、拒绝原子性、两设备隔离与显式 volatile 语义符合 M0 冻结 profile 的主要条款。
- **不等于 M1 完成、也不等于 M3 验收通过**：M1 因模型服务中断未交付自证与收尾；上述第 5 节第 1 条是与冻结契约的实质差距，需实现或由 M0 正式收敛；容量/失败/到期路径未实测。
- 建议下一步（Orca 恢复后）：M1 仅补授权时间窗与收尾自证 → M2 按本文件复跑并扩展到 429/回滚/到期 → M3 独立放行。Lead 本文件可作为 M2/M3 的输入与交叉校验基线，不替代其独立结论。

## 7. 追加注记（Lead，2026-11-10 09:12）——关于 M2 复跑出现“合法请求 503”

- 背景：Orca 原生 M2 复跑 smoke 时，正常合法遥测请求收到 503 `DURABILITY_BACKPRESSURE`，疑似服务进入全局 `failed` 状态。
- 代码事实（只读核对）：`prototype/src/bin/dev_profile/mod.rs` 的 `commit()` 在任一 DuckDB 事务失败时置 `state.failed = true`；此后 `ingest()` 对**所有新消息**返回 503。M0 计划 §5 第 6 条明确要求“DB 提交失败须回滚整个事务，登记 failed 并停止新消息准入（503）”。因此“一旦 DB 提交失败就持续 503”是**契约要求的行为**，不是缺陷。
- 推论：若 M2 的 smoke 在同一个服务实例里先做了“强制 DB 提交失败 / 重复 receipt 冲突”等故障注入断言，再执行普通遥测断言，则后续 503 是测试顺序造成的，**不是服务违约**。正确做法：把“DB 失败 → 回滚 → 停止准入”作为**最后一个**断言，且使用**独立实例**；普通功能断言应在干净实例上执行。
- 另一条相关行为：服务对需拒绝的请求（如 durable、超大/慢 body）在写出响应后可能直接关闭连接、不读完请求体，部分客户端会看到 `ConnectionResetError`。计划只要求“拒绝发生在任何接收副作用之前”和“限制长度/读取时间”，未要求读尽 body，因此归为传输层行为说明；客户端应以“读到 503 响应”为准，或复用连接时容忍关闭。
- 结论不变：这两点不构成实现与冻结契约的冲突；M2/M3 应据此区分“真实缺陷”与“测试顺序/客户端传输假象”，不得把 503 直接判为实现缺陷，也不得用它掩盖真实断言失败。
