# Orca 最小只读 MVP 协调状态

## 当前事实

- Run：`run_02b4df14640f`。
- 协调 owner：恢复会话 `codex_7d5b524d_96f1_4c04_aac0_e42afb9caa15`（2026-11-10 接管）。已执行 `run-use --from codex_7d5b524d_96f1_4c04_aac0_e42afb9caa15 --id run_02b4df14640f --json`，退出 0；沿用原 Run。此前恢复会话为 `codex_b2469cb7_b4e9_4432_80b0_7c5840e9f2c8`，原协调 session `codex_9322d54c_fcd7_41a9_8f02_7e0a07ed5957` 已 done。
- 当前 checkout：`/Users/fengsl/code/work/iotfast-thing-engine`；全部 worker 使用 `--worktree current`，不得新建丢失 dirty 文档的 checkout。
- 用户新授权仅针对独立开发演示 profile；原文档“本轮只交付规范”是历史范围，不据此阻塞本次授权，也不把 D00–D10 或 P0 宣称完成。
- 本文件由协调 owner 独占；`orca-mvp-plan.md` 由 M0 文档 owner 独占。限定开发 profile 的最终复验通过，范围与未覆盖项见下文；不代表完整 P0 或生产验收。
- 模型策略（用户新增授权）：本项目后续所有新 worker 显式使用 `--agent codex --model gpt-6.1-terra`，不改全局账号设置。M0 保持原启动时默认模型（当时未传 `--model`，receipt 未提供实际模型，准确型号未知），不中断/重启换模型。

## Dirty 基线保护

开始时 `git status --short` 退出码 0：已有多份修改文档、删除的旧平铺 Rust 模块，以及未跟踪 PRODUCT、实现架构、AI 任务包、决策台账、体验文档和 `prototype/src/{catalog,core,ingest,query}/` 重组目录。以首次实际 status 输出为完整基线，不将已有删除视作待清理垃圾。严禁 reset、clean、自动提交、还原或覆盖既有重组和文档。

## 任务 DAG

`M0 → M1 → M2 → M3`；仅 ready 任务可启动；失败修复退回唯一实现 owner，不让审查者写实现。

| 卡片 | Task ID | Dispatch ID | Model | Owner / 允许范围 | 依赖 | 状态 | 下步 / 阻塞 |
|---|---|---|---|---|---|---|---|
| M0 范围与契约 | `task_7c6ec9fcc2e6` | 首轮 `ctx_a56029eee354`；复验 `ctx_d0d6218e3587` | 首轮原默认未知；复验 `gpt-6.1-terra` | M0 文档owner（plan/M0证据），当前已完成 | 无 | completed；首轮dispatch failed保留 | 仅静态决议放行，不计功能PASS |
| M1 可启动服务 | `task_cf842168c554` | `ctx_54c1ec8d5434`；`ctx_5f4f0d72e3bd`；`ctx_37bc7b2c19a2`；Terra `ctx_d567bcab28fc`（404）；完成 `ctx_1457e75b01a3` | 完成 attempt 为 `auto/kimi-k3.1-pro` 应急替代（Terra magpie 404） | M1 唯一实现owner；旧失败 attempt 保留 | M0 completed | completed；早响应 reset 修复已独立复验 | 真实 1h 到期、HTTP 429、独立进程 DB 故障注入、干净隔离构建仍未验证；M3 必须列为未覆盖 |
| M2 smoke 与说明 | `task_0a73f16941fc` | `ctx_977e5473a009`（404）；`ctx_e458074f012e`（503）；`ctx_b616386522e7`（failed）；完成 `ctx_04c09ae29684` | 完成 attempt 为 `auto/kimi-k3.1-pro` 应急替代（非 Terra 验证） | M2 独立测试owner；只写 smoke/runbook/M2 证据 | M1 | completed；两次 106/106 PASS | 证据见 `evidence/orca-mvp-m2.md`；真实到期、429 容量、独立 DB 注入、干净构建仍未覆盖 |
| M3 独立验收 | `task_6ec4562cd029` | 首轮 `ctx_7864f88f5acb`（failed）；当前纠正 `ctx_998e7a0a6d5a` | `auto/gpt-6.1-terra`（Terra provider 路由） | `M3 独立验收owner`；只追加 M3 证据，不覆盖既有报告 | M2 completed | completed；首轮 failed 和复验中非零记录保留 | M3-R1 修正历史请求必填字段后完成独立探针、build、两轮 smoke、DuckDB 和空白核验；限定开发 profile PASS，未覆盖项见证据 |
| S1 认证/Scope 安全审查 | `task_88550cb44b7d` | `ctx_6193737036b7` | `gpt-6.1-terra`（receipt确认） | `S1 Terra安全审查`；仅 `docs/development/evidence/mvp-s1-security-review.md` | 无，与 M0 并行 | failed，保留首轮结果 | 有安全发现但验证命令失败；转 S1-R1，不放行产品安全 |
| S2 端到端测试设计 | `task_3522f6b151bc` | `ctx_c49dbbc3a5f9` | `gpt-6.1-terra`（receipt确认） | `S2 Terra测试设计`；仅 `docs/development/evidence/mvp-s2-test-design.md` | 无，与 M0/S1/S3 并行 | failed，保留首轮结果 | 有设计与 JSON 后续通过证据但曾命令失败；转 S2-R1 |
| S3 构建环境核验 | `task_079ea15f53bf` | `ctx_aad322e9d75a` | `gpt-6.1-terra`（receipt确认） | `S3 Terra构建审查`；仅 `docs/development/evidence/mvp-s3-build-review.md` | 无，与 M0/S1/S2 并行 | failed（协调复核纠正） | worker 自报成功但命令表有非零/掩盖错误；转 S3-R1 |
| S1-R1 审查验证纠正 | `task_6092cc544634` | `ctx_112666f9b512` | 同一 Terra worker 延续 | 原 S1 owner / 同一报告，追加复验证据 | S1 failed 后显式 follow-up | completed（仅审查产物） | 已核对 pipefail 检索及正文检查0；产品安全未通过 |
| S2-R1 设计验证纠正 | `task_45ba3d310396` | `ctx_5d1dcaf7332c` | 同一 Terra worker 延续 | 原 S2 owner / 同一报告，追加复验证据 | S2 failed 后显式 follow-up | completed（仅设计产物） | 已核对三JSON与正文检查0；字段契约待M0、实际smoke待M2 |
| S3-R1 环境证据纠正 | `task_f1c51d6d490a` | `ctx_0fb471b42c89` | 同一 Terra worker 延续 | 原 S3 owner / 同一报告，追加复验证据 | S3 failed 后显式 follow-up | completed（仅静态核验产物） | 已核对独立命令及正文检查均0；实际构建/链接/引擎版本仍未验证 |

S1–S3 为无写冲突的只读审查/设计支线，不改 M0→M1→M2→M3 硬依赖；已有报告在对应下游启动时传入，尚未生成的报告不成为等待生产冻结的无限门禁。报告不是实际端到端或最终安全验收。

## 目标与验证纪律

loopback HTTP、模拟网关工程值、同模型两设备隔离、latest、raw DuckDB 历史、真实核验开发服务凭据并映射服务端固定可信资源范围、拒绝自报 Scope、明确错误/质量/采样时间。仅显式 volatile；不承诺 WAL 联合故障可靠性，不返回未验证 durable。不做 UI、控制、外部 TSDB、多租户平台、生产部署；WS 与动态配置 API 等缩减项必须明示未实现。

每次 worker 返回须记录命令、退出码、环境、未通过/未执行项；非零退出按失败，不以文档通过替代功能通过。事件唤醒推进，不运行轮询脚本，不消费外部 Lead 邮件，不因沉默重启。

## 实际验证记录

1. 技能读取、入口/基线读取：命令退出码 0；尚未执行服务构建或 smoke。
2. `orca orchestration run-create --objective … --json`：退出码 0，返回上述 Run ID。
3. 四次 `orca orchestration task-create --run run_02b4df14640f --spec-file … --deps … --json`：均退出码 0，建立上述串行 DAG；每卡完整 spec 均保存于 Orca task，原始输入备份在 `/tmp/iotfast-orca-mvp-run_02b4df14640f/M{0,1,2,3}.md`，含输入、允许路径、禁止范围、验收命令、失败/停止条件。
4. `orca orchestration task-list --run run_02b4df14640f --ready --brief --json`：退出码 0，仅 M0 ready。
5. `orca orchestration worker-start --run run_02b4df14640f --task task_7c6ec9fcc2e6 --worktree current --agent codex --display-name 'M0 文档契约owner' --json`：退出码 0；mode `structured`，worker handle `structworker_453721be-e3aa-46e1-a2bd-3e96e5c09eb5`，workspace 与当前 checkout 相同。receipt 要求结束本轮，等待 Orca 事件唤醒。尚无 worker 完成/功能成功证据。
6. 新增授权事件后，一次 `task-list`、一次 `worker-show ctx_a56029eee354`：退出码均 0；M0 dispatched 且 live/working，未为换模型中断。
7. 三次 S1–S3 `task-create` 和三次 `worker-start --worktree current --agent codex --model gpt-6.1-terra`：退出码全部 0；三个 receipt 均确认 Terra、structured、相同当前 checkout；Task/Dispatch 如表。每卡完整 spec 保存于 Orca 和 `/tmp/iotfast-orca-mvp-run_02b4df14640f/S{1,2,3}.md`。
8. Superagent 入口核实：本机 Orca 1.4.255 的 `orca --help` 退出码 0，公开帮助列出 orchestration Run/task/worker/gate，没有找到名为 Superagent 的独立命令。此结果仅说明该 CLI 帮助未暴露入口，不证明 GUI 或其他版本不存在；不猜造 `superagent` 命令。当前正式可用编排入口为已实际成功的 `orchestration`。
9. S1/S2 `worker_done` 均明确 outcome `failed`；本次 `task-list --run run_02b4df14640f --brief --json` 退出码 0，确认两原卡 failed，M0/S3 dispatched，M1–M3 pending。报告文件已读取，不把报告存在当任务完成。S1 未执行 HTTP/cargo；S2 未执行服务/cargo。S1 不存在路径搜索被管道掩盖，S2 曾 Python SyntaxError 退出 1、no-index 差异退出 1；均保留失败记录。`git diff --check` 不覆盖其未跟踪报告，后续要求显式正文校验。
10. 已向 M0 发送三项安全风险与 S2 设计待决范围，退出码 0；两次 `orchestration send --to dispatch:<原S1/S2> …` 退出码 0，receipt 明确 `delivery:follow_up`，生成上表 S1-R1/S2-R1 Task/Dispatch，复用原 owner、同一 checkout，不是沉默重启。只授权纠正证据，不转交实现权限。
11. S1-R1/S2-R1 `worker_done` outcome `succeeded`，对应报告追加段已读取核对：S1 确认实际入站路径后 pipefail 检索0及正文检查0；S2三个 JSON、正文检查及 status/diff 均0，明确普通 diff 不覆盖未跟踪正文。原 S1/S2 failed 保留。任务查询确认两复验 completed，ready 查询为空；不启动未满足 M0 依赖的 M1。
12. S3 `worker_done` 自报 succeeded；报告记录 macOS27.0 arm64、Rust/Cargo1.98.1、DuckDB crate锁1.20000.1、历史 v2.0.0 库缓存、运行引擎版本未知。但命令表同时有不存在路径的退出1/2和分号末项掩盖错误；不能接受其“全部验收0”的概括。协调执行 `task-update --id task_079ea15f53bf --status failed --result … --json` 退出0，纠正为failed，非静默抹去失败。
13. `send --to dispatch:ctx_aad322e9d75a` 退出0，生成 S3-R1 `task_f1c51d6d490a` / `ctx_0fb471b42c89`，同owner独占原报告追加复验；M0 已收到支线进展与实际构建版本仍未知的提醒。未运行服务/cargo，S3原报告的构建建议未经实际验证。
14. S3-R1 `worker_done` outcome `succeeded`；追加复验段已核对，manifest/lock、实际 build/link 脚本、历史库/输出的独立存在性检查、TOML 读取、otool/cat 及正文格式检查均记退出0。原 S3 failed 保留；只接受静态环境产物，不计构建/链接/运行时引擎版本通过。本事件 ready 查询退出0、列表为空，继续等待 M0，无主链任务可提前启动。
15. M0 初轮 `worker_done` outcome `failed`：plan 与 M0 evidence 已落盘，初次字段标记缺 `IDEMPOTENCY_CONFLICT` 的 Python 退出1、no-index差异检查1，修正后的静态命令0仍不能抹去首轮失败。两份文档已读；`worker-show ctx_a56029eee354` 退出0确认 `state:failed/stage:settled/activity:idle`，不是因沉默恢复。
16. 加载官方 recovery-and-cleanup 指南后，以 `worker-start --task task_7c6ec9fcc2e6 --retry-of ctx_a56029eee354 --worktree current --agent codex --model gpt-6.1-terra` 显式重试同Task，退出0，receipt确认 `ctx_d0d6218e3587`、Terra、structured、当前checkout。旧dispatch失败保留，由新attempt接管M0文档owner，不增加一张无法满足原M1依赖的替代主卡。已send明确仅复验既有文档、不重写计划、不扩大生产前置、保留旧失败。
17. M0-R1 `worker_done` succeeded；已读取M0证据§6，独立运行§3.1的status、diff及Python链接/JSON/正文/字段矩阵检查均0，环境macOS27 arm64、Python3.14.7、git2.54.0，未改plan、保留首轮失败。只接受静态M0决议；`task-list --ready`退出0，仅M1 ready。`git status --short`退出0，dirty重组仍在，新增并行 `orca-terra-coordination-update.md` 未由本协调者写入或覆盖。
18. `worker-start --run run_02b4df14640f --task task_cf842168c554 --worktree current --agent codex --model gpt-6.1-terra --display-name 'M1 Terra唯一实现owner' --json` 退出0，receipt确认Terra/structured/当前checkout，Dispatch `ctx_54c1ec8d5434`。已send三个支线报告、S1修复路由、隔离target/SELECT version()要求和原契约不可默改限制；退出0。M1尚无构建或功能结果。
19. M1初轮 `worker_done` failed；已读 [M1证据](evidence/orca-mvp-m1.md)，仅在现有target下 `cargo build --manifest-path prototype/Cargo.toml` 和 `cargo test --manifest-path prototype/Cargo.toml` 退出0，未改实现代码，`dev_server`/HTTP/认证/异步DuckDB全未实现；隔离构建与真实引擎版本未验证。`worker-show`退出0确认failed/settled/idle；`worker-read`退出0，末段仅可行性评估后归档，无具体外部阻塞或ask。该结果不满足M1，不放行M2。
20. 基于明确失败而非沉默，显式 `worker-start --task task_cf842168c554 --retry-of ctx_54c1ec8d5434 --worktree current --agent codex --model gpt-6.1-terra` 同卡实现重试退出0，Dispatch `ctx_5f4f0d72e3bd`、structured/当前checkout。已send要求直接实现、在scope内修复与验证，不能以基线或任务较长作为交付/停止理由；真阻塞用ask给精确错误，旧失败保留、不增并写owner。
21. M1-R1 ask `msg_05b9281a8a52`：启动骨架时默认 `127.0.0.1:18080` 已占用，`cargo run --manifest-path prototype/Cargo.toml --bin dev_server` 退出1，`Os code48 AddrInUse`；worker 同时报告 `SELECT version()` 为 `v2.0.0`，完整查询/库路径/隔离构建证据尚待M1报告核对。协调 `reply` 退出0：授权用另一空闲字面量IPv4 loopback端口（如18081），符合既有plan的1–65535配置范围；保留失败，不杀占用进程、不放宽地址、不复用已初始化数据目录。要求继续实际实现，不能因已解释的端口冲突只交骨架。
22. M1-R1 ask `msg_bacf0b92c9c0`：`cargo fmt --manifest-path prototype/Cargo.toml -- prototype/src/bin/dev_server.rs prototype/src/bin/dev_profile/mod.rs` 退出0但实际格式化整个crate，波及既有dirty/未跟踪重组和5个原先干净tests。协调reply退出0：保留当前现场继续实施，不用HEAD/reset/checkout覆盖原稿，停止整crate格式化；要求受影响文件先保存当前副本至/tmp（不是事故前快照）、精确清单与证据，不把“纯格式”自述当已验证事实，无可信基线不声称已恢复/等价。原实现单写owner不变，未授权用户原稿的语义修复。
23. 协调只读 `git status --short`、`git diff --stat -- prototype/tests`、`git diff -w --numstat -- prototype/tests` 均退出0；5 tests为 `d01_health_and_d00_test.rs`、`d05_d06_ingest_and_wal_test.rs`、`d07_history_and_latest_test.rs`、`d08_northbound_and_subscription_test.rs`、`review_fixes_test.rs`，总diff 483增加/193删除。`-w`仍有差异，可能包括换行重排，不据此断言存在/不存在语义改变。未跟踪重组文件无格式前可信快照，完整保留基线未遭修改的承诺目前无法证明。M1证据当前仍称读取超时/JSON深度未完成，已明确不得因依赖局限放宽plan；最终隔离build/HTTP矩阵仍待实现owner证据。
24. 外部推理供应商事件 `msg_chaterr_7c01a2514109da432a579a75`：M1 worker 报 `stream disconnected ... dial tcp: lookup www.llmname.com: no such host`。协调 `worker-show`/`task-list`/`worker-list` 均退出0：Task仍dispatched，`ctx_5f4f0d72e3bd` 为 live、terminal active、activity idle，未settled，不能另起并发worker。实测 `dscacheutil -q host -a name www.llmname.com` 退出0并解析 `192.204.35.234`；`curl -sS -o /dev/null -w '%{http_code}' --max-time 8 http://www.llmname.com/v1/models` 退出0、HTTP 401，说明DNS已恢复而非实现结论。已 `send` 同dispatch（`msg_57c5d1e88358`、`msg_215fcb0f52f4`）并 `reply`（`msg_9e2b984a34a3`），要求保留历史续跑、禁止 fmt、补齐独立grant窗口和失败路径，不以供应商中断报完成。
25. 最新 `git status --short` 退出0，除既有dirty外出现外部Lead报告 `docs/development/evidence/lead-mvp-independent-verification.md`；该文件由外部Lead只读写，协调者未改。报告记录48项接口断言0失败、实际DuckDB `v2.0.0`、两设备隔离/volatile/错误边界等，但明确未核实实际动态库路径、未实测token到期/容量/DB回滚、缺少独立grant上下界；因此只能作为M2/M3输入，不能替代M1自证或M3放行。
26. 本接管会话只读复核：`run-use --from codex_b2469cb7_b4e9_4432_80b0_7c5840e9f2c8 --id run_02b4df14640f --json` 退出0；`task-list` 显示 M1 `task_cf842168c554` completed、M2 `task_0a73f16941fc` failed、M3 `task_6ec4562cd029` pending；`worker-list` 显示 M1 完成 attempt `ctx_37bc7b2c19a2` 为 succeeded，M2 旧 attempt `ctx_977e5473a009`/`ctx_e458074f012e`/`ctx_b616386522e7` 均保留失败，另有 Orca 自动续派 `ctx_ee07e8d9fffb`（task `task_a688916d7f97`，同一 M2 终端）为 ready/in_progress。`worker-read --dispatch ctx_5f4f0d72e3bd` 退出0，确认旧 M1 dispatch 已 stopped/exited，不因沉默另起实现 owner。
27. 本接管会话实际复跑 M1：`cargo test --manifest-path prototype/Cargo.toml` 退出0（lib/集成/review_fixes 全绿，含 dev_server 的 grant 边界与受控回滚单测）；`python3 prototype/examples/orca_m1_verify.py` 退出0，输出 `PASS health, auth, scope, atomic rejection, durable, dedup, conflict, device isolation, bad quality, sample time, ordering, raw history`，stderr `DuckDB SELECT version(): v2.0.0`。该复跑只确认 M1 自证可重放；真实1小时到期、HTTP 429容量、独立进程 DB 故障注入、干净隔离构建仍未覆盖。
28. 本接管会话实际复跑 M2 smoke：`python3 scripts/orca-mvp-smoke.py` 退出1，证据 `/tmp/iotfast-m2-k8ggftsn/results.json`。M01–M09 均 PASS（含 durable 503 后同实例 `m2-stale` 202、两设备工程值与 latest/raw）；唯一失败为 M07 future skew：脚本构造 `int(time.time()*1000)+60001`，请求到达时服务 `now()` 已前进，落入 60s 容差返回 202。该失败是测试样本裕量竞态，不是服务违约；已要求 M2 只修测试裕量并连续两次退出0，禁止放宽为接受 202。此前 `m2-stale` 503 本轮未复现，保留旧失败与传输 reset 记录但不据此追服务缺陷。
29. M2 续跑 worker 在 09:14 已产出两次独立完整 smoke 通过：`/tmp/iotfast-m2-fmyrmflf/results.json` 与 `/tmp/iotfast-m2-82_7plkd/results.json` 均为 102 cases、0 fails，末项 `M11 independent DuckDB IDs` PASS，实际 DuckDB 库为 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`。但 worker 随后追加 JCS/源质量补充断言时先误删 `quality.source_protocol` 导致正确 409，又在另一次运行遇到预期错误请求的 `ConnectionResetError`；前者已改为删除原值为 null 的 `source_code`，后者属于传输层关闭而非服务断言通过。最终 M2 必须用最终脚本再连续两次 exit 0，不能用这两次较早结果替代最终版自证；协调者已发送 `msg_3300bf6b70c1` 要求只改 smoke、保留真实 reset 记录、不得放宽断言。
30. M2 最终脚本在 M03 reader-write 连续两次 reset：证据 `/tmp/iotfast-m2-begnr818/results.json`，有效 reader token + 合法 447 字节 telemetry body，预期 403 `FORBIDDEN_ACTION`，attempt1/2 均 `ConnectionResetError [Errno 54]`，无实际响应。协调者独立最小复现（`/tmp/repro_m03.py`）连续 4 次均收到 403 错误信封，说明不是确定性 403 丢失，而是“服务在读取 body 前写 403 并关闭、客户端仍在写 body”的时序竞态；不能把 4 次成功当作 M2 已通过。已将 M1 `task_cf842168c554` 重新置 ready，并在同一 task 以 Terra `ctx_d567bcab28fc` 重开，范围仅限早拒绝响应的有界 body 排空/优雅关闭与真实 50 次回归，禁止改 wire/断言/服务语义。M2/M3 继续等待。
31. Terra 重开 attempt `ctx_d567bcab28fc` 在读取任务后即以 magpie `404 ... knows no model "gpt-6.1-terra"` 结束，未产生实现或验证；该失败保留，未把 TUI 标签当作模型可用证据。已 `worker-abandon` 并关闭其空终端，随后在同一 `task_cf842168c554` 以 documented emergency model `auto/kimi-k3.1-pro` 启动 `ctx_1457e75b01a3`（terminal `term_9812264a-3e42-4554-8fed-c77d3db90036`），范围与 M1-R2 相同；worker-list 显示 ready/live/in_progress，worker-read 显示正在运行 hooks。M2/M3 仍等待。
32. M1-R2 `ctx_1457e75b01a3` 报告 succeeded：在统一 `respond()` 前对唯一合法 `Content-Length<=65536` 做有界 body 排空，保留原 5s deadline 与认证/动作/校验顺序；新增 `prototype/examples/orca_m1_early_reject_verify.py`。协调者独立复验：`cargo build --bin dev_server` 退出0；bin dev_server 2 项 + d01–d08/review_fixes 共16项退出0；`python3 prototype/examples/orca_m1_verify.py` 退出0；`python3 prototype/examples/orca_m1_early_reject_verify.py` 退出0，50 次 reader 403 + 100ms 慢 body 403 + durable 503 全部读到统一错误体、0 reset，DuckDB `v2.0.0`。M1 task 已 completed；1h 到期、HTTP 429、独立进程 DB 注入、干净隔离构建仍未验证。
33. 外部并发工作区变更：2026-11-10 13:19 出现未跟踪 `prototype/tests/mvp_thing_model_server_test.rs`（另有 `prototype/src/server/`），与 M1 `dev_profile` 无关；协调者全量 `cargo test` 因此退出101，失败点在 `tests/mvp_thing_model_server_test.rs:636` 的 WebSocket snapshot 读取，不在 M1 修改路径。未修改该测试或 `server/`，按范围外处理；M1 采用上述 targeted 16 项和真实脚本验收。协调者随后用当前最终 `scripts/orca-mvp-smoke.py` 单跑得到 `SMOKE PASS assertions=106`、退出0，证据 `/tmp/iotfast-m2-nkah3u6f/results.json`。已把 M2 权威 `task_0a73f16941fc` 以 `--retry-of ctx_b616386522e7` 重新启动为 `ctx_04c09ae29684`（Kimi 应急模型），要求连续两次 exit0 后追加证据。
34. M2 `ctx_04c09ae29684` 报告 succeeded；证据 `evidence/orca-mvp-m2.md` 记录两次真实运行：`/tmp/iotfast-m2-78gc8onl/results.json`、`/tmp/iotfast-m2-mtqprtsb/results.json`，均 106/106 PASS、`reset_traces=0`、独立 DuckDB `v2.0.0`。协调者又独立连续跑两次：`/tmp/iotfast-m2-h9ifbqzu/results.json`、`/tmp/iotfast-m2-3p5md7jt/results.json`，均 106/106 PASS、exit0。M2 task_0a73f16941fc 已 completed；真实 1h 到期、HTTP 429、独立进程 DB 注入、干净隔离构建仍未覆盖，历史失败保留。
35. Magpie `/v1/models` 显示 Terra 位于 `auto/gpt-6.1-terra` 与 `gpt/gpt-6.1-terra`，裸 `gpt-6.1-terra` 不是可路由名；这解释了先前 Terra 404。已用 `auto/gpt-6.1-terra` 创建 Terra 终端 `term_34c1c3a1-5e96-4bdc-b34b-9a57bffe31a5`，在 `task_6ec4562cd029` 启动 M3 `ctx_7864f88f5acb`；要求真实启动、两次 smoke、独立 403/durable/grant/latest/raw 抽查，只追加 M3 证据，不改实现/M1/M2/plan/status/Lead。
36. M3 首轮 `ctx_7864f88f5acb` 报 failed，但证据显示两次真实 smoke 均 106/106 PASS、DuckDB v2.0.0；失败来自验收工具自身：首轮独立 Python 探针把 `properties` 字典当对象迭代，`TypeError` 退出1；随后 `perl` 空白检查正则把行尾换行误判为尾随空白，退出1。修正 `view['properties']['temperature']` 与正则后，探针/检查均退出0；未发现服务缺陷。报告 `docs/development/evidence/orca-mvp-m3.md` 如实保留首次非零与证据完整性限制，未覆盖 09:14 的 `orca-mvp-m3-independent-acceptance.md`。
37. 协调者裁决：上述两项属于 M3 验收工具错误，不永久否定产品，但不得抹去；同一 `task_6ec4562cd029` 以 `--retry-of ctx_7864f88f5acb` 启动 M3-R1 `ctx_998e7a0a6d5a`（Terra `auto/gpt-6.1-terra`，terminal `term_2ab051af-a66c-4016-be63-553ce274c64e`）。要求只修 M3 探针/证据检查并重跑 build、两次 smoke、独立探针、空白/diff 检查、独立 DuckDB grant/行数核对；必需最终命令全部 exit0 才可 succeeded，发现真实服务偏差则 failed 并退回 M1。
38. 被放弃的 follow-up `task_59b589fdea8f` / `ctx_e3b158839d9f` 在关闭前向 `evidence/orca-mvp-m3-independent-acceptance.md` 追加了 Terra 复验段（13:38，文件 21588B）：两次 smoke 106/106、20 次 reader 403 无 reset、2 条 observation、2 条一小时 grant、DuckDB v2.0.0，并保留既有结论；该任务不是权威 M3，已置 failed/superseded。协调者将其仅作旁证，不据此直接放行；权威 M3-R1 `ctx_998e7a0a6d5a` 继续独立复验并写入 `evidence/orca-mvp-m3.md`。
39. 协调者针对 M3-R1 的 `msg_12aed74f1932` 做独立最小复现：在全新真实 `dev_server`（随机 loopback 端口、临时数据目录、随机 token）上，对 `/v1/history/query` 提交两资源、raw、前后各 60 秒、四种质量、limit 100。省略 `aggregation`/`cursor` 时得到 `422 VALIDATION_FAILED`；补 `aggregation:null`、`cursor:null` 后得到 `200`，`items` 为空。冻结契约要求八字段必填，故本轮 M3 失败是探针输入错误，不是已证实服务缺陷；原失败仍保留。已回复 `msg_ed06db5579d5`，要求 M3-R1 打印脱敏状态/正文、改用完整八字段、跑完全部权限与无副作用断言，并重跑 build、两次 smoke、独立 DuckDB 行/grant 核对、空白与 diff 检查；最终命令全部 0 才可 succeeded。`worker-show` 显示 M3-R1 已从等待转为 `working`。
40. M3-R1 随后再次报告 `orca orchestration check` 为 No messages，与既有终端/调度邮箱分裂一致；`inbox --terminal dispatch:ctx_998e7a0a6d5a` 显示 `msg_ed06db5579d5` 及两条旧 dispatch 消息仍未处理。已不依赖 reply/check，改用 `send --to dispatch:ctx_998e7a0a6d5a --type dispatch --task-id task_6ec4562cd029 --dispatch-id ctx_998e7a0a6d5a` 重发权威指令 `msg_aa48109b6329`；`worker-show` 复核 activity 从 done 转为 `working`，未新建 worker，也未改服务。

## 已知风险与修复路由

- S1 源码审查发现（尚非 HTTP 复现）：`prototype/src/query/northbound.rs` 的历史路径未显式校验 Scope 有效期；`prototype/src/ingest/validate.rs` 的来源授权忽略 `valid_from_ms`；`prototype/src/core/scope.rs` 内部上下文可反序列化且有宽范围能力，HTTP 若直接采信会形成提权。
- M0 收敛演示边界；M1 为这些被暴露路径的唯一修复 owner，或提供不可达隔离的可验证说明；M2 必须真实 HTTP 验证凭据、时间边界、固定资源映射、自报 Scope 拒绝及无副作用，M3 独立放行。协调者及侧审不改实现。
- S2 的端点、状态码、分页建议只作测试输入，需按 M0 及权威 wire 对齐，不能反向覆盖契约，不能把未实现分页计为通过。
- S3 的静态环境线索：历史 DuckDB 动态库缓存可作为 M1 构建候选输入，但 crate 版本、目录标签与实际加载引擎版本必须分开；M1 需隔离 target 实际构建、检查链接并查询运行时版本。缺失 CLI/pkg-config 不自行安装，不提前断言构建不可用或已通过。
- **工作区保护事故（尚未证明安全恢复）**：M1-R1整crate格式化波及 `prototype/src/core/contract.rs`、`prototype/src/ingest/{wal,storage,validate}.rs`、`prototype/src/catalog/{asset,binding}.rs`、`prototype/src/query/northbound.rs` 和上列5个tests。禁止猜测原内容并覆盖；暂保留现场不是用户批准无关整理，也不等于已恢复或已验证纯格式。M1保存当前快照并列证据，M3须独立核查可核查的差异；未跟踪文件缺事故前快照仍是最终风险，不能隐藏。
- **外部供应商中断**：M1 worker 曾因 `www.llmname.com` DNS 失败停止当前模型轮次；协调已实测端点恢复并在原Dispatch续跑。该错误不是实现通过/失败证据，worker 仍须提交完整M1自己的命令与环境证据。
- **Lead 报告缺口**：Lead独立执行显示主力闭环可用，但独立 grant 有效期/到期、容量与DB失败回滚等未覆盖；M1须补/明确，M2须复跑，M3须独立放行。不得把 48 项 Lead 断言当最终验收。

## 协调事件

- 恢复协调（2026-11-10）：已由新协调 session 接管原 Run `run_02b4df14640f`。只读核对显示 M1 `task_cf842168c554` 的 `ctx_5f4f0d72e3bd` 仍为 live、terminal active、activity idle，不能仅凭 DNS 中断或沉默认定退出；未创建并发实现 owner，也未新建 Run。已向原 dispatch 发送恢复指令 `msg_ca5ab5f5dd4c`，要求补独立 grant 有效期及逐请求上下界校验、修正冻结契约偏差并提交真实自证，保留 dirty 基线与格式化事故现场。Lead 独立证据报告真实启动及 48 项接口断言通过，但授权时间窗、到期、容量和数据库回滚仍有未覆盖项；此结论不代替 M1 自证及 M2/M3 独立验收。M2、M3 仍 pending，等待 M1 事件唤醒；如原 worker 无法处理续派，须获得明确状态和恢复依据后再决定重派，不轮询。

- M1 受控重派（2026-11-10）：转录确认原 `ctx_5f4f0d72e3bd` 在连续 5 次 `www.llmname.com` DNS 失败后未再处理消息，`worker-show` 仍显示 live/idle 但无新活动；已用 `worker-stop` 收到 `closed_agent_terminal`，未把“沉默”当作已退出。第一次新启动 `ctx_1a209f257b13` 在 `agent_readiness` 失败，错误为 Codex 参数 `--dangerously-bypass-approvals-and-sandbox` 重复；已释放其 reclaimable 终端。只读核对定位根因是交互式 `~/.zshrc` 的 `codex` shell function 自动注入该参数，与 Orca 启动参数叠加，不是仓库、模型或 DNS 故障。改用显式 `command codex --dangerously-bypass-approvals-and-sandbox -m gpt-6.1-terra` 创建终端并等待 `tui-idle` 后，以同一 `task_cf842168c554` 启动 `ctx_37bc7b2c19a2`，恢复范围已发送（`msg_1740050d52de`）；这是启动故障的最小绕过，不改变唯一实现 owner 或冻结契约。复核 `worker-show` 为 `ready`、`activity=working`、`lastActivity=just now`；M1 尚未报告完成，M2/M3 仍未启动；等待 worker_done/escalation 唤醒，不轮询。

- M1 完成与 M2 放行（2026-11-10）：`ctx_37bc7b2c19a2` 以 `worker_done succeeded` 报告，`task_cf842168c554` 已 completed。证据承认真实一小时到期、HTTP 容量触顶、独立进程故障注入和当前代码干净隔离构建未验证，保留格式化事故。协调者实际复跑 `cargo test --manifest-path prototype/Cargo.toml` 全绿，并执行 `python3 prototype/examples/orca_m1_verify.py` 得到 `PASS health, auth, scope, atomic rejection, durable, dedup, conflict, device isolation, bad quality, sample time, ordering, raw history` 与 `DuckDB SELECT version(): v2.0.0`；抽查确认 `source_device_grant` 独立表按 `[valid_from_ms, valid_to_ms)` 逐请求查询，请求标识由进程序号生成。该复核只证明 M1 自证可重放，不等于 M2/M3 独立验收。已在同一 Run 启动 M2 `task_0a73f16941fc` / `ctx_977e5473a009`，新终端 `term_e8f801bd-5946-420f-827a-4daaafd60c89` 运行显式 `command codex ... -m gpt-6.1-terra`（绕过同一 zsh function 故障），测试范围已发送（`msg_24d7938c13c1`）；M3 继续等待 M2。

- M2 模型路由阻塞与恢复（2026-11-10）：`ctx_977e5473a009` 在读取基线后收到 `404 ... magpie knows no model "gpt-6.1-terra"`，未提交 worker_done，任务没有交付；该错误是 provider 模型目录变化，不是 M2 测试结论。已 `worker-abandon` 并关闭外部终端 `term_e8f801bd-5946-420f-827a-4daaafd60c89`，保留失败记录。只读核对发现本地 cc-switch 的 Sub2API 配置仍列出 `gpt-6.1-terra`，且 `www.llmname.com` 已恢复解析与 HTTP 可达；新建 `term_46aa6a7a-31c3-4e14-8f67-8ef15665a9d5`，以 `model_provider=sub2api`、`requires_openai_auth=false`、`env_key=OPENAI_API_KEY` 启动同一 Codex/Terra，未在输出中打印密钥。M2 在同一 `task_0a73f16941fc` 重新启动为 `ctx_e458074f012e`，范围消息 `msg_389e32f0528f` 已发送，复核为 `ready/live/working`。先前 404 attempt 保留为失败历史；M3 仍等待 M2。

- M2 Terra provider 503 阻塞（2026-11-10）：`ctx_e458074f012e` 在读取计划后收到 `503 Service Unavailable`，错误来自 `http://www.llmname.com/responses`，未产生 M2 脚本或证据文件。独立探针：Sub2API 的 `/v1/models` 返回 503，`/v1/responses` 以 `gpt-6.1-terra`、`ping`、`max_output_tokens=1` 也返回 503 `Service temporarily unavailable`；这证明不是测试断言失败。已 `worker-abandon` 并关闭 `term_46aa6a7a-31c3-4e14-8f67-8ef15665a9d5`，同一 `task_0a73f16941fc` 置为 `blocked`，两次 provider 失败 attempt 均保留。M3 继续 pending；需要 Terra 服务恢复或用户批准改用 magpie 当前可用模型后才能继续。未将 M2 标为通过，也未改实现或测试范围。

- M2 应急模型继续（2026-11-10）：用户回复“继续”后再次探测 Sub2API，`/v1/models` 仍为 503 且错误为 `No available OpenAI model discovery accounts`。为不中断独立验证流程，将 `task_0a73f16941fc` 重新置 ready，使用 magpie 当前可用的 `auto/kimi-k3.1-pro` 启动新终端 `term_a362bfc2-8eab-4ece-a22f-2f5a227ffbc0`，绑定 dispatch `ctx_b616386522e7`；范围消息 `msg_31a7cc2d6cce` 明确要求 evidence 记录模型替代、provider 故障及“不得声称 Terra 验证”。当前 `ready/live/working`，M3 仍等待 M2。该决定是用户要求继续下的应急降级，不改变冻结契约、测试范围或验收标准。

- M2 首轮传输异常裁决（2026-11-10）：M2 报告首轮主命令退出 1：前 26 项通过后，首次 durable POST 附近 `http.client` 抛 `ConnectionResetError [Errno 54]`；独立新会话同请求经 `http.client` 与 `curl` 均得到 503 `DURABILITY_BACKPRESSURE`，暂不能稳定归因实现。已答复 `msg_a17a8f5d8f8d`：授权 M2 先增强 smoke 的请求关联、请求体发送状态、期望/实际 state、headers/body、异常类型与耗时记录，并在新进程复跑至少两次；禁止修改实现、wire 或服务，禁止把重置当通过。若 reset 在两次干净运行中复现，保留失败并由 M1 排查早响应/未消费 body 的传输时序；若不再复现，仍保留首轮失败及不确定性。M3 继续等待。

- M2 裁决重发（2026-11-10）：M2 worker 报告 `orca orchestration check` 为空并保持 idle，未收到前一条 question reply；同时其 `worker-show` 调用缺少 `--dispatch`，属命令用法错误。已通过 dispatch 邮箱重发同一裁决 `msg_77774adbaae1`，并给出正确命令示例；复核 worker 从 idle 转为 `working`，M2 继续增强 smoke 并复跑，M3 仍等待。

- M2 邮箱投递修正（2026-11-10）：M2 再次报告 `check` 为空并 escalation，核对 `inbox --terminal dispatch:ctx_b616386522e7` 发现 `msg_a17a8f5d8f8d`、`msg_77774adbaae1` 等实际仍在 dispatch 邮箱且 `handled=false`；终端邮箱为空，说明 worker 的 `check` 与 dispatch mailbox 绑定不一致，不是消息丢失。未改用终端直接注入，仍通过 Orca `send --type dispatch` 重发 `msg_467f225b61b0`，复核 worker 从 idle 转为 `working`，M2 继续增强 smoke 并复跑，M3 等待。

- 恢复协调者接管与 M2 续跑（2026-11-10）：新 session `codex_b2469cb7_b4e9_4432_80b0_7c5840e9f2c8` 已 `run-use` 接管原 Run，未新建主 Run；只读 ledger 确认 M1 已完成、M2 失败历史保留、M3 pending。原 M1 `ctx_5f4f0d72e3bd` 已 stopped/exited，不再作为 live owner；完成 attempt `ctx_37bc7b2c19a2` 保留。M2 旧 attempt 的三个 dispatch 全部失败，但 Orca 在旧 dispatch 上续派生成 `ctx_ee07e8d9fffb`/`task_a688916d7f97`，该 worker 正按协调者裁决修 smoke；权威 M2 卡仍为 `task_0a73f16941fc`，完成后由协调者归并并放行 M3。协调者真实复跑 M1 tests/verify 均退出0；真实复跑 smoke 退出1，定位唯一确定性失败为 future-skew 样本 1ms 裕量竞态，M09 `m2-stale` 503 未复现。已发送 `msg_248f3e53f8ea`，要求只修测试裕量、连续两次退出0、不得改服务/放宽断言；M3 继续等待 M2 事件，不轮询。

- M1 重开修复 403 早响应（2026-11-10）：M2 最终脚本在 M03 reader-write 连续两次 reset，已回复 escalation `msg_a159e87498fd`，不把 reset 当通过。协调者独立 4/4 得到 403，确认是时序竞态；根因是 `serve()` 在动作拒绝时读取 body 前返回 403 并关闭连接。已将 `task_cf842168c554` 重新置 ready，创建 Terra 终端 `term_228ad782-8057-4a97-ac26-93e256394460`（TUI 显示 `gpt-6.1-terra high`），以 `ctx_d567bcab28fc` 启动同一 task 的修复 attempt，范围只允许早拒绝 body 有界排空/优雅关闭、真实进程 50 次回归及 M1 证据；禁止改 M2 脚本/证据、wire、plan/status/Lead，禁止 reset/clean/checkout/commit 与整 crate fmt。M2 保持现脚本和证据等待，M3 仍 pending；等待 worker_done 唤醒，不轮询。

- 初始化：建立 Run 与四卡 DAG，M0 成功派发；下一步由 M0 报告事件唤醒，审阅 plan/证据及失败项后查询 ready 并启动 M1。M1–M3 尚未执行；当前无已证实功能阻塞，不等同没有缺陷。
- 用户新增并行授权：S1/S2/S3 均成功派发，互不写同文件；已通知 M0 支线报告位置，不增加生产冻结门禁；此刻仍无完成证据或功能 PASS。后续唤醒先处理报告/提问，再启动 ready 主链任务，所有新启动均 Terra。
- S1/S2 失败事件：核实原任务 failed，报告中的安全阻断项已送 M0 并登记 M1/M2 路由；显式安排原 owner 分别 S1-R1/S2-R1 纠正验证，未篡改原 failed 状态、未将设计当实测。等待这些报告或 M0/S3 事件；主链尚未有新 ready 卡，不轮询。
- S1/S2 复验与 S3 报告事件：前两复验完成，已按证据范围接受设计/审查产物；S3 自述成功被报告非零证据否定，已纠正 failed 并显式派发 S3-R1。此轮 ready 查询为空，M0 尚未报告；下一步等待事件，不因沉默重启，不把侧审代替实现或最终验收。
- S3-R1 完成事件：独立静态复验接受，三个支线均以新复验卡交付有效报告，三个原失败卡保留历史。M0 尚待报告，构建/HTTP/安全修复仍未完成，等待事件推进主链。
- M0 初轮失败事件：plan 已给出有限开发profile和 M01–M12，非完整P0；主链不以“文档已在”越依赖。官方同Task retry已启动Terra M0-R1，仅独立复验已修正文档。M1/M2/M3仍未启动，等待M0-R1最终证据后推进。
- M0-R1 成功事件：核对静态复验和ready后启动Terra M1，唯一实现owner已收到安全、测试及构建线索。下一步等待实现报告/问题事件；M2依赖M1，M3依赖M2，不能提前报端到端或安全通过。
- M1 初轮未实施事件：现缓存基线build/test通过但没有代码交付、无具体外部阻塞。首轮failed保留，核对settled后派发同卡M1-R1继续实际实现；旧owner不再写，新owner独占原实现scope。M2/M3继续pending，不把原型已有测试冒充MVP通过。
- M1-R1 端口阻塞提问：默认18080冲突，已明确授权可配置空闲loopback端口并继续实现，未改契约/plan、未授权干预其他进程。骨架与引擎版本为worker阶段性自述，尚无服务完整交付或独立HTTP通过证据。
- M1-R1 格式化事故提问：已答复保留现场、不做破坏性还原、停止整库格式化，要求快照和精确披露；未证实纯格式/完整保护原稿，不隐瞒该限制。实现继续，读取超时等具体缺陷仍未验收，M2/M3不提前放行。
- M1-R1 供应商中断恢复：DNS恢复后在同Dispatch续跑；已通知该worker引用Lead报告并补独立grant时间窗。Task保持dispatched，未新起writer；M2/M3继续pending，等待worker_done或ask。

- M1-R2 早响应传输修复（2026-11-10）：M2 在 M03 reader-write 发现有效 reader token 写 telemetry 预期 403，却在新连接连续两次 `ConnectionResetError`，证据 `/tmp/iotfast-m2-begnr818/results.json`。协调者独立 4/4 正常 403，判定为早响应与未消费 body 的时序竞态；已把 `task_cf842168c554` 重开。首次 Terra attempt `ctx_d567bcab28fc` 因 magpie `404 knows no model gpt-6.1-terra` 失败并保留；同一 task 改以 `auto/kimi-k3.1-pro` 启动 `ctx_1457e75b01a3`，范围只允许早拒绝 body 有界排空/优雅关闭、真实 50 次回归与 M1 证据，禁止改 M2 脚本/证据、wire、plan/status/Lead，禁止 reset/clean/checkout/commit 与整 crate fmt。M2 等待修复后最终两跑，M3 仍 pending；不轮询。

- M1-R2 完成与 M2 最终重试（2026-11-10）：`ctx_1457e75b01a3` 以 `worker_done succeeded` 交付有界 `drain_body()` 修复和真实 52 请求回归；协调者独立 build、16 项 targeted tests、M1 verify、early-reject verify 均退出0，0 reset，DuckDB v2.0.0；task_cf842168c554 completed。全量 cargo test 因 13:19 外部新增的未跟踪 `mvp_thing_model_server_test.rs` WebSocket 用例失败，不在 M1 修改路径，未修改。协调者单跑最终 smoke 106 项通过；已用 `--retry-of ctx_b616386522e7` 把权威 M2 `task_0a73f16941fc` 重新启动为 `ctx_04c09ae29684`（Kimi 应急模型），要求连续两次 exit0 并追加证据。M3 仍 pending，等待 M2 事件，不轮询。

- M2 完成与 M3 Terra 放行（2026-11-10）：M2 `ctx_04c09ae29684` 两次 106/106 PASS、reset_traces=0、DuckDB v2.0.0，task_0a73f16941fc completed；协调者独立复跑两次同样 106/106、exit0。Magpie 模型表确认 Terra 的可路由名为 `auto/gpt-6.1-terra`，裸名不可用；已用该模型创建终端并在 `task_6ec4562cd029` 启动 M3 `ctx_7864f88f5acb`。M3 只追加 `orca-mvp-m3-independent-acceptance.md`，必须真实启动服务、两次 smoke 并独立抽查 403/durable/grant/latest/raw；未覆盖的一小时到期、429、DB 注入、干净构建必须明示。等待 M3 worker_done，不轮询。

- M3 首轮失败与工具纠正（2026-11-10）：M3 `ctx_7864f88f5acb` 报 failed，但两次真实 smoke 均 106/106 PASS；失败是探针 TypeError 与空白正则误报两项验收工具非零，修正后均通过，未发现服务缺陷。按“非零不隐藏、可纠正工具”的原则保留首轮失败与证据，已用 `--retry-of ctx_7864f88f5acb` 在同一 `task_6ec4562cd029` 启动 M3-R1 `ctx_998e7a0a6d5a`（Terra），只允许修 M3 探针/检查并重跑必需命令；发现真实服务偏差才退回 M1。等待 M3-R1 worker_done，不轮询。

- M3 旁证与权威复验并行（2026-11-10）：被放弃的 follow-up `task_59b589fdea8f` 关闭前追加了 Terra 复验段到旧 M3 验收文件（两次 smoke、20 次 403、2 行观测、2 条 grant），已置 failed/superseded，只作旁证；权威 M3-R1 `ctx_998e7a0a6d5a` 继续在 `task_6ec4562cd029` 独立复验，并被告知不得覆盖旧节、不得只引用旁证。等待其 worker_done，不轮询。

## 范围与验收交付

M0 已创建 [本地 MVP 范围与验收](orca-mvp-plan.md) 与 [M0证据](evidence/orca-mvp-m0.md)，首轮失败保留，M0-R1独立静态复验已放行M1实施。该计划固定 `dev_server`、真实开发秘密/一小时固定授权范围、裸工程值遥测与设备view/raw、显式volatile pending ACK和durable503、有界无游标历史；WS/source-state/动态配置/生产认证可靠性均未实现。最终实际运行命令、未实现项、结果与风险在M1–M3证据后更新，不提前宣称成功。


## 最终协调状态（2026-11-10）

- 恢复协调者以当前合法 session 成功接管原 Run；因 dispatch 邮箱与终端邮箱分裂，完整复验指令通过 `msg_3aec8c52a827` 发到 M3 终端邮箱。随后收到原 task/dispatch 的 `worker_done`，outcome `succeeded`；`task-list --run run_02b4df14640f --json` 确认 M0、M1、M2、M3 均为 completed。旧 dispatch 邮件未送达的 escalation 保留；终端邮件已触发实际复验，无须再重派旧消息。
- 已核对 [M3 证据](evidence/orca-mvp-m3.md) 的 M3-R1 追加段：历史查询补齐 `aggregation: null`、`cursor: null` 后，完整独立探针退出 0；缓存构建退出 0，两次 smoke 各 106/106，DuckDB 只读核对及未跟踪文件正文空白检查退出 0。首轮探针失败、复验前置请求 422 和临时数据库脚本缩进错误均保留；这些非零记录不被改写为成功，最终结论仅针对修正后必验复跑。
- 该结论仅适用于本机限定只读开发 profile；真实一小时到期、HTTP 429、独立进程 DB 故障注入、durable duplicate、并发、干净隔离构建、生产认证与恢复、性能和现场设备未验证。全量 cargo test 曾有外部新增 WebSocket 用例失败，未修复；历史格式化对既有 dirty 文件的影响无法完整证明已恢复。生产 Q01–Q12 未关闭，不宣称完整 P0 或生产就绪。
- 工作区既有和本 Run 的未提交改动均保留；未执行 reset、clean 或提交。历史辅助任务失败与后续纠正记录保持不变。
