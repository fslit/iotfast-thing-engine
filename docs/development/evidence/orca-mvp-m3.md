# M3 独立安全与正确性验收

## 本次独立复验 `ctx_998e7a0a6d5a`

**FAIL，不得 completed。** 本轮独立探针前置断言退出 1，尚未获取响应正文，不能判定是探针输入错误还是服务缺陷；未经核验的后续安全断言不得标 PASS。保留以下历史报告，不能用其结果替代本轮探针。

本轮只读既有 dirty 工作区、M0 plan、M1/M2 证据、runbook、入口文档及 `prototype/src/bin/dev_profile/mod.rs`；codebase-memory 图工具不可用，以 Semble 和定点读取回退。环境为 Darwin 27.0.0 arm64，`rustc 1.98.1 (48a229cea 2026-09-01)`、`cargo 1.98.1 (797e8a9bc 2026-08-05)`；smoke 实测服务及独立查询 DuckDB `v2.0.0`，复用现有构建缓存，非干净构建证明。

| 命令 | 退出码 | 结果 |
|---|---:|---|
| `git status --short` | 0 | 既有大量 tracked/untracked dirty；未恢复或清理 |
| `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server && python3 scripts/orca-mvp-smoke.py && python3 scripts/orca-mvp-smoke.py` | 0 | 构建及连续两轮各 106 项通过，证据 `/tmp/iotfast-m2-v4j3dvj3/results.json`、`/tmp/iotfast-m2-3nhsqd13/results.json` |
| `python3 /tmp/iotfast-m3-independent-ctx998e.py` | 1 | 真实服务启动后，历史空结果前置断言失败；脚本保留于该路径，输出未记录具体响应，服务经 SIGINT 停止 |

探针以随机凭据和独立临时目录启动真实服务；对 `/v1/history/query` 使用两资源、raw、前后各 60 秒时间窗、四种质量、limit 100，预期 HTTP 200 且 items 为空，实际只知该复合断言为假，HTTP 状态及正文未记录。失败发生在凭据、伪造 Scope/tenant、跨资源和无副作用检查之前；这些独立补充检查均**未执行**。需要复验时先输出脱敏状态及正文并确认请求契约，若证实服务缺陷须交 M1 唯一实现 owner 修复；不能由验收方修改服务或放宽门禁。

| 验收项 | 本轮结果 | 边界 |
|---|---|---|
| 实际非 loopback 拒绝 | PASS | 两轮 smoke |
| 真实凭据、客户端自报范围、逐资源拒绝、错误不泄露 | 未执行 | smoke 有覆盖，但独立补充探针未到达，不能据此完成必验 |
| 两设备隔离、工程值、质量/时间/版本/乱序 | PASS | smoke 有限样本 |
| 整批拒绝无副作用 | PASS | smoke 有限样本；独立拒绝副作用检查未执行 |
| HTTP latest/raw 与真实 DuckDB | PASS | smoke 两轮，独立只读各 9 行 |
| volatile ACK、durable 拒绝 | PASS | smoke 有限样本，未验证 durable duplicate |
| WAL 可靠性 | 不适用 | 不承诺持久接收；故障恢复未测 |
| 启动与 smoke 重复 | PASS | 当前机器缓存，非干净环境 |
| WS、动态配置、生产认证、恢复、UI、控制、外部 TSDB | 不适用 | 未作为已实现能力验收；不关闭生产 Q01–Q12 |

未验证真实到期、并发、生产安全、真实网关、故障恢复、容量及性能；本轮总结果 FAIL。

## M3-R1 协调复验（`ctx_998e7a0a6d5a`）

结论：**限定开发 profile 复验 PASS**；历史 FAIL 与非零记录保留，不代表完整 P0 或生产验收。先前历史请求缺 `aggregation`、`cursor`，本次重现 422 `VALIDATION_FAILED`，补齐两个 null 后 200 且 items 为空，确认是探针输入错误而非已证实服务缺陷。先前 TypeError 和空白正则错误同样保留在原记录。

| 完整命令 | 退出码 | 实际与证据 |
|---|---:|---|
| `git status --short > /tmp/iotfast-m3-r1-status.txt` | 0 | 保留 dirty 基线 |
| `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server > /tmp/iotfast-m3-r1-build.log 2>&1` | 0 | 当前机器已有 target 缓存，非隔离干净构建 |
| `python3 /tmp/iotfast-m3-independent-ctx998e.py > /tmp/iotfast-m3-r1-probe-final.log 2>&1` | 0 | 完整可复制脚本保存在该绝对路径；真实服务、随机凭据、空目录及空闲 loopback；服务报告 DuckDB `v2.0.0`，数据目录 `/var/folders/j1/zgmfl7jj5vq2r5ft4rg9rkk80000gn/T/m3-independent-814aw5l1/data` |
| `python3 scripts/orca-mvp-smoke.py > /tmp/iotfast-m3-r1-smoke1.log 2>&1` | 0 | 106/106，`/tmp/iotfast-m2-7nncds0_/results.json` |
| `python3 scripts/orca-mvp-smoke.py > /tmp/iotfast-m3-r1-smoke2.log 2>&1` | 0 | 106/106，`/tmp/iotfast-m2-er1ansp0/results.json` |
| `python3 /tmp/iotfast-m3-r1-duckdb.py` 首次 | 1 | 临时脚本拷贝类时误带空的 `def main():`，发生 IndentationError，未执行数据库查询；保留失败，不归因于服务 |
| `python3 /tmp/iotfast-m3-r1-duckdb.py` 修正后 | 0 | 独立 C API READ_ONLY：`v2.0.0`；observations 恰为 A/`res-demo-a-temperature`/12.5 和 B/`res-demo-b-temperature`/34.5；两条 gateway-demo grant，起点 1794295173030、终点 1794298773030，各 3600000ms |

独立探针实际：错误 bearer 401；reader 自报 `X-Scope`、`X-Tenant-Id`、query scope 各 422；reader 读 B 404、查 A+B 404、完整 body 添加 scope 422、写 telemetry 403；拒绝响应均不含真实凭据或 B 标识。拒绝前后两设备 latest 和 raw 保持不变；随后分别接收 12.5、34.5，latest 显示各自数值且 ACK 为 `accepted_volatile`。上述探针日志 `/tmp/iotfast-m3-r1-probe-final.log`；首次修正后独立探针也退出 0，日志 `/tmp/iotfast-m3-r1-probe.log`。两轮 smoke 的真实 HTTP、DuckDB、质量/采样时间/版本/乱序、整批拒绝和显式 durable 拒绝结果见各日志；非 loopback 实际拒绝并未监听。WAL 不作未经验证的可靠性承诺。

未覆盖真实一小时墙钟到期、HTTP 429、独立进程 DB 故障注入、durable duplicate、并发、干净隔离构建、生产认证与恢复、性能、现场设备；WS、动态配置、UI、控制和外部 TSDB 非本 profile 已实现能力，生产 Q01–Q12 未关闭。图工具不可用，结构审查使用 Semble 和定点源码回退。

空白核验：`perl -ne 'if (/[\t ]+\r?$/) { print "line $. trailing whitespace\n"; $bad=1 } END {exit $bad ? 1 : 0}' docs/development/evidence/orca-mvp-m3.md`、`git diff --check -- docs/development/evidence/orca-mvp-m3.md` 均退出 0；报告未跟踪，另运行 `python3 -c 'from pathlib import Path; p=Path("docs/development/evidence/orca-mvp-m3.md"); d=p.read_bytes(); assert d.endswith(b"\n"); assert all(not line.endswith((b" ",b"\t")) for line in d.splitlines()); print("untracked content whitespace PASS")'` 退出 0，直接检查正文。

任务 `task_6ec4562cd029` / `ctx_7864f88f5acb`，2026-11-10。结论：**FAIL，不能标记 completed**。首轮独立探针自身异常退出 1；即使修正探针后复测通过，任务明确规定任何非零退出均失败。该异常不是已证实的服务缺陷，不能归因于实现。验收未覆盖项亦不能算通过。

## 范围与环境

只读审查 M0 plan、M1/M2 证据、runbook、入口规范、决策台账及 `prototype/src/bin/dev_profile/mod.rs` 中的 HTTP 认证、固定权限、输入、入站和查询代码；运行现有 smoke 及独立临时探针。原工作区已有大量 tracked/untracked 改动，以 `/tmp/iotfast-m3-status.txt` 所保存 `git status --short` 为基线，不恢复、不覆盖。仅新增本报告；构建复用既有 target，临时探针使用全新随机秘密和空目录。codebase-memory 图工具本会话不可用，使用 Semble 搜索与定点源码读取回退，不能声称图索引覆盖。

OS：Darwin 27.0.0 arm64；`rustc 1.98.1 (48a229cea 2026-09-01)`；`cargo 1.98.1 (797e8a9bc 2026-08-05)`；实际服务与独立 DuckDB 查询均为 `v2.0.0`，动态库为 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`。构建未隔离，不能证明干净构建可重复。

## 命令和结果

| 命令 | 退出码 | 实际 |
|---|---:|---|
| `git status --short` | 0 | dirty 基线记录于 `/tmp/iotfast-m3-status.txt` |
| `cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server` | 0 | 构建成功，复用缓存 |
| `python3 scripts/orca-mvp-smoke.py`，第一次 | 0 | 106/106；`/tmp/iotfast-m2-354u8tw9/results.json` |
| `python3 scripts/orca-mvp-smoke.py`，第二次 | 0 | 106/106；`/tmp/iotfast-m2-8idt21fu/results.json` |
| 独立临时 Python HTTP 探针（首轮） | 1 | 七项认证/权限及历史无副作用断言通过，随后错误地将 `properties` 字典迭代为对象，触发 `TypeError: string indices must be integers, not 'str'`；原始现场 `/var/folders/j1/zgmfl7jj5vq2r5ft4rg9rkk80000gn/T/iotfast-m3-probe-qvau2j8i`。探针异常而非服务异常；latest 断言未执行 |
| 独立临时 Python HTTP 探针（修正 `view['properties']['temperature']` 后全新实例复跑） | 0 | 七项拒绝、历史及两设备 latest 无副作用均通过；`/var/folders/j1/zgmfl7jj5vq2r5ft4rg9rkk80000gn/T/iotfast-m3-recheck-fjtuqjhs` |
| `git diff --check -- docs/development/evidence/orca-mvp-m3.md` | 0 | 新文件未被 Git 跟踪，另行检查正文空白 |
| `perl -ne 'if (/\s+$/ && !/^\s*$/) { print "line $. trailing whitespace\n"; $bad=1 } END {exit $bad ? 1 : 0}' docs/development/evidence/orca-mvp-m3.md` | 1 | 检查命令自身误将每行换行匹配为空白；不是真实尾随空白 |
| `perl -ne 'if (/[\t ]+\r?$/) { print "line $. trailing whitespace\n"; $bad=1 } END {exit $bad ? 1 : 0}' docs/development/evidence/orca-mvp-m3.md` | 0 | 修正正则后正文无尾随空白；此前非零仍保留 |

独立探针通过 `subprocess.Popen` 启动真实二进制，随机选择 loopback 端口、生成三组随机 64 位十六进制 token；使用 Python `http.client` 发请求；结束仅向自身服务进程发送 SIGINT（进程返回 -2，非探针命令失败）。两轮探针代码以 `python3 - <<'PY'` 直接运行于本任务终端，未保存脚本，因此无法提供脚本文件作为完整命令复现；对应输入及结果如下。此证据完整性限制与首轮非零均保留，不以复测掩盖。

| 独立探针输入 | 预期 | 实际 |
|---|---|---|
| 错误 bearer 读 A | 401 UNAUTHENTICATED | 一致 |
| reader 读 B，附 `X-Scope: *` 或 `X-Tenant-Id: other` | 422 VALIDATION_FAILED | 两项一致 |
| reader 读 B；reader 查询 A+B | 各 404 RESOURCE_NOT_FOUND | 一致，错误未含 B 资源名或任一真实 token |
| reader 历史 body 附 scope；reader POST telemetry | 422 VALIDATION_FAILED；403 FORBIDDEN_ACTION | 一致 |
| 拒绝前后 all-reader 查询 A+B raw；all-reader 查看 A、B | 空历史不变；两个 latest 均 no_observation | 修正探针后全通过 |

## 逐项结果

| 检查 | 结果 | 证据与限制 |
|---|---|---|
| 非 loopback 实际拒绝 | PASS | 两轮 smoke 实际启动拒绝且无监听 |
| 凭据、固定 Scope、逐资源拒绝及错误不泄漏 | PASS（限定探测） | 两轮 smoke 和上表独立探针；源代码核对身份与指定资源映射；不声称穷举所有攻击 |
| 同模型两设备隔离、工程值不二次换算 | PASS | 两轮真实 HTTP smoke 检查 A=245.6、B=31.2 |
| 质量、采样时间、版本、乱序 | PASS（限定样本） | 两轮 smoke 检查 bad/stale、时间版本及序列排序 |
| 整批拒绝无副作用 | PASS（限定样本） | 两轮 smoke 检查非法混合点、修正重试和历史/latest；不是并发压力证明 |
| 真实 HTTP latest/raw 和真实 DuckDB | PASS | 两轮独立只读 DuckDB 各验证 9 行、ID 与 hash；服务引擎 v2.0.0 |
| volatile ACK 与 durable 拒绝 | PASS（限定新请求） | 两轮 202 volatile 和 503 durable；durable duplicate 未执行 |
| WAL 可靠性 | 不适用 | 本 profile 不承诺 durable 或联合恢复，未做崩溃恢复 |
| 启动和 smoke 可重复 | PASS（限定当前机器缓存） | 按 runbook 构建后两轮成功；不代表干净环境复现 |
| 未实现路由及边界 | PASS（拒绝行为） | smoke 覆盖 WS、动态配置等 501；生产认证、故障恢复非实现能力；不声称 UI、控制或外部 TSDB 已交付 |
| 生产 Q 台账 | 不适用 | Q01–Q12 生产问题不因此关闭；此为受限开发 profile |
| 总体验收 | FAIL | 首轮独立探针非零；另见未覆盖项与证据完整性限制 |

## 未覆盖与风险

真实一小时 token/grant 到期、HTTP 容量上限、DB 故障注入、durable duplicate、并发去重、断电/崩溃恢复、真实网关、生产安全、性能与现场语义均未执行，不计 PASS。当前进程共享本机明文 loopback，不能当成生产安全边界。原有未跟踪源码及历史格式化改动无法由本次验证证明原有语义未变。测试证据结果文件由 smoke 写入，带其原作者任务/模型标识，不代表本 M3 探针作者身份。
