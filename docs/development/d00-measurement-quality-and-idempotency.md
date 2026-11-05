# D00-B/C：测量、质量事件与消息幂等评审方案

状态：候选设计，未评审冻结、未实现、未运行测试。适用于 Rust 设备数据模型引擎；遵守 [引擎边界](../architecture/data-engine-scope.md)、[P0 遥测契约](../specifications/p0-telemetry-contract.md) 和 [D00 评审清单](d00-contract-review.md)。与正式规范不一致时先登记并修订规范，不通过本文默认覆盖既有契约。示例数据完全虚构，供讨论与语法验证。

## 1. 问题和选择

场景：水泵最近一次压力采样为 0.42 MPa，之后网关连接超时，现场没有读到新压力。数据服务需要告诉上层“最后一次压力是多少”“它何时采到”“当前通信 / 质量是否异常”，同时避免将 0.42 当作断线时刻的新采样写入历史。

候选方案将 P0 输入分为两类：

1. measurement：一个设备的一批真实测量，沿用现有 iotfast.telemetry.v1 和整消息校验。
2. quality_event：来源对一个或多个属性报告质量状态变化的事件；新语义必须拥有独立 spec 和端点，不能悄悄改写 measurement 的 value 语义。

这不是要求所有已接入网关必须升级到该新事件协议。接入方式、接口路径、落盘优先级与兼容策略待评审；在质量事件功能未完成时，上层只能读取现有数据、时间和已知连接信息，不得谎称具备完整点位质量历史。

## 2. 测量和质量事件职责

| 项目 | 测量 measurement | 质量事件 quality_event |
|---|---|---|
| 表达 | 某属性的新测量事实 | 某属性质量 / 采集状态变化 |
| 值 | 必填且符合已发布模型；不接受 null 伪造测量 | 不带测量值；不能更新 last-known 数值 |
| 数据时间 | source_at_ms 为采样时间，携带来源 | observed_at_ms 为状态观察时间，携带来源 |
| 去重身份 | tenant + source + epoch + message_id，在测量命名空间 | tenant + source + epoch + event_id，在事件命名空间 |
| 事务 / WAL | 按选择的接收模式；durable 需要真实同步 | 若选择 durable 必须使用经验证的持久边界；不能继承虚假的承诺 |
| 历史 | 按历史策略记录测量 | 可记录独立 quality_event；不能计作测量点 |

一条 measurement 可以附带该采样的质量；它只表明该测量当时的质量。后来的连接超时属于新状态，更新质量事件 / 最新状态而非修改已发生的采样事实。资产连接状态、点位质量与 freshness 逻辑上分开，不能因在线即判 good。

## 3. 质量事件候选结构

以下是可评审示例，路径、长度、枚举、时间来源和错误码均未冻结：

```json
{
  "spec": "iotfast.quality-event.v1",
  "event_id": "q-pump001-0001",
  "source_id": "gateway-01",
  "source_epoch": "epoch-001",
  "device_id": "pump-001",
  "model_revision_id": "pump-model-r1",
  "binding_revision_id": "pump-bind-r1",
  "observed_at_ms": 1793869202000,
  "time_origin": "gateway_observed",
  "source_seq": "1002",
  "items": [
    {
      "property": "pressure",
      "state": "bad",
      "reason": "source_timeout",
      "source_code": null
    }
  ]
}
```

候选约束：items 非空且同事件 property 不重复；模型 / 属性 / 来源 / 生效绑定及可信数据范围校验通过后整事件接受或拒绝；服务端生成 received_at_ms。event_id 与 measurement 的 message_id 即使文本相同，也必须在不同消息类型命名空间去重。source_seq 的具体作用域 / 单调要求尚需和网关确定，不能先将它与测量序列混作单一全局序列。quality_event 代表明确状态事实；是否支持整设备连接事件、多点批量、重试窗口与源离线推断需进一步确定。

如果原协议只提供“网关离线”但无法判断单个属性质量，不能自动声称所有属性源质量明确为 bad；可以在服务端标注 connection=offline 和 freshness=stale，由上层看到未知 / 失联状态，映射规则需按来源能力配置。

## 4. 状态投影和边界时间

建议针对每个资源分别保存：

- last_value：最近合法测量的 value、value_sampled_at_ms、time_origin、observation_id 和所属模型 / 绑定版本。
- last_measurement_quality：该测量自身附带的质量，随观测事实固定。
- current_quality：最近被有效接受的质量事件 / 状态信息及 quality_observed_at_ms。
- connection_state：从可信连接状态或心跳推断的来源状态，和单点质量分开。
- freshness：基于所选时钟及 stale_after 策略计算，报告计算时间和策略版本；时钟不可信 / 无样本为 unknown。
- projection_revision：有序状态投影版本，用于快照和订阅一致性，不代表所有属性同步采样。

### 候选可见效果

| 序列 | value / value_sampled_at | current_quality | freshness / connection |
|---|---|---|---|
| 尚未采样 | null / null | unknown | unknown |
| t1 收到 0.42、good | 0.42 / t1 | good（来源为测量） | fresh / 当前可知状态 |
| t2 收到可信 timeout 事件 | 0.42 / t1 | bad、observed_at=t2 | 单独计算 / 可报告 offline |
| t3 收到新的 0.43、good | 0.43 / t3 | 需依据测量与质量事件的排序协议确定 | 单独计算 |

t3 是否自动恢复质量不能仅凭“消息后到”决定；不同 source / epoch / 时间不可信时不能比较。候选 P0 同一生效 source 与 epoch 内按可信源时间和序列排序；跨 epoch / source、空源时间、相等时间的优先级由绑定切换 / 具体协议另行冻结。超时事件不更新 value_sampled_at，不新增数值历史。迟到测量可满足策略写历史，但不得覆盖较新的 latest / 状态。必须用测试证明独立状态变化和快照订阅不会互相丢失。

## 5. 状态机与冲突清单

- 未采样 → 有效测量：创建 last_value，保存时间和该采样质量。
- 有效测量 → 源异常：保持 last_value，记录当前质量或连接事件；视图显示来源与各自时间。
- 源异常 → 重新采样：若时间与序列通过排序及当前绑定检查，更新 last_value；质量恢复规则应明确、可追溯。
- 任何状态 → stale：基于时间阈值投影，不改历史观测或原始质量。系统重启后按时钟 / 已持久化策略重算；时间倒退等异常标记 unknown。
- 模型 / 绑定切换：状态带版本；不能将旧资源、旧映射或旧采样当新模型的当前事实。切换窗口内先保持原状态和版本、标记同步中；是否保留旧 latest 需在正式接口冻结。

并发要求：按资源的测量 / 质量 / 配置变更需有明确顺序，或以版本冲突防止更新丢失；源消息唯一性与业务顺序不是同一概念。状态表分离时必须有一致快照或可检测版本屏障，无法做到时明确弱一致。不得使用外部数据库读到的当前质量覆盖更可信的新事件，除非绑定和策略明确。

## 6. 消息幂等：先确定语义，再选择字节编码

目前 P0 已定义的去重域为可信 tenant_id、source_id、source_epoch、message_id；本节给出 D00-C 候选设计：

| 项目 | 参加测量语义比较？ | 理由 |
|---|---|---|
| device_id、model_revision_id、binding_revision_id | 是 | 描述该观测属于谁及如何解释 |
| 每项 property、value、source_at_ms、source_time_origin、source_seq | 是 | 测量事实的组成 |
| quality.state / source_protocol / source_code | 是，字段规范与默认值需先冻结 | 不得把质量变化认作完全相同消息 |
| request_id、服务端 received_at_ms、重传的 replayed | 否 | 属于传输或接收信息，不改变采样事实 |
| 非法 / 未知字段 | 待决 | 不能一边丢弃字段一边宣称语义内容相同 |

需先经过认证 / 范围检查和整批结构、资源、版本、值域验证。对于已接受的合法重试，建议先按消息唯一域在并发安全状态中比较原规范化事实：相同返回原 ingest_id，不同冲突。绑定已经切换或源凭据撤销后的合法重复如何处理需明确：**服务身份与当前权限必须重新检查；既有重复确认不能作为绕过授权的入口**。既有原消息能否在新绑定激活后查询结果，应引用首次接受快照和可追溯记录，不能用新映射重新解释。

同 key 并发：只有一个首次接受者，其他请求等待 / 返回待处理或同一结果；必须设有崩溃可恢复的 pending 状态和超时策略。同 key、不同 hash 冲突不得以“后到覆盖”。WAL 同步、数据库应用和外部目标分别有状态；durable ACK 不代表历史已写入。

### 必须冻结的字节级规则

points 顺序是否重要；property 的 Unicode 标准与大小写；重复 JSON key 拒绝；null 与缺失的区别；字符串 / 整数上限；浮点 1 与 1.0、-0.0、超精度 / 科学计数、NaN / Infinity；source_seq 前导零；算法 / Schema 版本。需要精确的规范化算法和黄金**字节 / 哈希**向量；不能简单比较 JSON 文本或依赖某个语言序列化默认行为。

跨语言（实际网关实现语言和 Rust）生成同一语义字段的相同规范化字节与摘要后才能声称互操作。现在还未选定 JCS 等算法，不能把示例 hash 或候选算法写成最终协议。旧版本如何升级、去重保留期及 key 重用策略也必须评审。

## 7. 候选质量事件幂等与恢复

同一个 (tenant_id, source_id, source_epoch, event_id) 在 quality_event 域内代表同一事件；相同语义重放返回同一事件处理结果，内容变化返回冲突。事件与测量 key 域显式分离以防碰撞。若质量事件需要 durable，先写入受校验的 WAL 记录，再独立投影 quality_event / current_quality；应用去重标记与相关投影需有可验证一致性边界。事件写入成功但状态投影失败，应在恢复后重放且不生成重复事件；不能因事件投影失败就伪造新的压力值。

资源数量、消息体积、质量事件时间范围、event_id 保留期、非有限数、结构过深和有效期检查按正式配置执行；暂无实测参数，不在文中硬编码生产默认值。

## 8. 可执行验收向量

| 编号 | 条件 | 必须观察 |
|---|---|---|
| QE-01 | 从未采样的 pressure | value=null、quality=unknown、source_at=null |
| QE-02 | t1 测量 0.42，t2 来源 timeout | value 仍 0.42、采样时间 t1；quality / 连接时间分别记录 t2 |
| QE-03 | t2 timeout 重传同 event_id / 相同内容 | 同一个事件结果，不重复追加、不改变 last_value |
| QE-04 | 同 event_id 修改 state / reason | 冲突，原事件与当前投影不被覆盖 |
| QE-05 | timeout 事件到达时源是网关而非具体点 | 只按经确认映射更新，不能无条件声称每个传感器质量 bad |
| QE-06 | 旧采样晚到并伴随过期质量 | 历史策略可保存但较新 latest / quality 不倒退 |
| QE-07 | t3 合法新测量与质量事件并发 | 依冻结的排序规则得到确定结果，且不丢更新 |
| QE-08 | 状态事件 WAL 已确认后、投影提交前崩溃 | 事件恢复一次，数值历史不新增虚假采样 |
| ID-01 | 测量仅 replayed 不同 | 返回首次 ingest_id，不重复测量 |
| ID-02 | 相同 key 变更值 / quality / 采样时间 | 冲突，不覆盖 |
| ID-03 | 测量 ID 和质量事件 ID 文本相同 | 域隔离，各有独立记录 |
| ID-04 | 撤销服务范围后重放旧消息 | 拒绝未经授权访问；不得泄露历史请求细节 |
| ID-05 | 再次发送时绑定版本已经改变 | 重新授权，按首次接收记录和冻结规则返回；不得以新倍率解释旧事实 |
| ID-06 | 并发同 key / 同语义，进程中途失败 | 至多一个逻辑接收结果；恢复后可追溯，无误导性 durable ACK |
| ID-07 | points 换序、1 / 1.0、负零、缺失 / null、Unicode | 在规则冻结前均标待决，冻结后必须有跨语言黄金向量 |

各项需要记录输入、认证主体、初始状态、输出、WAL / 事务位置、历史 / latest 差异和重试结果。本节为测试设计，不代表通过。

## 9. D00-B/C 下一步

1. 收集网关能提供的状态 / 质量事件样本与时钟、序列和离线缓存说明；没有真实样本时只用虚构 fixture。
2. 确定单点 / 整设备状态粒度、时钟可信度、测量与事件排序及状态恢复规则。
3. 确定规范化算法和正反 JSON / 字节 / 摘要测试向量；之后再制作机器可读 Schema / API 规格。
4. 实测 WAL、DuckDB 同事务和崩溃恢复，报告版本 / 命令 / 结果；外部目标能力需独立实测。

未完成上述步骤前，本方案不得描述为正式冻结的标准或已验证的可靠方案。
