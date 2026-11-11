# S2：本地 MVP HTTP smoke 与 fixture 测试设计

状态：**设计交付，未运行服务或 smoke，不代表功能通过**。任务 `task_3522f6b151bc`；唯一允许写入本文件，禁止改 wire、代码、脚本、实际 fixture、manifest/lock、plan/status 和其他证据。M2 拥有后续脚本。停止条件：契约冲突或无法确定的字段交 M0，不能从原型默认行为推断；命令非零不得报告通过。输入为当前 dirty checkout；M0 `docs/development/orca-mvp-plan.md` 在设计时尚不存在，本设计按已授权最小演示目标提出，须待 M0 对齐。

权威依据：`docs/architecture/design-specification.md` §§18、21–22、24；`docs/specifications/service-identity-and-scope-contract.md` §§2–3；`docs/specifications/northbound-api-and-subscription-contract.md` §§7、9；`docs/development/specification-decisions.md` Q02/Q03/Q08/Q12。具体 HTTP 仅引用这些文件明示的路径和字段；本地演示的固定配置与限额不是生产承诺。下列是未来断言，不是实测事实。

## M0 待核准事项

本报告提出的端点、字段、状态码和分页断言均须由 M0 核准适用范围；其中引用的权威设计不等于本地演示 wire 已冻结。以下是相关用例的对齐事项，不是要求 M0 在本轮解决所有生产契约问题的无限前置条件；未纳入演示范围的用例应明确标记未覆盖，而非修改 wire 或算作通过。

1. 确认启动命令、隔离目录与配置格式、监听地址/端口、健康检查、预置模型/绑定/资源 ID 的获得方式；拒绝非 loopback 绑定的启动错误及退出码。不可用动态 API 造测试前提。
2. 确认开发凭据安全注入、两种服务端固定 scope、授权动作、错误响应 envelope：缺失/错误凭据应认证失败且不泄漏；精确 HTTP 状态和错误码尚待冻结，不能任选原型结果。禁止日志打印凭据。
3. 确认 latest 路径（权威候选 `GET /v1/devices/{device_id}/shadow`）、响应字段、绑定版本可见性、资源 ID；确认 history 查询的最大范围、limit 边界、等待历史可见的有限超时和不支持特性的错误。明确 volatile-only 服务拒绝 `Prefer: durability=durable` 的具体非 2xx 状态及错误码；不得返回 `accepted_volatile` 代替显式 durable 要求。
4. 确认 bad 测量与独立状态事件的支持及契约、stale 阈值/可控时钟、future drift 规则、DB 只读对账方法与 schema、关闭后 DB 可读性，以及未定义字段/未知字段行为。缺任何必要项时对应测试标记阻塞，不能将跳过算通过。尤其 bad 质量事件不得伪装为新测量。

## fixture 与独立运行

纯虚构设备 `demo-a`、`demo-b` 共用模型版本 `mr-demo-1` 和 `temperature`（float64、°C）；各自绑定 `br-demo-a-1`、`br-demo-b-1`，资源 ID 由 M0 固定或经授权查询获得，记为 `RA`、`RB`。两设备均绑定 `gateway_normalized`；测试配置可保存倍率 0.1 作为追溯元数据，网关已算出的 24.5 / 31.2 必须原样输出，绝不能再变成 2.45 / 3.12。不得声称能反推出原始寄存器。模型、设备、绑定和范围由隔离的启动配置预置，不假设控制面 API 已实现。

凭据 G 授权来源 `gateway-demo` 和两设备写入；读取凭据 R 仅授权 RA，读取凭据 ALL 授权 RA/RB；错误凭据 X 无效。均为每次运行临时测试秘密，通过 M0 支持的安全渠道注入，不能入仓、URL、日志或证据；测试输出只写代号。若 M0 不提供这些独立授权，相关用例阻塞。

选每轮当前 UTC 毫秒 `T`，至少距 stale 阈值和未来漂移边界有安全余量；下方数字仅是可解析的示例，执行时以整数替换时间，保持 A1=`T-2000`、A2=`T-1000`、迟到=`T-1500`、B1=`T-1000`。用单独的新 `source_epoch` 和消息 ID 每轮隔离；`source_seq` 为十进制字符串。例 A1（B1 仅替换设备、绑定、message_id、值与序列）：

```json
{
  "spec": "iotfast.telemetry.v1",
  "message_id": "s2-a1",
  "source_id": "gateway-demo",
  "source_epoch": "s2-epoch-1",
  "device_id": "demo-a",
  "model_revision_id": "mr-demo-1",
  "binding_revision_id": "br-demo-a-1",
  "points": [{"property": "temperature", "value": 24.5, "source_at_ms": 1794277598000, "source_time_origin": "gateway_sample", "source_seq": "9", "quality": {"state": "good", "source_protocol": "modbus", "source_code": null}, "delivery": {"replayed": false}}]
}
```

整批拒绝样例：同设备同消息含一个合法点和一个未知属性，须先确保 `not_in_model` 确实不在固定模型中。两点同时间和序列不涉及重复 property。

```json
{
  "spec": "iotfast.telemetry.v1",
  "message_id": "s2-atomic-invalid",
  "source_id": "gateway-demo",
  "source_epoch": "s2-epoch-1",
  "device_id": "demo-a",
  "model_revision_id": "mr-demo-1",
  "binding_revision_id": "br-demo-a-1",
  "points": [
    {"property": "temperature", "value": 99.0, "source_at_ms": 1794277599000, "source_time_origin": "gateway_sample", "source_seq": "20", "quality": {"state": "good", "source_protocol": "modbus", "source_code": null}, "delivery": {"replayed": false}},
    {"property": "not_in_model", "value": 1.0, "source_at_ms": 1794277599000, "source_time_origin": "gateway_sample", "source_seq": "20", "quality": {"state": "good", "source_protocol": "modbus", "source_code": null}, "delivery": {"replayed": false}}
  ]
}
```

历史请求模板（`RA` 执行前替换为服务端可信资源 ID；时间以本轮 T 改成有界半开区间，limit 在 M0 配额内）：

```json
{"resource_ids": ["RA"], "range": {"start_ms": 1794277597000, "end_ms": 1794277600000}, "mode": "raw", "aggregation": null, "quality_states": ["good", "uncertain", "bad"], "order": "asc", "limit": 100, "cursor": null}
```

## 可执行测试序列与断言

每步记录请求 ID、脱敏输入、HTTP 状态和错误码、响应、断言及前后查询结果。请求 `POST /v1/telemetry` 显式带 `Prefer: durability=volatile`。使用有限轮询及 M0 规定超时等待历史可见；超时失败，不用固定 sleep 冒充提交。`received_at_ms` 是服务生成时间，不等于采样时间；对账按 observation ID 或明确业务键，不依赖返回顺序或自造 ID。

| ID | 操作 | 期望与无副作用 |
|---|---|---|
| S01 | 独立临时目录启动，仅绑定 `127.0.0.1`；再用 M0 指定非 loopback 地址另起进程 | 本地连接成功；非 loopback 配置必须拒绝启动且不监听。用监听地址检查和可用的非 loopback 接口探测交叉验证；无接口时记未验证，不以本机 loopback 成功证明隔离。 |
| S02 | 对写入与读取分别不带凭据、带 X，之后带客户端自报 tenant/scope/资源范围尝试访问 RB | 认证失败；无效凭据即使附自报 scope 仍失败；R 的自报 scope 不能扩权，RB 单点与含 RA/RB 的历史批查均 `404 RESOURCE_NOT_FOUND`，不回显越权 ID；合法 RA 读取仍成功。自报字段或头名称、拒绝码待 M0 定义；前后 latest/raw 不变。 |
| S03 | G 上报未授权 source 或设备 | `403 SOURCE_NOT_AUTHORIZED`，整条拒绝；ALL 查询两设备 latest 和同时间窗 raw 前后完全一致。缺动作的凭据如 M0 提供，额外断言 `403 FORBIDDEN_ACTION`。 |
| S04 | G 发 A1 和 B1 | 各 `202 accepted_volatile`、`history_status=pending`，记录 ingest_id；分别查询 latest：设备、模型、绑定、时间、good、工程值 24.5 / 31.2 不串设备；freshness 在新鲜窗口为 fresh（实际枚举待 M0 确认）。不得返回 `accepted_durable`。 |
| S05 | A2 上报 25.0、`T-1000`、seq `10`；再发迟到 24.7、`T-1500`、seq `11`，另发同 `T-1000`、seq `9` | 接受后 latest 仍为 A2；迟到及同时间低序列进入 raw，不能覆盖 latest；数字比较而非字典序。确认 A/B 各自采样时间、版本不变串。 |
| S06 | 构造 M0 已冻结的 bad 测量或独立点位读取超时状态事件；之后等待或推进可控时钟越过 stale 阈值 | 精确比较 M0 事件语义；独立状态事件须保留 last-known 值及原 `value_sampled_at_ms`，独立更新质量时间，不能产生新测量。stale 不改变原质量，明确 stale，不显示实时正常；无事件契约/时钟则阻塞，不伪造通过。 |
| S07 | 发整批无效样例；另测错误模型/绑定版本 | 非法点 `422` 并含逐点错误；版本冲突 `409`，具体码依 M0。前后比对 A/B latest、该窗口 raw 和 DuckDB 对应记录集合/计数，合法点 99.0 也不得出现；查询需覆盖完整分页和历史异步收敛。 |
| S08 | 独立新消息显式请求 `Prefer: durability=durable` | volatile-only 服务明确非 2xx 拒绝，不得降级成功或声称 durable；前后 latest/raw/DB 无新增。具体码依 M0。 |
| S09 | `POST /v1/history/query` 查询 RA/RB，覆盖 `[start_ms,end_ms)` 边界和小 limit 分页 | `200`，仅范围内已接受测量、半开边界正确；按 `(source_at_ms, observation_id)` 稳定排序，分页不重不漏；每项匹配设备、属性、值、质量、采样时间、版本及来源时间标记。空窗口为空；范围外不得出现。具体分页支持待 M0 冻结。 |
| S10 | 相同/逆序时间范围、超配额范围或数量、非法 limit、非法 mode/类型 | `422` 参数拒绝；不产生数据变更；限额值及错误码按 M0 固定。不得接收任意 SQL 或表名；相关字段未冻结则列阻塞而非自造 wire。 |
| S11 | 历史可见后，按 M0 安全方式只读 DuckDB；与 HTTP raw 全量结果、各设备 latest 比较 | 只比较已接受的观测及实际定义的存储字段：值、质量、源时间、设备、属性、模型/绑定版本，集合/数量一致；latest 是顺序规则选出的当前值而非每条历史。DB 不可安全并发读取则按 M0 方式正常关闭后只读；不能用数据库行存在证明 ACK durable。 |

所有负例前后做授权快照，比较包含两设备、完整时间窗和分页的 latest/raw/DB；异步写入先收敛再采基线。没有 DB 对账方式时 S07/S08/S11 未通过而非默认成功。若实现支持幂等，可追加相同 key/内容重试 `200 duplicate` 和同 key 不同内容 `409 IDEMPOTENCY_CONFLICT`，但 volatile 重启不承诺去重。

## 两次运行、清理与证据

M2 连续执行两次完整序列，每次新建操作系统临时目录、独立端口、唯一 epoch/消息 ID 和新秘密，空 DB；启动前验证目录属于本轮且不存在旧进程，端口冲突直接失败，不杀不属于测试的进程。正常或异常退出均仅停止自己启动的 PID，等待退出；保存脱敏日志、配置摘要、请求响应、断言、DB 对账结果和退出码到 M2 授权证据位置，再清理自己创建的临时目录；清理失败报告，不删除用户数据。两轮分别报告 PASS/FAIL/BLOCKED 和原因，任一失败即整体未通过；不能以第二轮覆盖第一轮。不得让真实设备、凭据或生产数据进入测试。

每轮记录 OS/CPU、文件系统、HEAD 和 dirty、运行配置、端口、依赖版本、实际命令/退出码、测试数据来源、启动/停止与清理结果。WS、动态配置 API、生产认证、WAL/崩溃恢复、故障重放、真实网关、性能及可靠性不在本 smoke 通过范围；未经单独证据不得宣称实现或生产可用。

## 本次设计验证

环境：Darwin arm64，HEAD `c0a5a42`，dirty checkout；与 M0/S1/S3 并行，M0 plan 设计时不存在。未运行 cargo、服务、HTTP 或 DuckDB；设计通过不等于实际 smoke 通过。验收命令及真实退出码见下方；JSON 仅在系统临时文件校验，不生成仓库 fixture。

- `git status --short`：退出码 0，已有大量并行 dirty 文件，本报告为未跟踪文件。
- `git diff --check -- docs/development/evidence/mvp-s2-test-design.md`：退出码 0；但未跟踪文件不在普通 diff 中，该结果不覆盖正文。
- 将三个 JSON 块提取到 `tempfile.TemporaryDirectory()`，逐个执行 `python3 -m json.tool <临时文件>`：各退出码 0，提取及校验命令退出码 0。
- 首次组合校验命令被执行环境拒绝（含 `rm -rf`），未启动；第二次组合命令中的 Python 因引号错误 `SyntaxError`，退出码 1，后以修正命令复验成功。`git diff --no-index --check /dev/null docs/development/evidence/mvp-s2-test-design.md` 无空白错误输出，但退出码 1（文件存在差异）；按本任务非零即失败规则记录为失败，不能将本次任务报告为 completed。

## 独立复验（新任务）

任务 `task_45ba3d310396`，Darwin arm64，HEAD `c0a5a42`，dirty checkout，2026-11-10 CST。首轮任务仍为 failed，以上原始记录保留；`git diff --no-index` 的退出码 1 表示存在差异，不是空白错误，但不追溯改变首轮结果。本轮只验证设计产物，不运行 cargo、HTTP、服务或实际 smoke。

本轮执行 `git status --short` 与 `git diff --check -- docs/development/evidence/mvp-s2-test-design.md`，退出码均为 0；后者不覆盖未跟踪报告正文。下列独立命令对正文全部三个 JSON 块用临时文件调用 `python3 -m json.tool`，并检查每行尾随空格、单个 EOF 换行；退出码 0 表示全部检查通过：

```sh
python3 - <<'PY'
from pathlib import Path
from tempfile import TemporaryDirectory
import subprocess

text = Path('docs/development/evidence/mvp-s2-test-design.md').read_text()
assert text.endswith('\n') and not text.endswith('\n\n')
assert all(line == line.rstrip(' \t\r') for line in text.split('\n'))
blocks = [part.split('```', 1)[0] for part in text.split('```json\n')[1:]]
assert len(blocks) == 3
with TemporaryDirectory() as directory:
    for index, block in enumerate(blocks, 1):
        path = Path(directory) / f'fixture-{index}.json'
        path.write_text(block)
        subprocess.run(['python3', '-m', 'json.tool', str(path)],
                       stdout=subprocess.DEVNULL, check=True)
        print(f'fixture-{index}: exit 0')
print('trailing whitespace and EOF: passed')
PY
```

这仅证明 JSON 语法与报告文本格式，不验证字段契约、服务功能或生产能力；M0 核准和 M2 实测仍待进行。
