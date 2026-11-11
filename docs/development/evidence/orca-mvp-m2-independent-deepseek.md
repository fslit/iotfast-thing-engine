# Orca MVP M2 独立 HTTP smoke 与契约测试证据

- 日期：2026-11-10（Asia/Shanghai）
- HEAD：`c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`（工作区为既有 dirty 重组，本文件只新增本证据，未改任何源码、Cargo 清单、他人文档或既有证据）
- 角色：M2 独立测试工程，不参与实现；只读方式构建/启动/验证，不改代码
- 结论（先行）：**本次真实运行的 24 个断言组全部 PASS，0 FAIL**；未覆盖项单列于第 7 节，**不得当作通过**。

## 1. 环境

| 项目 | 值 |
|---|---|
| 平台 | macOS 27.0（Build 26A428），Darwin Kernel 27.0.0，arm64（RELEASE_ARM64_T6000） |
| rustc | 1.98.1 (48a229cea 2026-09-01) |
| cargo | 1.98.1 (797e8a9bc 2026-08-05) |
| DuckDB 运行时版本 | `v2.0.0`（进程启动即 `SELECT version()`，见服务 stderr 首行 `DuckDB SELECT version(): v2.0.0`） |
| 动态库 | `@rpath/libduckdb.dylib`；二进制无 `LC_RPATH`，直接执行需 `DYLD_LIBRARY_PATH=$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0`（历史缓存，非系统安装） |
| 服务二进制 | `prototype/target/debug/dev_server` |
| 监听 | `127.0.0.1:18301`（启动前 `lsof` 确认空闲；未触碰被他人占用的 18080） |

## 2. 构建与测试（真实退出码）

| 命令 | 退出码 | 说明 |
|---|---|---|
| `cargo build --manifest-path prototype/Cargo.toml --bin dev_server` | **0** | `Finished dev profile`，复用现有 target 缓存（**不构成干净构建证据**） |
| `cargo test --manifest-path prototype/Cargo.toml` | **0** | 16 个测试全部通过、0 失败，0 doc-tests。分布：lib 0；`dev_server` 单测 2（`grant_has_inclusive_start_exclusive_end_and_matches_source_and_device`、`failed_commit_rolls_back_and_stops_new_admission`）；集成 `d01`–`d08` 各 1；`review_fixes_test` 7。 |

> 注：`cargo test` 的 2 个 `dev_server` 单测覆盖了 grant 上下界（含端点无关性）与“提交失败回滚 + 停止准入”的内部逻辑，但不等于 HTTP 层的容量 / 回滚实测（见第 7 节）。

## 3. 启动（真实实测）

最终证据所用启动命令（令牌值不落盘、不打印；此处仅列来源）：

```sh
export IOTFAST_DEV_DATA_DIR="$(mktemp -d /tmp/iotfast-m2.XXXXXX)"   # 全新空目录，0700
export IOTFAST_DEV_BIND=127.0.0.1:18301
export IOTFAST_DEV_INGEST_TOKEN=$(openssl rand -hex 32)             # 64 位十六进制，代号 G
export IOTFAST_DEV_READER_TOKEN=$(openssl rand -hex 32)             # 64 位十六进制，代号 R
export IOTFAST_DEV_ALL_TOKEN=$(openssl rand -hex 32)                # 64 位十六进制，代号 ALL
DYLD_LIBRARY_PATH="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0" \
  ./prototype/target/debug/dev_server
```

- 令牌来源：三个 `openssl rand -hex 32`，各 64 位十六进制、互不相同；**真实值未写入本文件、日志或 URL**（测试脚本仅在进程环境内使用）。
- 服务 stderr 首行：`DuckDB SELECT version(): v2.0.0`。
- 数据目录内容：`engine.duckdb`（12288 B，`-rw-------` 0600）、`engine.duckdb.wal`（DuckDB 自身 WAL）；**无应用层 `wal/` 目录**——与“仅声明 volatile”一致，无任何 durable 承诺。
- 运维发现（诚实记录）：首次用 `nohup ./dev_server` 启动失败——`dyld: Library not loaded: @rpath/libduckdb.dylib ... no LC_RPATH's found`（进程 abort）。原因是 `nohup`（及 `/usr/bin/env` 等 SIP 受限程序）会剥离 `DYLD_*` 环境变量；改为“导出 `DYLD_LIBRARY_PATH` 后直接作为子进程启动”即正常。这不是服务缺陷，是启动方式差异，已在启动命令中体现。

## 4. 必测矩阵逐项结果（真实请求）

方法：Python `http.client` 发起真实 HTTP 请求 + 原始 socket（传输层）；每次记录请求、预期、实际状态码/关键字段与 PASS/FAIL。所有请求均打到 `127.0.0.1:18301` 上的真实进程。

| ID | 请求 | 预期 | 实际（关键字段） | 结果 |
|---|---|---|---|---|
| 1 | `GET /health`（无认证） | 200，profile=`orca-mvp-dev-v1`，`durability=volatile`，不含秘密 | 200 `{"status":"ok","profile":"orca-mvp-dev-v1","durability":"volatile"}`；响应体不含任何令牌；仅 3 个键，无 path/secret/scope/tenant | **PASS** |
| 2 | 无凭据 / 错令牌 / ingest 令牌读视图 | 401、401、403 `FORBIDDEN_ACTION` | 无凭据→401 `UNAUTHENTICATED`；错误 64hex→401；G 读视图→403 `FORBIDDEN_ACTION` | **PASS** |
| 2b | 两个 `Authorization` 头 | 401 `UNAUTHENTICATED` | 401 `UNAUTHENTICATED` | **PASS** |
| 5-pre | 首次观测前 `GET view demo-a`（ALL） | `no_observation`、value null、quality unknown、reason no_observation、静态 binding | 200；`observation_state=no_observation`、`value=null`、`quality=unknown`、`quality_reason=no_observation`、`binding_revision_id=br-demo-a-1` | **PASS** |
| 3 | 相同 body 连发两次；再同 `message_id` 改 value | 202 `accepted_volatile`+`pending`+ingest_id；200 `duplicate` 同 ingest_id；409 `IDEMPOTENCY_CONFLICT` | 202 `status=accepted_volatile,history_status=pending,ingest_id=ing-1`；重复→200 `duplicate` 同 `ing-1`（history_status=`committed`）；改值→409 `IDEMPOTENCY_CONFLICT` | **PASS** |
| 4 | `Prefer: durability=durable`，随后同 key 正常 | 503 `DURABILITY_BACKPRESSURE` retryable/retry_after；该 key 未被占用→202 | 503，code=`DURABILITY_BACKPRESSURE`，`retryable=true`，`retry_after_ms=1000`；随后同 message_id 无 Prefer→202 `accepted_volatile` | **PASS** |
| 5a | 上报 demo-a=245.6 good，GET 视图 | 202→200，value=245.6（不二次换算），quality good，fresh | 202；视图 200 `value=245.6`、`quality=good`、`freshness=fresh`、`observation_state=observed`、`value_sampled_at_ms`=发送值、`quality_observed_at_ms`=sampled、unit=`{Cel,°C}`、`data_type=float64` | **PASS** |
| 5b | 间隔 1.2s 两次 GET | 采样时间不被 GET 刷新 | 两次 `value_sampled_at_ms` 完全一致；value 仍 245.6 | **PASS** |
| 5c | 上报 demo-a=50.5 quality=bad，GET | 实测值保留、quality 仍 bad | 202；视图 `value=50.5`、`quality=bad` | **PASS** |
| 5d | demo-b 上报 12.3，`source_at_ms≈now-45s`，GET | 唯一/较新样本→stale | 202；视图 `value=12.3`、`quality=good`、`freshness=stale`、`value_sampled_at_ms`=旧采样值 | **PASS** |
| 6 | demo-a=22.2、demo-b=-3.25 写入后分别 GET | 两设备共享同一模型、latest 隔离 | 202/202；demo-a `value=22.2`、demo-b `value=-3.25`；两者 `model_revision_id=mr-demo-1`、`model_version=1.0.0`；互不影响 | **PASS** |
| 7 | reader 读 demo-b；reader 混合两资源历史；ALL 读两资源；reader 单资源历史 | reader demo-b 404；reader 混合整次 404；ALL 两资源 200；reader 单资源 200 | reader→demo-b 404 `RESOURCE_NOT_FOUND`；reader 混合历史→404；reader demo-a→200；ALL demo-a/demo-b→200/200；ALL 混合历史→200；reader 单资源历史→200 | **PASS** |
| 8a | raw 历史 asc/desc | 200、字段齐全、asc/desc 有序 | 200，items=7；item 含 `observation_id/resource_id/device_id/property/model_revision_id/binding_revision_id/source_at_ms/received_at_ms/source_time_origin/value_type/value/unit/quality/source_quality_code/replayed/record_kind`；顶层 `mode=raw,next_cursor=null,partial=false,consistency=strong_snapshot`；asc 非降序、desc 非升序 | **PASS** |
| 8b | `limit=1` 但结果 >1 | 422 `VALIDATION_FAILED`，details 含 `RESULT_LIMIT_EXCEEDED` | 422 `VALIDATION_FAILED`，`details=[{field:"limit",issue:"RESULT_LIMIT_EXCEEDED",...}]`，无 items | **PASS** |
| 8c | `cursor` 非 null / `mode=aggregate` / `aggregation` 非 null | 各 422 `CAPABILITY_UNSUPPORTED` | 三者均 422 `CAPABILITY_UNSUPPORTED` | **PASS** |
| 8d | 跨度 >3600000ms / `limit=0` | 各 422 `VALIDATION_FAILED` | 两者均 422 `VALIDATION_FAILED` | **PASS** |
| 9 | `gateway-demo`+demo-a/b；`gateway-evil`+demo-a | 202、202；403 `SOURCE_NOT_AUTHORIZED` | demo-a 202、demo-b 202；gateway-evil→403 `SOURCE_NOT_AUTHORIZED` | **PASS** |
| 10 | 14 类非法遥测 | 见下 | 全部按预期：binding 不符→409 `REVISION_CONFLICT`；model 不符→409 `REVISION_CONFLICT`；未来+120s→422 `CLOCK_SKEW_EXCEEDED`；未知 epoch→422 `CAPABILITY_UNSUPPORTED`；非 `gateway_sample`（device_clock）→422 `CAPABILITY_UNSUPPORTED`；seq 前导零→422；seq 非数字→422；value>1000→422；value<-100→422；字符串 value→422；>1点/重复属性→422；未知字段（自报 `scope`）→422；伪造 `X-Tenant-Id`→422；null value→422；错误 Content-Type→422（除已注明 409/其他 code 外均为 VALIDATION_FAILED） | **PASS** |
| 11 | 2 个被拒请求前后对比；再用被拒 message_id 发合法请求 | 前后视图 latest 与历史条数不变；被拒 key 未占用→202 | R1/R2 均 422；latest 快照 `(30.0, ts)` 前后一致；历史条数 9→9；同 `message_id=atom-msg-1` 合法 body→202 | **PASS** |
| 12 | `/v1/source-state`、`/v1/ingestions/{id}`、`/shadow`、未知路由、已知路由错方法 | 501、501、501、404、501/501 | 501 `CAPABILITY_UNSUPPORTED`×3；`/v1/unknown/xyz`→404 `RESOURCE_NOT_FOUND`；`GET /v1/telemetry`→501；`POST /v1/devices/demo-a/view`→501 | **PASS** |
| 13 | `X-Request-Id` 合法 / 含空格 / 缺失 | 原样回显；422；服务端生成非空 | 合法 `my-req-123` 原样回显；`bad id with space`→422 `VALIDATION_FAILED`；缺失→服务端生成 `req-1794272410764-72`（非 `req-dev`） | **PASS** |
| 14a | 原始 socket：声明 `Content-Length: 70000` 只发头不发 body | 读 body 前即 422 | 0.001s 内返回 422 `VALIDATION_FAILED`（未读取 body） | **PASS** |
| 14b | 原始 socket：声明 `Content-Length: 100` 不发 body | 读取时限内（约 5s）关闭连接 | 5.002s 收到 422 后连接 EOF（读取期限生效，未耗尽内存） | **PASS** |
| 15 | Python 子进程逐配置启动（带超时） | 非法配置非零退出且不监听；基线可启动 | 见第 5 节 | **PASS** |

**统计：24 个断言组，24 PASS，0 FAIL。**

## 5. 启动闸门（任务 15）逐项（Python 子进程 + 超时实测）

| 配置 | 退出码 | 是否监听 | 结果 |
|---|---|---|---|
| `IOTFAST_DEV_BIND=0.0.0.0:18401` | 1 | 否 | PASS（`invalid loopback bind`） |
| `localhost:18401` | 1 | 否 | PASS（`invalid bind address`） |
| `[::1]:18401` | 1 | 否 | PASS（`invalid bind address`） |
| `192.168.1.5:18401` | 1 | 否 | PASS（`invalid loopback bind`） |
| `127.0.0.1:0` | 1 | 否 | PASS（`invalid loopback bind`） |
| 缺失 ingest 令牌 | 1 | 否 | PASS（`missing development token`） |
| 三个令牌相同 | 1 | 否 | PASS（`tokens must be distinct`） |
| 63 位令牌 | 1 | 否 | PASS（`invalid development token`） |
| 非十六进制（64×`g`） | 1 | 否 | PASS（`invalid development token`） |
| 非空数据目录 | 1 | 否 | PASS（`data directory must be absolute and empty`） |
| 相对数据目录 | 1 | 否 | PASS（`data directory must be absolute and empty`） |
| 基线：loopback + 空目录 + 随机令牌 | 运行中（未退出） | 是（18402 可连） | PASS（stderr `DuckDB SELECT version(): v2.0.0`） |

## 6. 额外抽查（超出必测矩阵，实测）

| 请求 | 实际 | 结论 |
|---|---|---|
| `GET /v1/devices/demo-a/view?tenant=demo-a`（ALL） | 422 `VALIDATION_FAILED` | 查询参数一律拒绝（§3） |
| `POST /v1/history/query?x=1`（ALL） | 422 `VALIDATION_FAILED` | 查询参数一律拒绝 |
| `POST /v1/history/query` 缺 `Content-Type` | 422 `VALIDATION_FAILED` | 必须 `application/json` |
| 未知设备 `demo-c`（`gateway-demo`）上报 | 403 `SOURCE_NOT_AUTHORIZED` | 不按请求动态创建设备/补授权 |
| 两个 `X-Request-Id` 头 | 422 `VALIDATION_FAILED` | 多重追踪头拒绝 |

补充说明（测试过程诚实记录）：首轮脚本运行曾出现 2 个失败（ID 12、13），经定位系**测试脚本自身的解包/参数缺陷**，非服务缺陷；修脚本后以**全新空数据目录 + 全新随机令牌重启服务**，完整重跑得到上述 24/24 的一致结果。第 4/5 节数据均取自该单次干净运行。

## 7. 未覆盖 / 未验证项（不得当作通过）

1. **令牌到期边界（`started_at_ms + 3600000`）**：未实测（需等待约 1 小时）；仅代码路径与 grant 单测覆盖相关时间比较，HTTP 到期拒绝未验证。
2. **容量与限流（10000 receipt / 256 pending → 429）**：未在 HTTP 层触发；未发送足量消息造成 429。
3. **DB 提交失败真实注入 → 503 停止准入**：仅有 `failed_commit_rolls_back_and_stops_new_admission` 单测（构造内部状态）覆盖回滚/停止准入逻辑；未在 HTTP 层真实注入底层失败。
4. **来源授权时间窗的到期端点（`t >= valid_to_ms`）**：实现已含显式 `source_device_grant` 表并逐请求检查 `valid_from_ms <= t < valid_to_ms`（已修此前 Lead 指出的“无独立 grant 记录”差距），本次实测了 source/device 允许与拒绝，但未实测 grant 到期时刻拒绝。
5. **同 message_id 并发串行化**：未做并发压力/竞态测试；仅做顺序重复与冲突。
6. **JCS / observation_id 黄金向量交叉核对**：未独立重算 sha256-jcs-v1 与 observation_id 公式；依赖服务内部一致性（重复/冲突行为符合预期）。
7. **超深嵌套（>8）单独隔离**：实现有 `valid_depth` 深度上限 8 与 65536 body 上限；本轮用 `Content-Length:70000` 验证了 body 上限拒绝，但未单独构造纯“深度 >8”样本隔离其错误来源。
8. **真实网关 / TLS / 性能 / 崩溃恢复 / 跨会话去重**：均明确范围外，未验证。
9. **传输层 case 14b**：观察到“约 5s 返回 422 后关闭连接”而非静默 idle 关闭；契约仅要求“耗尽内存前限制长度/读取时间”，故记为 PASS，但非统一 JSON 的优雅拒绝（正常路径错误体是统一 JSON）。
10. 本次为单机、合成数据、loopback 明文、单进程验证；不含生产认证、部署与隔离保证。

## 8. 清理

- 结束时终止 M2 启动的 `dev_server` 进程与后台作业，删除临时数据目录；最终自查 `pgrep -f 'target/debug/dev_server'` 无残留（见收尾记录）。未触碰 18080 上的他人进程。

---

**最终结论**：本次独立、真实 HTTP 实测覆盖任务 1–15 的全部必测矩阵共 24 个断言组，**全部 PASS、0 FAIL**；服务在构建（退出 0）、cargo test（退出 0）、启动闸门、认证/错误码、工程值不二次换算、latest 隔离、raw 历史、来源授权、拒绝原子性、路由与传输时限等方面符合 `orca-mvp-dev-v1` 冻结契约的主要条款。上述第 7 节未覆盖项不得计为通过；实现与契约未发现冲突，故未改动任何代码。
