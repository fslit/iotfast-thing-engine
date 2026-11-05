# D00-C：跨语言幂等规范化候选方案

状态：评审草案，**未冻结、未实现、未通过跨语言协议一致性测试**；适用范围仅为标准化遥测 measurement，质量事件需独立 profile。关联 [质量事件与幂等设计](d00-measurement-quality-and-idempotency.md)、[D00 决策清单](d00-contract-review.md) 及 [P0 入站草案](../specifications/p0-telemetry-contract.md)。协议改变需评审并同步相关文档，本文不自动覆盖现有规范。

## 1. 问题

网关可能使用 Go、JavaScript 等语言，引擎使用 Rust；同一业务消息的 JSON 字段顺序或空格可以不同。直接对请求原文求哈希会将这些重试当成不同消息。相反，忽略时间、质量或版本又可能把不同采样错误当成相同消息。

去重身份与内容摘要分开：
- key：可信服务上下文 tenant_id + source_id + source_epoch + message_id + 消息类型域；调用者身份和资源范围每次重试重新验证。
- hash：对经检查的业务语义投影和规范化字节计算，决定该 key 的内容是否一致。
- 初次成功接收时分配一次 ingest_id 与各 observation_id，写进可恢复记录；重复请求引用同一结果。

客户端 request_id、重传标志 replayed、服务端 received_at_ms 不代表新测量，候选不纳入内容 hash；但认证、租户范围和记录完整性不得被略过。质量事件 event_id 与测量 message_id 在不同类型域。

## 2. 候选方案：语义投影 + RFC 8785 JCS + SHA-256

RFC 8785 的 JSON Canonicalization Scheme（JCS）给出确定性 JSON 输出，但并不定义本业务如何选择字段、数组是否可重排或 null / 缺省是否等价。本项目还需定义业务投影和类型规则，不能把“支持 JSON”当作“支持 JCS”。

候选 profile：measurement-jcs-candidate-v1，存入 WAL / 去重记录并与摘要一起版本化；profile 更改要有迁移 / 兼容方案，不能在保留窗口中按新算法重算旧记录而导致重试冲突。

### 2.1 输入校验顺序

1. 验证服务身份与可信租户 / 资源范围、请求大小和解析资源限制；不得仅凭 tenant_id / source_id 自报放行。
2. 解析 JSON；拒绝重复对象键、无效 Unicode / 非法 JSON、非有限数、重复 property、超限和未知字段（未知字段是否全部拒绝仍待正式 Schema 裁定；候选严格模式拒绝）。
3. 按模型和绑定验证设备、字段、类型、单位、时间及权限。对于已接收消息重试，须先重新验证当前身份权限并识别历史记录；不能在绑定切换后仅按新模型重新解释。完整重复请求的验收顺序见第 5 节。
4. 由明确列出的业务字段建立投影，缺失 / null 的语义由字段契约定义；未列入投影的其他字段不得在严格模式下悄悄影响采样结果。
5. JCS 输出 UTF-8 不含 BOM、无结尾换行，计算 SHA-256，小写十六进制；记录 profile、摘要及后续碰撞处置所需的经过审定的语义证据。

### 2.2 候选字段与排序

投影顶层包含 profile、spec、source_id、source_epoch、device_id、model_revision_id、binding_revision_id、points；message_id 和 tenant_id 属于 key，不重复作为投影字段。每个 points 项包含 property、value、source_at_ms、source_time_origin、source_seq、quality；各 quality 允许字段及缺失 / null 要由 Schema 决定。

**候选方案暂保留 points 数组顺序**：项目键位相同但重新排序会产生不同摘要。这是明确的候选互操作取舍，不是用户已经批准的产品约束。若决定点位顺序无关，必须在 JCS 前按冻结的 property 标识比较规则排序、拒绝重复 property，并更新版本及黄金向量。JCS 自身不重排数组。

### 2.3 数值与字符串

- JSON 浮点必须可表达为有限 IEEE 754 binary64；-0 与 0 的序列化效果、指数表示采用 JCS / ECMAScript 规则。输入解析 / 模型类型检查和实际 JCS 库行为必须测试。
- source_seq 为十进制字符串候选；范围、前导零和空格规则尚未冻结。超安全整数范围的 int64 采用约定的十进制字符串，不能先转 JS Number 造成丢失；同一字段只允许已评审的一种编码。
- RFC 8785 不进行 Unicode 标准化。标识符是否限制 ASCII、显示字段是否采用其他归一化需在业务层明确，不能以为 JCS 自动处理。
- quality.source_code 等可选字段的缺失与显式 null 不自动等价；先确定 Schema 和投影规则，测试前不可假定。
- 重复 JSON 对象键应在丢失信息之前拒绝；不能先由默认解析器吞掉再进行验证。

## 3. 精确样例：受限业务对象

以下 sample 是**投影之后**的 UTF-8 规范化文本，不是完整网关请求；键均为 ASCII，数组只有一项，数值不覆盖全部 JCS 情况。下面代码块每行文本不含前后空格；计算摘要时不包括 Markdown 代码块或其换行。

```json
{"binding_revision_id":"bind-1","device_id":"pump-1","model_revision_id":"model-1","points":[{"property":"pressure","quality":{"state":"good"},"source_at_ms":1700000000000,"source_seq":"42","source_time_origin":"gateway_sample","value":0.42}],"profile":"measurement-jcs-candidate-v1","source_epoch":"boot-1","source_id":"gw-1","spec":"iotfast.telemetry.v1"}
```

- UTF-8 字节数：357。
- SHA-256：`ddf758d6c44e596564f5447e1fda3b235aec00ea2f8754d74d52f3e4570fd922`。
- 将唯一的 `value` 改为 0.43，其余字段不变：357 字节，SHA-256 为 `1eb633502a9bfdfd979a73d03e23772f34f09be3179882ef7fae68703a8d6d61`。

候选下同 key、仅 replayed / request_id 变化而投影相同应返回原 ingest_id；相同 key、value 改为 0.43 应冲突。本样例用于校对字符和哈希，绝不是完整 JCS 测试集。

## 4. 已完成的局部核对

执行环境：2026-11-06，Python 3.12.14、Node.js v24.22.1；计算基于本节的虚构对象：
- Python 3 使用标准库对第 3 节仅 ASCII key / 单层数字示例进行 `json.dumps(sort_keys=True, ensure_ascii=False, separators=(',', ':'))` 与 `hashlib.sha256`；两个示例均为 357 字节并得到上述摘要。
- Node.js 用按照候选排序手动构造的同一个 0.42 对象，`JSON.stringify` + `crypto.createHash('sha256')` 得到相同的 357 字节与摘要。

**限制**：上述序列化仅对这个受限样例进行核对；Python 的通用 JSON 输出和手动排列的 Node 对象不是 RFC 8785 通用实现。未运行 Rust、Go 或真实网关，也没有覆盖 JCS 浮点边界、Unicode key 排序、负零、指数或错误数据，不能据此声称 JCS 一致性。

## 5. 重试与绑定切换的候选处理

1. 验证当前调用者身份、来源及资源读取 / 写入范围；权限已撤销就拒绝，不能从既存 key 推测未授权资源信息。
2. 验证包结构和消息 key 的基本合法性，查询有权限访问的既存记录。
3. 若已有 key，使用**当时存储的 profile**及约定的输入语义做比较；一致则按保留策略返回原 ingest_id / 当前处理状态；不一致则冲突。旧模型和绑定快照需可用；无法解释时返回明确错误，不能按当前绑定重算。
4. 若为新 key，则根据当前 / 历史有效的版本与业务规则做完整动态验证，获得每点稳定 observation_id，进入 WAL 或 volatile 接收。
5. 并发相同 key 时用可恢复的单一 pending / 原子条件提交协调；超时不提前返回 durable，重启依真实 WAL 恢复。

安全要求优先于历史可用性：已撤权的来源不能通过旧消息来查询先前响应。P0 原有整消息拒绝规则、已接受消息重试和历史绑定有效期之间需实现 / 故障实测证明，不能仅凭文档视为已完成。

## 6. 必须补齐的黄金向量

| 类型 | 待测输入 / 预期 |
|---|---|
| 对象键重排 | 语义对象相同，应得到同一规范字节 |
| points 反序 | 当前候选为不同；如果更改顺序策略需改变 profile 和预期 |
| replayed / request_id | 不影响业务投影 |
| 值 / 时间 / 质量 / 绑定变化 | 同 key 应产生不同语义，返回冲突 |
| null / 缺失 | 字段规则未定，冻结后明确预期 |
| 浮点 | 1、1.0、-0、最小 / 大数、指数、非有限数与模型约束 |
| int64 | 超 JS 安全范围的十进制字符串及上下限 |
| Unicode | 多字节字符、组合字符、不合法代理项、key 排序；禁止误用标准化 |
| 解析错误 | 重复 JSON key、未知字段、重复 property、结构深度和长度超限 |
| 并发与恢复 | 同 key 并发、WAL sync 前后、DB 提交 / checkpoint 和已撤权重试 |

正式验收：采用符合 RFC 8785 的经过审查实现、Rust + 实际网关语言分别运行同一输入与预期字节 / SHA-256 清单；记录运行命令、依赖版本、平台与提交 SHA。失败项、未测试项和真实网关样本缺口不得标成通过。

## 7. 参考

- RFC 8785 JSON Canonicalization Scheme：https://www.rfc-editor.org/rfc/rfc8785
- RFC 8785 是 informational 文档；采用前必须评估二进制浮点、跨语言类型精度及依赖维护状态。
