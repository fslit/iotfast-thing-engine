# M0 决议与文档验证证据

日期：2026-11-10；任务 `task_7c6ec9fcc2e6`；dispatch `ctx_a56029eee354`。唯一文档 owner；允许写入仅 [演示计划](../orca-mvp-plan.md) 与本文件。禁止现有台账 / 规范 / status、源码、manifest、公共文件写入；未 reset / clean / commit / 新建分支 / checkout，未联网研究、启动 helper 或执行代码实现。

## 1. 交付与决议

形成 `orca-mvp-dev-v1` 的待实现本地子集：原型 crate 的 `dev_server` 启动接口、IPv4 loopback、空隔离数据目录、三种真实校验的临时开发凭据与服务端显式资源映射、一小时有效期 / grant 上下界、拒绝自报 Scope、一个不可变模型两设备两绑定、gateway_normalized、权威遥测 / view / raw wire、原子拒绝、时序质量、异步 pending volatile ACK 与显式拒绝 durable。

历史只做单次只读事务内有界全结果，超额整次422、cursor非null拒绝，不称稳定分页。拒绝原型枚举value、扁平unit和虚构volatile WAL位置的外部序列化；新ACK保持总体§21.3 pending，不随原型同步committed。source-state、shadow、ingestion查询、WS、动态配置、生产安全 / 联合故障可靠性 / 性能和真实网关联调明确未实现或后续；Q01–Q12生产记录未改 / 未宣称关闭。

设计依据已按入口读取：[定位](../../architecture/data-engine-scope.md)、[README](../../../README.md)、[规范优先级](../../specifications/README.md)、[ADR0001](../../adr/0001-mvp-boundaries.md)、[总体设计§18–28及相关前文](../../architecture/design-specification.md)、[路线](../roadmap.md)、[验收](../acceptance.md)、[产品](../../../PRODUCT.md)、[AI任务包](../ai-execution-plan.md)、[决策台账](../specification-decisions.md)、[身份公共契约](../../specifications/service-identity-and-scope-contract.md)、[遥测](../../specifications/p0-telemetry-contract.md)、[北向](../../specifications/northbound-api-and-subscription-contract.md)、[数据库逻辑设计](../../specifications/database-schema-design.md)、[实现架构](../../architecture/implementation-architecture.md)、[D00蓝图](../../specifications/d00-integrated-implementation-blueprint.md)、[体验规范](../../product/integration-experience.md)、[历史证据更正](p0-core-engine-test-summary.md)。大型批量读取出现过工具输出截断，关键wire/时序/身份/ACK/历史条款随后用定点分段读取，不把截断输出当作完整代码覆盖。

S1 / S2 / S3是协调者提供的并行审查输入，不作为本卡生产硬依赖或功能PASS。吸收S1指出的history当前时间校验、grant valid_from_ms遗漏及可反序列化Scope风险；M1负责实现修复或证明HTTP不可达隔离，M2实测。吸收S2测试建议但固定本计划的epoch、latest=/view和wire；S2原建议不能反向覆盖权威契约。S3历史动态库 / target仅为构建线索，不是本轮真实链接引擎版本证据。

## 2. 工作区与检索边界

开始 `pwd` 为 `/Users/fengsl/code/work/iotfast-thing-engine`；HEAD `c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`。开始 `git status --short` 退出0：已有`.github/ISSUE_TEMPLATE/development.md`、`AGENTS.md`、README、架构 / 规范 / 路线 / 验收 / 历史证据、prototype README与lib修改，旧扁平模块删除；PRODUCT、实现架构、AI任务包、决策台账、产品目录、新core/catalog/ingest/query及协调者status文件未跟踪。全部为已有或并行他人改动，未回滚、未认领。开始两目标文件不存在；写入前再次检查目标无他人改动。

按codebase-memory skill尝试检查工具可用性：本会话没有 `list_projects` / `index_status` / `check_index_coverage` 等图工具；`list_mcp_resources` 与 `list_mcp_resource_templates` 均返回空列表，**未执行图索引检查、不声称图覆盖**。使用semble两次聚焦检索（telemetry validation/latest/history/scope；TelemetryEnvelope/IngestReceipt/HistoryQuery structs），返回后直接读 `prototype/src/ingest/validate.rs`、`prototype/src/ingest/storage.rs`、`prototype/src/query/northbound.rs`，再由模块兼容导出定位 `prototype/src/core/contract.rs`、manifest与lib。只做定点原型与wire对照，没有完整调用图或安全审计结论。

一次 `sed -n '1,150p' prototype/src/core/d00.rs` 找错重组路径，输出不存在；该串联命令末项退出0不能代表这个子命令成功。已由lib导出确认实际是 `core/contract.rs`并读取；这是路径探查失败，非验收PASS。

## 3. 环境与验证命令

实测环境：macOS 27.0（26A428），Darwin arm64；zsh；rustc `1.98.1 (48a229cea 2026-09-01)`；cargo `1.98.1 (797e8a9bc 2026-08-05)`；Python `3.14.7`；git `2.54.0 (Apple Git-157)`；OpenSSL `4.1.0`。环境命令 `sw_vers`、`uname -sm`、`rustc --version`、`cargo --version`、`python3 --version`、`git --version`、`openssl version` 各退出0。实际DuckDB链接引擎、HTTP运行 / 端口 / 内存 / 磁盘负载均未测，不能从这些版本号推导构建 / 性能PASS。

### 3.1 验收命令（仓库根）

```sh
git status --short
git diff --check -- docs/development/orca-mvp-plan.md docs/development/evidence/orca-mvp-m0.md
python3 - <<'PY'
import json
import re
from pathlib import Path
paths = [Path('docs/development/orca-mvp-plan.md'), Path('docs/development/evidence/orca-mvp-m0.md')]
for path in paths:
    text = path.read_text()
    for line_number, line in enumerate(text.splitlines(), 1):
        assert not line.rstrip() != line, (str(path), line_number, 'trailing whitespace')
    assert text.endswith('\n'), (str(path), 'missing final newline')
    for target in re.findall(r'(?<!!)\[[^\]]+\]\(([^)]+)\)', text):
        if '://' not in target and not target.startswith('#'):
            assert (path.parent / target.split('#', 1)[0]).exists(), (str(path), target)
    for block in re.findall(r'```json\n(.*?)\n```', text, re.S):
        json.loads(block)
plan = paths[0].read_text()
for expected in ['GET /health', 'POST /v1/telemetry', 'GET /v1/devices/{device_id}/view', 'POST /v1/history/query', 'UNAUTHENTICATED', 'FORBIDDEN_ACTION', 'SOURCE_NOT_AUTHORIZED', 'RESOURCE_NOT_FOUND', 'REVISION_CONFLICT', 'VALIDATION_FAILED', 'CLOCK_SKEW_EXCEEDED', 'IDEMPOTENCY_CONFLICT', 'CAPABILITY_UNSUPPORTED', 'RATE_LIMITED', 'DURABILITY_BACKPRESSURE', 'accepted_volatile', 'history_status', 'RESULT_LIMIT_EXCEEDED', 'source_time_origin', 'binding_revision_id']:
    assert expected in plan, expected
assert all(f'| M{number:02d} ' in plan for number in range(1, 13))
print('PASS: two documents, local link paths, JSON examples, whitespace, endpoint/error/field/matrix markers')
PY
```

本脚本只检验相对链接的本地路径可达，不证明Markdown anchor、机器Schema、JCS或服务行为正确；覆盖文字标记检查不是语义证明。`git diff --check` 不遍历未跟踪文件，故Python独立检查两个新文件的尾部空白与末尾换行，不仅凭git命令给新文件PASS。

首轮本节Python检查退出1：缺少明确字面量 `IDEMPOTENCY_CONFLICT`（计划原来仅写同key冲突409）。这是文档验收失败，已在计划§7补充权威错误码与触发条件；需复验后再结论，不能把这次失败记PASS。检查仅限M0文档；所有M01–M12功能用例尚未执行。

修复后复验：完整执行上述Python heredoc，退出0，输出 `PASS: two documents, local link paths, JSON examples, whitespace, endpoint/error/field/matrix markers`；`git diff --check -- docs/development/orca-mvp-plan.md docs/development/evidence/orca-mvp-m0.md` 退出0；`git status --short` 退出0，除已记录的他人dirty / 并行报告外，本卡仅新增两份允许文档。文档终验通过范围仅上述静态检查，首轮退出1保留；不声称功能通过。

### 3.2 先前非零记录

首次 `git diff --check -- docs/development/orca-mvp-plan.md docs/development/evidence/orca-mvp-m0.md` 退出0，但当时evidence尚不存在，且plan未跟踪，不能当作两文件内容校验。补做 `git diff --no-index --check /dev/null docs/development/orca-mvp-plan.md` 返回1、无输出；不将此非零标PASS，也不依赖它验收，改为上面直接检查新文件内容的Python命令后再记录终验。无cargo / lint / build / HTTP测试失败被本文件隐藏成成功。

## 4. 未执行与后续

- 本卡未执行cargo build/check/test/run、依赖下载、DuckDB版本运行查询、服务启动 / HTTP smoke、崩溃 / 物理断电、真实网关或性能验证；M0文档PASS不等于任何功能PASS。
- M1按协调者授权的实现范围交付二进制和wire转换、严格解析、身份边界、队列 / 事务；不能把历史缓存、serde默认行为和现有原型receipt直接公开。
- M2对本计划M01–M12给独立实测，未实现项列未执行 / 范围外，任何非零列失败；若实现与契约冲突请协调者裁决。
- 归档命令 `python3 /Users/fengsl/.local/share/orca-obsidian/sync.py --session current --summary-file /tmp/orca-mvp-m0-summary.RAzPub` 退出0，updated=1，updated_paths给出 `/Users/fengsl/Documents/Obsidian Vault/Orca/10-任务记录/2026/2026-11-10-M0 范围契约与最小开发profile决议-8d06967ff3.md`，据此报告本会话摘要已同步；未直接重写自动笔记 / 人工区。回执另有60个其他历史session待补，不把它们算本任务归档完成。

## 5. 任务结论

两份文档已产出，修复后静态检查退出0，M1可据计划实施；但本dispatch曾出现验收Python退出1及no-index检查退出1，按本任务“非零验证按失败、不报completed”的规则，**本卡回报outcome=failed，保留失败而不宣称completed**。协调者可在保留首轮失败的前提下独立复验 / 另立复验卡确认；不是要求重做生产Q01–Q12。真实代码与HTTP功能仍未执行，本结论既不否定已落盘决议，也不将修复后文档检查伪装为功能完成。

## 6. M0-R1 独立复验

本节为同任务 `task_7c6ec9fcc2e6` 新 dispatch `ctx_d0d6218e3587` 对原失败 dispatch `ctx_a56029eee354` 的复验；上文历史失败与结论保持不变。协调者确认旧 owner 已结束且本轮独占两份文档。本轮未改 plan，仅追加本证据；开始时两目标均未跟踪，保留现有 dirty 工作区。

环境：macOS 27.0 (26A428)、Darwin arm64、zsh 5.9、Python 3.14.7、git 2.54.0 (Apple Git-157)。仓库根依次运行本文件 §3.1 中完整的 `git status --short`、`git diff --check -- docs/development/orca-mvp-plan.md docs/development/evidence/orca-mvp-m0.md` 和完整 Python heredoc，三者退出码均为 0；Python 输出 `PASS: two documents, local link paths, JSON examples, whitespace, endpoint/error/field/matrix markers`。Python 直接读取两份未跟踪正文，检查末尾换行、尾部空白、相对链接路径、JSON 示例，以及端点、错误、字段、M01–M12 矩阵标记；它不验证锚点、Schema 或服务语义，`git diff --check` 本身不检查未跟踪正文。

本次仅是文档静态复验，不是功能 PASS；未运行 cargo、构建、HTTP、真实网关或生产安全、可靠性和性能验证。M1 / M2 仍须实现并实测计划，历史生产 Q01–Q12 仍未关闭。
