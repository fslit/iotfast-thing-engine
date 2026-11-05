# 服务身份、可信数据范围与 D00 公共契约设计

状态：设计草案，尚未实现或完成安全实测。本文细化 [数据引擎定位与业务模型规范](../architecture/data-engine-scope.md)、[完整设计规范 18–25 节](../architecture/design-specification.md) 与 [P0 遥测入站契约](p0-telemetry-contract.md)，作为 D00 契约冻结阶段关于服务边界、确定性标识、幂等摘要、状态消息分离和统一错误码的基准草案。如与总体设计第二部分存在冲突，先登记并解决后再实施。

## 1. 目标与职责边界

本引擎不建设最终用户账号、角色（RBAC）、菜单按钮权限、业务审批流或设备控制联锁；但**上层负责用户权限不等于引擎免鉴权**。引擎在部署边界上遵循以下铁律：

1. **只接受经认证的服务调用**：所有北向查询、模型/绑定配置与南向网关上报必须携带可验证的服务身份凭证（如 mTLS 客户端身份或由可信网关签发的短期签名令牌；具体传输机制按部署环境锁定并实测）。
2. **数据范围（Scope）强制执行且默认拒绝（fail-closed）**：每次调用必须解析出服务端验证过的 `tenant_id` 与可访问的数据范围；缺失身份、签名无效、令牌过期、租户不匹配或范围为空时一律拒绝，不得回退为“默认全租户可见”。
3. **禁止信任客户端自报上下文字段**：请求体、查询参数或普通未签名 HTTP 头中的 `tenant_id`、`user_id`、`X-Role`、`caller_id` 不能单独作为授权依据；请求体若包含 `tenant_id` 或 `source_id`，仅用于与已认证上下文做一致性核对，不一致即拒绝。
4. **操作人上下文仅作审计追溯**：上层平台可透传已鉴权的操作人标识（`operator_id`、`operator_session_ref`）写入 `audit_log`，但引擎不据此计算业务权限。

## 2. 可信调用方分类与范围断言（Scope Context）

引擎将认证后的调用主体归一化为不可伪造的内部 `VerifiedScopeContext`。按职责分为四类调用方：

| 调用方类别 (`caller_kind`) | 典型来源 | 允许的动作集合 (`allowed_actions`) | 范围约束粒度 |
|---|---|---|---|
| `southbound_source` | 采集网关、协议桥接器 | `telemetry:ingest`、`source_state:report`、`binding:ack` | 绑定的 `source_id` 及 `source_device_grant` 授权的 `device_id` 列表 |
| `northbound_service` | SCADA、组态、报表、MES/EMS 后端 | `catalog:read`、`view:read`、`shadow:read`、`subscription:connect`、`history:query` | 指定 `tenant_id` 下的 `asset_ids`（含可选子树）、`device_ids`、`resource_ids` 或 `uri_prefixes` |
| `catalog_reader` | 上层只读 MCP 适配服务、AI 检索网关 | `catalog:read`、`view:read`、`history:query`（受配额限制） | 显式收窄的只读 `asset_ids` / `device_ids` / `resource_ids`，禁止控制面写操作 |
| `management_service` | 上层模型与设备管理后台服务 | `model:write`、`asset:write`、`device:write`、`binding:write`、`source:write` | 指定 `tenant_id` 及受控管理域，强制记录 `audit_log` |

### 2.1 Scope Context 逻辑结构示例

以下 JSON 表示服务端验证通过后在引擎内部流转的逻辑断言结构，不代表允许客户端明文直传：

```json
{
  "spec": "iotfast.scope_context.v1",
  "tenant_id": "ten-8iic",
  "project_id": "proj-smt-01",
  "caller_service_id": "svc-scada-api-01",
  "caller_kind": "northbound_service",
  "issued_at_ms": 1793869200000,
  "expires_at_ms": 1793869500000,
  "allowed_actions": [
    "catalog:read",
    "view:read",
    "shadow:read",
    "subscription:connect",
    "history:query"
  ],
  "scope_filter": {
    "asset_ids": ["asset-plant-a-room-1"],
    "include_asset_descendants": true,
    "device_ids": [],
    "resource_ids": [],
    "uri_prefixes": ["uns://8iic/plant-a/room-1/"]
  },
  "quota_profile": "standard_northbound",
  "operator_audit": {
    "operator_id": "op-1024",
    "client_ip": "10.20.1.15",
    "upstream_request_id": "up-req-9981"
  }
}
```

### 2.2 范围求值规则

1. **租户硬隔离**：首先匹配 `resource.tenant_id == context.tenant_id`；跨租户访问在任何接口下均不可见。
2. **动作白名单**：当前 API 所需动作必须包含在 `allowed_actions` 中，否则返回 `403 FORBIDDEN_ACTION`。
3. **资源交集/并集判定**：
   - 对 `southbound_source`：仅允许操作 `context.source_id == payload.source_id` 且在当前有效 `source_device_grant` 内的 `device_id`。
   - 对北向与管理类调用：`scope_filter` 中非空维度（`asset_ids`、`device_ids`、`resource_ids`、`uri_prefixes`）按明确的并集语义组成可访问资源集合，再与租户和动作范围求交；若所有范围维度均为空且未显式声明租户级全量标志 `tenant_wide: true`，按 **空范围（拒绝一切资源）** 处理。
   - `uri_prefixes` 仅按规范化后的路径段边界匹配（例如 `uns://8iic/plant-a/` 匹配 `uns://8iic/plant-a/pump-001`，绝不匹配 `uns://8iic/plant-ab/pump-001`）。

## 3. 范围过滤、隐匿策略与长连接生命周期

| 访问方式 | 越权或超范围行为 | 设计理由 |
|---|---|---|
| 按 ID 单点读取 (`GET /v1/devices/{id}`、`/view`、`/shadow`、`GET /v1/resources/{id}`) | 统一返回 `404 RESOURCE_NOT_FOUND`（或部署统一配置的隐匿响应），不区分“ID 不存在”与“存在但无范围权限” | 防止调用方通过枚举 ID 探测其他产线或租户的设备存在性 |
| 批量读取 / 历史查询 (`POST /v1/history/query`、批量 latest) | 若请求显式指定了不在授权范围内的 `resource_id`，整次请求返回 `404 RESOURCE_NOT_FOUND`（不得区分或回显越权与不存在的 ID），不静默丢弃部分 ID 后返回看似完整的成功结果 | 避免上层报表或算法因静默漏点算出错误汇总 |
| 目录浏览 / 搜索 (`GET /v1/resources`、`browse_resources`、`search_resources`) | 先在存储/索引层应用 `VerifiedScopeContext` 过滤，再做关键词匹配、排序与分页；不返回范围外节点计数 | 满足总体设计 25.1 节“权限过滤必须先于检索与排序” |
| 南向遥测 / 状态上报 (`POST /v1/telemetry`、`POST /v1/source-state`) | `source_id` 或 `device_id` 未授权时返回 `403 SOURCE_NOT_AUTHORIZED`，整消息拒绝 | 明确向网关暴露配置或授权异常 |
| 缓存与游标 (`cursor`) | 所有读缓存 key 与分页 `cursor` 签名必须绑定 `(tenant_id, scope_hash)` | 防止不同范围的请求复用缓存或交换游标越权翻页 |

**WebSocket 订阅长连接边界**：
- 连接建立时验证 `VerifiedScopeContext`；订阅过滤器中的所有目标资源或前缀必须是当前 `scope_filter` 的子集。
- 当会话凭证到达 `expires_at_ms` 且未在宽限期内完成续期，或服务端收到范围撤销通知时，服务端主动发送 `subscription_terminated`（`reason: "credential_expired"` 或 `"scope_revoked"`）并关闭订阅流，禁止过期会话无限期接收推送。

## 4. D00 公共身份、幂等摘要与确定性生成规则

为解决 `p0-telemetry-contract.md` 与 `storage-adapter-contract.md`（ST-01）中的待冻结事项，P0 对核心标识与摘要作如下规范化约定：

### 4.1 关键标识格式与确定性规则

| 标识 | 格式约束 | 生成与稳定性规则 |
|---|---|---|
| `tenant_id` / `device_id` / `resource_id` / `source_id` | ASCII `[a-z0-9][a-z0-9_-]{1,62}[a-z0-9]`，长度 3–64，区分大小写（统一小写） | 服务端创建时分配，生命周期内不可变，停用后不复用 |
| `message_id` | 可打印 ASCII（`0x21..0x7E`），长度 1–128 | 网关/来源生成；同一重试周期内固定 |
| `source_epoch` | ASCII `[A-Za-z0-9_.:-]{1,64}` | 来源序列纪元；同次重试不变，重启若无法恢复序列则必须更换新 epoch |
| `message_key` | 四元组 `(tenant_id, source_id, source_epoch, message_id)` | 入站鉴权后确定，作为幂等去重唯一键 |
| `observation_id` | `"obs-" + Base32LowerNoPad(SHA-256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])` | 由已通过校验的 `message_key` 与点位 `property` 确定性派生（首次入 WAL 前确定性派生并固化入 WAL）；**严禁在重放或跨目标投递时重新生成不同 ID** |
| `wal_position` | 固定宽度可比较格式 `"{segment_no:016x}:{record_offset:016x}"` | 按 `(segment_no, record_offset)` 无符号 64 位整数顺序比较，不按任意可变长字符串比较 |

说明：采用基于 `(message_key, property)` 的确定性 `observation_id` 派生（辅以 WAL 固化记录）可确保网关超时重试、WAL 崩溃重放以及多时序库投递对同一测量点始终得出完全一致的 `observation_id`。

### 4.2 遥测业务摘要 (`payload_hash`) Canonicalization 规则

服务端计算 `payload_hash` 用于判定同 `message_key` 的重试是“合法重复（`200 duplicate`）”还是“内容篡改冲突（`409 IDEMPOTENCY_CONFLICT`）”。

1. **算法标识**：`sha256-jcs-v1`（基于 RFC 8785 JSON Canonicalization Scheme 的受限子集，UTF-8 编码后计算 SHA-256，输出 `sha256:<64位小写hex>`）。
2. **纳入摘要的规范化结构**：
   - 顶层固定包含：`spec`、`tenant_id`（来自认证上下文）、`source_id`、`source_epoch`、`device_id`、`model_revision_id`、`binding_revision_id`、`points`。
   - `points` 数组先按 `property` 字节序升序排列（因单消息内 `property` 唯一，排序结果唯一确定）。
   - 每个点纳入摘要的字段：`property`、`value_type`（由模型属性定义确定）、`value`、`source_at_ms`、`source_time_origin`、`source_seq`、`quality.state`、`quality.source_protocol`（缺失记为 `null`）、`quality.source_code`（缺失记为 `null`）。
3. **显式排除在摘要之外的字段**（重传时允许变化）：
   - `points[].delivery.replayed`（首次发送为 `false`、超时重传变为 `true` 时，摘要保持不变）；
   - HTTP 头中的 `X-Request-Id`、`Prefer`、认证令牌；
   - 服务端生成的 `received_at_ms`、`ingest_id`、`wal_position`。
4. **数值与类型规范化**：
   - `bool` 为字面量 `true` / `false`；`int64` 为不带前导零的十进制整数字符串；
   - `float64` 拒绝 `NaN` / `±Infinity`；`-0.0` 规范化为 `0`；按照 IEEE 754 双精度遵循 RFC 8785 最短精确十进制表示；同一协议版本固定唯一实现并附跨语言测试向量。

## 5. 时间来源词表、序列号与时钟漂移规则

### 5.1 `source_time_origin` 枚举词表（D00 冻结）

| 取值 | 含义 | 允许入站条件 |
|---|---|---|
| `device_clock` | 现场设备/仪表自身时钟产生的原始采样时间戳 | 绑定配置声明协议报文含设备时钟 |
| `protocol_timestamp` | 现场总线/工业协议栈（如 OPC UA Server Timestamp）附带的时间戳 | 绑定配置声明协议提供时间戳 |
| `gateway_sample` | 设备无时钟，由采集网关在完成该轮点位读取时打上的采样时间戳 | P0 默认常用模式；不得在展示层标为“设备内部时钟” |
| `external_source` | 外部时序库（`referenced_read`）或上层业务事实自带的原始记录时间 | 仅限外部引用或受控导入路径 |

### 5.2 `source_seq` 与时钟漂移处理

- **`source_seq` 约束**：必填，十进制数字字符串，匹配正则 `^(0|[1-9][0-9]{0,19})$`，取值范围 `0 ..= 18446744073709551615`（`u64::MAX`）。比较先后顺序时必须解析为无符号 64 位整数，严禁按字符串字典序比较（例如 `"1000"` 必须大于 `"999"`）。
- **时间范围与超前漂移**：
  - `source_at_ms` 必须为正整数（UTC Unix 毫秒）。
  - 设服务端接收时间为 `received_at_ms`，最大允许超前漂移阈值为 `max_future_skew_ms`（项目配置，默认示例 `60000` 即 60 秒）：
    - 若 `source_at_ms <= received_at_ms + max_future_skew_ms`：时间有效，正常参与 latest 顺序比较与历史落库。
    - 若 `source_at_ms > received_at_ms + max_future_skew_ms`：判定为时钟超前异常（`clock_status = "future_skew_exceeded"`）；按项目配置的严格级别拒绝整条消息（`422 CLOCK_SKEW_EXCEEDED`）或隔离记录，**严禁将其写入 `latest_value`**（否则一个未来时间戳将导致后续所有正常实时测量因时间较小而无法刷新 latest）。

## 6. 数值测量消息与连接/质量状态事件分离契约

总体设计第 22.1 节与测试向量 `T06` 规定：**“连接超时、无新测量值时，有具体点位错误证据时点位质量转 bad，`last_known_value` 保留旧采样时间，不伪造新测量”**。为在 wire schema 层彻底消除歧义，D00 明确将**数值测量上报**与**来源/点位状态事件上报**分离：

1. **数值测量消息（`POST /v1/telemetry`，`spec = "iotfast.telemetry.v1"`）**：
   - 每个 `points[]` 必须携带符合模型类型的非空工程值 `value`（`value: null` 在测量消息中一律返回 `422 INVALID_POINT_VALUE`）。
   - 允许上报带实际工程值的 `good` 或 `uncertain` 测量（例如传感器仍在输出读数但校准存疑时，`value: 245.6, quality.state: "uncertain"`）。
2. **连接与质量状态事件消息（`POST /v1/source-state`，`spec = "iotfast.source_state.v1"`）**：
   - 当网关发生 Modbus 读超时、设备断连、协议异常或恢复连接，且**没有产生新的工程测量值**时，通过状态事件上报。
   - 服务端处理逻辑：根据事件粒度和可信证据更新连接状态；仅有具体点位错误证据时才更新其 `point_quality_state` 和追加 `quality_event`；**不向 `history_observation` 插入虚假数值行，也不覆盖 `latest_value` 中的 `value` 与 `value_sampled_at_ms`**。

### 状态事件上报示例 (`POST /v1/source-state`)

```json
{
  "spec": "iotfast.source_state.v1",
  "event_id": "evt-20261105-000089",
  "source_id": "gateway-01",
  "source_epoch": "epoch-001",
  "device_id": "dev-oven-02",
  "binding_revision_id": "br-oven-12",
  "observed_at_ms": 1793869260000,
  "connection_state": "disconnected",
  "point_states": [
    {
      "property": "zone1_temp",
      "quality_state": "bad",
      "quality_reason": "modbus_read_timeout",
      "source_protocol": "modbus",
      "source_code": "ETIMEDOUT"
    }
  ]
}
```

## 7. 统一错误结构、错误码字典与单位约定

### 7.1 标准错误响应信封

所有 4xx / 5xx 响应均返回统一 JSON 结构，不包含内部堆栈、数据库文件路径或连接凭据：

```json
{
  "request_id": "req-001",
  "code": "VALIDATION_FAILED",
  "message": "Telemetry message rejected due to invalid point property",
  "retryable": false,
  "retry_after_ms": null,
  "details": [
    {
      "field": "points[1].property",
      "issue": "UNKNOWN_PROPERTY",
      "description": "Property 'zone9_temp' is not defined in model_revision 'mr-oven-1'"
    }
  ]
}
```

### 7.2 核心错误码字典

| 错误码 (`code`) | HTTP 状态码 | `retryable` | 触发条件 |
|---|---|---|---|
| `UNAUTHENTICATED` | 401 | `false` | 缺失服务凭证、签名无效或凭证已过期 |
| `FORBIDDEN_ACTION` | 403 | `false` | 调用方 `caller_kind` 或 `allowed_actions` 不包含当前操作 |
| `SOURCE_NOT_AUTHORIZED` | 403 | `false` | 网关 `source_id` 未获授权操作目标 `device_id` |
| `RESOURCE_NOT_FOUND` | 404 | `false` | 目标资源/设备不存在，或不在调用方 `VerifiedScopeContext` 范围内 |
| `IDEMPOTENCY_CONFLICT` | 409 | `false` | 相同 `message_key` 或操作幂等键但业务摘要 `payload_hash` 不同 |
| `REVISION_CONFLICT` | 409 | `false` | 乐观并发版本号不匹配，或设备上报的 `model_revision_id` / `binding_revision_id` 与当前激活快照不一致且无法证明属合法旧时段 |
| `VALIDATION_FAILED` | 422 | `false` | 字段缺失、类型不匹配、未定义属性、单消息重复属性、`value: null` 或未知 `spec` |
| `CLOCK_SKEW_EXCEEDED` | 422 | `false` | `source_at_ms` 超出最大允许未来漂移阈值 |
| `CROSS_VERSION_INCOMPATIBLE` | 422 | `false` | 历史聚合跨越了单位或类型不兼容的模型版本 |
| `CAPABILITY_UNSUPPORTED` | 422 / 501 | `false` | 请求的查询/聚合/插值超出目标存储后端已验证的 `capability_snapshot` |
| `RATE_LIMITED` | 429 | `true` | 超出调用方或租户的请求配额，伴随 `retry_after_ms` |
| `DURABILITY_BACKPRESSURE` | 503 | `true` | 客户端要求 `durable`，但应用层 WAL 达到硬水位或磁盘同步不可用 |
| `SOURCE_UNAVAILABLE` | 503 | `true` | 外部时序库（`referenced_read`）连接超时或不可达 |

### 7.3 工程单位编码约定

模型属性与绑定中的单位统一包含机器可读 `code`（优先采用区分大小写的 UCUM 编码）与展示符号 `symbol`。引擎判断跨绑定或跨模型版本单位是否一致时，**严格比较规范化 `code`**，不按展示字符模糊匹配：

| 物理量 | `code` (UCUM) | `symbol` | 说明 |
|---|---|---|---|
| 摄氏温度 | `Cel` | `°C` | 区分于开尔文 `K` |
| 压力（兆帕 / 千帕） | `MPa` / `kPa` | `MPa` / `kPa` | 不同数量级视为不同单位编码 |
| 电流 / 电压 | `A` / `V` | `A` / `V` | 安培 / 伏特 |
| 有功功率 | `kW` | `kW` | 千瓦 |
| 累计电能 | `kW.h`（兼容别名 `kWh`，发布时规范化为 `kW.h`） | `kWh` | 千瓦时 |
| 无量纲比例 / 开关状态 | `1` / `null` | `%` 或无 | 布尔开关量 `unit` 为 `null`；比率 `0..1` 使用 `1` |

## 8. 验收向量

- **SC01（缺失或伪造 Scope 拒绝）**：客户端不带可信服务凭证，仅在 Header 传 `X-Tenant-Id: ten-8iic`、`X-Role: admin`，所有接口返回 `401 UNAUTHENTICATED`。
- **SC02（单点与批量越权隐匿）**：仅授权 `asset-line-01` 的服务读取 `asset-line-02` 下的设备视图或将其加入 `POST /v1/history/query`，返回 `404 RESOURCE_NOT_FOUND`，不返回数据也不泄露设备配置。
- **SC03（URI 前缀边界安全）**：授权前缀为 `uns://8iic/plant-a/` 的调用方访问 `uns://8iic/plant-ab/pump-01`，被判定为越权拒绝。
- **SC04（确定性 `observation_id` 与重放一致）**：同一条消息重试（`replayed` 从 `false` 变为 `true`），计算出的 `payload_hash` 与各点 `observation_id` 完全不变，返回 `200 duplicate`。
- **SC05（同 key 篡改点位值冲突）**：相同 `(tenant_id, source_id, source_epoch, message_id)` 将温度从 `245.6` 改为 `245.7`，摘要变化，返回 `409 IDEMPOTENCY_CONFLICT`。
- **SC06（`source_seq` 数值序比较）**：同一毫秒内收到 `source_seq="999"`（值 24.0）与 `source_seq="1000"`（值 25.0），`latest_value` 正确保留 `"1000"` 对应的 25.0。
- **SC07（未来时钟漂移拦截）**：网关上报 `source_at_ms` 超出服务端当前时间 2 小时，消息被拒绝或隔离，不污染 `latest_value`。
- **SC08（状态事件不伪造测量）**：上报 `iotfast.source_state.v1` 超时事件后，设备视图中 `quality="bad"`，但 `last_known_value` 及其 `value_sampled_at_ms` 保持原值，历史测量表不新增记录。
- **SC09（测量消息禁止 `null` 值）**：向 `POST /v1/telemetry` 发送 `value: null` 的点位，整消息返回 `422 VALIDATION_FAILED`。
- **SC10（长连接凭证过期断连）**：WebSocket 订阅会话超过 `expires_at_ms` 未续期，服务端发送 `subscription_terminated` 并停止推送后续增量。
