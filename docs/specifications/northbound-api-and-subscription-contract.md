# 北向标准接口、实时订阅与历史查询契约设计

状态：设计草案，尚未实现或完成联调。本文汇总并细化 [数据引擎定位与业务模型规范](../architecture/data-engine-scope.md)、[完整设计规范 20、24–25 节](../architecture/design-specification.md)、[设备业务模型详细设计](device-business-model.md) 与 [存储适配器契约第 8 节](storage-adapter-contract.md)，定义控制面与北向数据面的标准接口契约。所有接口均受 [服务身份与可信数据范围契约](service-identity-and-scope-contract.md) 约束。

## 1. 目标与接口分层

北向接口向 SCADA、组态、报表、MES/EMS 及上层 AI/MCP 适配层提供统一的模型目录、设备业务视图、实时状态订阅与历史查询能力。接口分为两类：

| 分层 | 路由前缀 | 调用方要求 (`caller_kind`) | 职责边界 |
|---|---|---|---|
| 控制面接口 | `/v1/products`、`/v1/models`、`/v1/assets`、`/v1/devices`（写）、`/v1/bindings`、`/v1/sources` | `management_service`（写操作强制写 `audit_log`） | 产品与物模型版本发布、资产树与 UNS 路径维护、设备实例化、来源授权与绑定激活 |
| 北向数据面接口 | `/v1/models`（读）、`/v1/resources`、`/v1/devices/{id}/view`、`/v1/devices/{id}/shadow`、`/v1/subscriptions`、`/v1/history/query` | `northbound_service`、`catalog_reader` | 资源发现、设备实时业务视图、实时影子、一致快照 + 增量订阅、受限历史查询 |

本引擎不提供面向最终用户的登录/角色管理接口，也不提供直接下发物理设备控制的执行接口。写操作支持 `Idempotency-Key` 请求头防止管理端超时重试产生重复资源。

## 2. 控制面：产品与物模型版本契约（D02）

### 2.1 核心端点

| 方法与路径 | 说明 | 关键约束 |
|---|---|---|
| `POST /v1/products` | 创建产品定义 | 同租户 `product_key` 唯一；停用不删除历史 |
| `POST /v1/products/{product_id}/models` | 创建物模型草稿（`state=draft`） | 指定 `model_version`（如 `"1.0.0"`）与模型定义 `body` |
| `POST /v1/models/{model_revision_id}:validate` | 校验草稿语法、类型、单位编码与属性唯一性 | 返回结构化校验报告及与上一已发布版本的兼容性分类 |
| `POST /v1/models/{model_revision_id}:publish` | 发布为不可变模型版本（`state=published`） | 同一数据库事务内固化 `model_body`、`content_hash` 并生成 `model_item` 索引；发布后拒绝任何原位修改 |
| `GET /v1/models/{model_revision_id}` | 读取指定模型版本详情 | 返回不可变模型快照、`content_hash` 与状态 |

### 2.2 版本兼容性判定规则

模型版本比较分为两类，影响设备模型升级策略：
- **向后兼容扩展（`compatible_extension`）**：仅新增可选属性（`source_kind` 可为 `unbound` 或新增采集点）或修改非语义显示描述；现有属性 `identifier`、`data_type`、`unit.code`、语义角色保持不变。
- **破坏性变更（`breaking_change`）**：删除已有属性、修改属性 `data_type`、修改 `unit.code`（如 `kPa` 改 `MPa`）或改变布尔/枚举状态含义。破坏性变更版本不得对已有绑定设备执行静默就地切换，必须新建或迁移绑定版本并按生效时间分段解释历史。

## 3. 控制面：资产树、设备实例化与 UNS 路径演进（D03）

### 3.1 设备实例化与资源生成 (`POST /v1/devices`)

创建设备实例时必须引用同一产品下 `state=published` 的 `model_revision_id` 与所属 `asset_id`。服务端在单一元数据事务中完成：
1. 创建 `device` 记录（分配稳定 `device_id`，初始 `revision = 1`）；
2. 遍历 `model_item` 中定义的每个能力项，创建对应的稳定 `resource` 记录（分配 `resource_id`）；
3. 根据所属资产链 `slug` 与 `device_key` 生成每个资源的当前 `resource_path`（`path_state = "active"`，`canonical_uri = "uns://{tenant_slug}/{asset_path}/{device_key}/properties/{identifier}"`）。

### 3.2 资产迁移与 UNS 路径重命名 (`POST /v1/devices/{device_id}:relocate` / `PATCH /v1/assets/{asset_id}`)

- 移动设备所属资产或修改资产 `slug` 时，**`asset_id`、`device_id`、`resource_id` 绝对不变**。
- 原 `resource_path` 记录在同一事务内设置 `valid_to_ms = now`、`redirect_until_ms = now + redirect_retention_ms`、`path_state = "redirecting"`，同时插入新的 `active` 路径行。
- 在 `redirect_until_ms` 到期前，任何新资产或新设备不得占用该历史 `canonical_uri`；同父节点下 `slug` 冲突或形成父子环路时返回 `409 REVISION_CONFLICT` / `422 VALIDATION_FAILED`。

## 4. 控制面：来源绑定与激活状态机（D04）

| 方法与路径 | 说明 | 状态机与安全要求 |
|---|---|---|
| `POST /v1/devices/{device_id}/bindings` | 创建绑定草稿（`state=draft`） | 关联设备当前 `model_revision_id` 与已注册 `source_id` |
| `POST /v1/bindings/{binding_revision_id}:validate` | 校验点位映射、协议地址、转换归属与单位一致性 | 通过后转入 `state=ready`；`normalization_owner="gateway"` 时禁止配置服务端二次倍率换算 |
| `POST /v1/bindings/{binding_revision_id}:activate` | 激活绑定快照 | 请求必须携带 `expected_device_revision` 及网关确认凭证 `gateway_ack`（含 `binding_revision_id` 与 `config_hash`）或显式声明人工停采窗口 `activation_mode="manual_stop_window"` |

激活成功后，同一设备旧 `active` 绑定自动转为 `superseded`（记录 `superseded_at_ms`），新绑定转为 `active`（记录 `activated_at_ms`）；P0 每台设备同一时刻最多一个 `active` 绑定。若网关与服务端版本对账未完成，设备绑定状态标记为 `binding_sync_pending`，拒绝不可证明采样时段的旧版本遥测。

## 5. 北向数据面：资源发现、设备业务视图与实时影子（D08）

### 5.1 资源解析与目录查询 (`GET /v1/resources`)

- **按 URI 精确解析**：`GET /v1/resources?uri=uns%3A%2F%2F8iic%2Fplant-a%2Froom-1%2Fpump-001%2Fproperties%2Foutlet_pressure`
  - 命中当前 `active` 路径：返回 `200` 及资源详情（`matched_path_state: "active"`）。
  - 命中保留期内的历史路径（且调用方对该 `resource_id` 有权）：返回 `200`，标注 `matched_path_state: "redirected"`，同时给出当前 `canonical_uri` 与稳定 `resource_id`。
- **按条件分页枚举**：`GET /v1/resources?asset_id=asset-plant-a&include_descendants=true&kind=property&limit=100&cursor=...`
  - 仅返回 `VerifiedScopeContext` 允许范围内的资源，稳定排序键为 `resource_id`。

### 5.2 设备实时业务视图 (`GET /v1/devices/{device_id}/view`)

组装设备元数据、已发布模型定义、绑定状态、各点最新测量值与当前质量状态，供组态画面、设备详情页与上层业务服务直接消费：

```json
{
  "request_id": "req-view-01",
  "device_id": "pump-001",
  "device_key": "pump-001",
  "device_name": "1号循环水泵",
  "product_id": "prod-pump",
  "product_key": "circulation-pump",
  "model_revision_id": "pump-model-r1",
  "model_version": "1.0.0",
  "asset_id": "asset-plant-a-room-1",
  "canonical_device_uri": "uns://8iic/plant-a/room-1/pump-001",
  "active_binding_revision_id": "pump-001-bind-r1",
  "source_connection_state": "connected",
  "view_generated_at_ms": 1793869205000,
  "view_consistency": "per_resource_latest",
  "properties": {
    "running_state": {
      "resource_id": "res-pump-001-running-state",
      "name": "运行状态",
      "data_type": "bool",
      "unit": null,
      "binding_state": "bound",
      "observation_state": "observed",
      "value": true,
      "quality": "good",
      "quality_reason": null,
      "freshness": "fresh",
      "value_sampled_at_ms": 1793869200120,
      "quality_observed_at_ms": 1793869200120,
      "received_at_ms": 1793869200150,
      "source_time_origin": "gateway_sample",
      "binding_revision_id": "pump-001-bind-r1"
    },
    "outlet_pressure": {
      "resource_id": "res-pump-001-outlet-pressure",
      "name": "出口压力",
      "data_type": "float64",
      "unit": {"code": "MPa", "symbol": "MPa"},
      "binding_state": "bound",
      "observation_state": "observed",
      "value": 0.42,
      "quality": "bad",
      "quality_reason": "modbus_read_timeout",
      "freshness": "stale",
      "value_sampled_at_ms": 1793869100120,
      "quality_observed_at_ms": 1793869200000,
      "received_at_ms": 1793869100145,
      "source_time_origin": "gateway_sample",
      "binding_revision_id": "pump-001-bind-r1"
    },
    "maintenance_note": {
      "resource_id": "res-pump-001-maint-note",
      "name": "维护备注",
      "data_type": "text",
      "unit": null,
      "binding_state": "unbound",
      "observation_state": "no_observation",
      "value": null,
      "quality": "unknown",
      "quality_reason": "unbound_property",
      "freshness": "unknown",
      "value_sampled_at_ms": null,
      "quality_observed_at_ms": null,
      "received_at_ms": null,
      "source_time_origin": null,
      "binding_revision_id": null
    }
  }
}
```

关键契约约束：
- `view_consistency = "per_resource_latest"` 明确声明各属性独立投影最新值，不隐含所有点位来自同一物理采样周期。
- 当点位发生通信超时（如上例 `outlet_pressure`）时，`value` 保留最后一次有效测量值 `0.42` 及其原始 `value_sampled_at_ms`（`1793869100120`），而 `quality` 变为 `"bad"`、`quality_observed_at_ms` 更新为故障观察时刻（`1793869200000`）。
- 已建模但未绑定的属性（`binding_state="unbound"`）或已绑定但尚未收到任何观测的属性（`observation_state="no_observation"`），`value` 必须为 `null`、`quality="unknown"`，**严禁填充 `0`、`false` 或空字符串**。

### 5.3 设备实时影子 (`GET /v1/devices/{device_id}/shadow`)

返回不含完整模型元数据描述的紧凑实时状态快照（含设备状态 `revision`、各点 `resource_id`、`value`、`value_sampled_at_ms`、`quality_observed_at_ms`、`quality`、`freshness`、`history_status`），用于高频轮询或轻量状态同步。

## 6. 北向数据面：WebSocket 实时订阅与快照衔接协议（D08 / F6）

端点：`WS /v1/subscriptions`

### 6.1 订阅状态机与边界序号 `S`

为保证客户端从“初始全量状态”切换到“实时增量流”时不漏掉切换瞬间的并发更新，服务端在状态更新的同一有序域（`stream_epoch` + 单调递增 `stream_seq`）内执行以下五步握手：

1. **客户端发送 `subscribe`**：指定 `client_sub_id` 与过滤条件（`device_ids`、`resource_ids` 或受控 `uri_prefixes`，必须全部位于当前 `VerifiedScopeContext` 范围内）。
2. **服务端注册订阅并锁定边界 `S`**：在实时状态内存/有序域原子记录当前序号 `S`，并将 `stream_seq > S` 的匹配更新放入该订阅的有界增量缓冲队列。
3. **服务端发送 `snapshot_begin` 与 `snapshot_chunk`**：输出与边界 `S` 一致的资源当前状态快照（若某资源在生成快照时已推进到 `seq_r <= S` 则直接体现；若在快照读取时已推进到 `seq_r > S`，客户端可通过后续 `delta` 幂等收敛到最新 `resource_revision`）。
4. **服务端发送 `snapshot_end`**：携带 `boundary_seq: S`，标志快照阶段完成。
5. **服务端发送 `delta` 流**：依次排空缓冲队列及后续新到达的 `stream_seq > S` 增量事件。

### 6.2 协议报文示例

**（1）客户端发起订阅**：

```json
{
  "type": "subscribe",
  "client_sub_id": "sub-client-01",
  "filter": {
    "device_ids": ["pump-001"],
    "resource_ids": [],
    "uri_prefixes": []
  },
  "options": {
    "coalesce_slow_consumer": true
  }
}
```

**（2）服务端快照结束帧**：

```json
{
  "type": "snapshot_end",
  "subscription_id": "sub-srv-9001",
  "client_sub_id": "sub-client-01",
  "stream_epoch": "srv-epoch-20261105-01",
  "boundary_seq": "4096"
}
```

**（3）服务端增量帧（区分测量更新与质量状态变更）**：

```json
{
  "type": "delta",
  "subscription_id": "sub-srv-9001",
  "stream_epoch": "srv-epoch-20261105-01",
  "stream_seq": "4097",
  "change_kind": "measurement_updated",
  "coalesced": false,
  "resource_id": "res-pump-001-outlet-pressure",
  "device_id": "pump-001",
  "property": "outlet_pressure",
  "model_revision_id": "pump-model-r1",
  "binding_revision_id": "pump-001-bind-r1",
  "resource_revision": 108,
  "value": 0.44,
  "unit": {"code": "MPa", "symbol": "MPa"},
  "quality": "good",
  "quality_reason": null,
  "freshness": "fresh",
  "value_sampled_at_ms": 1793869210120,
  "quality_observed_at_ms": 1793869210120
}
```

**（4）缓冲区溢出或无法维持一致边界时的 `resync_required`**：

```json
{
  "type": "resync_required",
  "subscription_id": "sub-srv-9001",
  "stream_epoch": "srv-epoch-20261105-01",
  "code": "BUFFER_OVERFLOW",
  "message": "Pending delta buffer exceeded capacity during snapshot or slow consumption; reconnect and fetch a new snapshot."
}
```

收到 `resync_required` 后，本轮订阅立即终止，客户端必须重新发起订阅拉取快照；严禁服务端静默丢弃增量帧后继续伪装连续流。

## 7. 北向数据面：受限历史查询契约（`POST /v1/history/query`）

### 7.1 请求结构

历史查询只接受结构化 JSON 查询（Typed AST），严禁传入任意 SQL 片段或物理表名：

```json
{
  "resource_ids": ["res-pump-001-outlet-pressure"],
  "range": {
    "start_ms": 1793865600000,
    "end_ms": 1793869200000
  },
  "mode": "raw",
  "aggregation": null,
  "quality_states": ["good", "uncertain", "bad"],
  "order": "asc",
  "limit": 500,
  "cursor": null
}
```

请求约束：
- `range` 严格为 UTC 左闭右开区间 `[start_ms, end_ms)`，且 `start_ms < end_ms`；最大时间跨度与单次 `resource_ids` 数量受配额（`quota_profile`）限制。
- `mode = "raw"` 时，`order` 按 `(source_at_ms, observation_id)` 升序或降序稳定排列。
- `mode = "aggregate"` 时，`aggregation` 指定 `{"bucket_ms": 60000, "functions": ["avg", "min", "max", "count"], "empty_bucket_policy": "omit"}`；若查询时间段内该资源跨越了单位或类型不兼容的 `model_revision_id`，返回 `422 CROSS_VERSION_INCOMPATIBLE`。

### 7.2 响应结构（含覆盖水位与已知缺口披露）

```json
{
  "request_id": "req-hist-01",
  "target_id": "local-duckdb",
  "source_mode": "managed_ingest",
  "mode": "raw",
  "consistency": "strong_snapshot",
  "partial": false,
  "warnings": [],
  "coverage": {
    "contiguous_watermark_ms": 1793869200000,
    "known_gaps": []
  },
  "items": [
    {
      "observation_id": "obs-6f4a9c2e8b1d037a5c8e9102ab",
      "resource_id": "res-pump-001-outlet-pressure",
      "device_id": "pump-001",
      "property": "outlet_pressure",
      "model_revision_id": "pump-model-r1",
      "binding_revision_id": "pump-001-bind-r1",
      "source_at_ms": 1793865601120,
      "received_at_ms": 1793865601150,
      "source_time_origin": "gateway_sample",
      "value_type": "float64",
      "value": 0.42,
      "unit": {"code": "MPa", "symbol": "MPa"},
      "quality": "good",
      "source_quality_code": null,
      "replayed": false,
      "record_kind": "measured"
    }
  ],
  "next_cursor": null
}
```

分页游标（`next_cursor`）经过服务端签名，内部绑定 `(tenant_id, scope_hash, query_fingerprint, snapshot_boundary, last_source_at_ms, last_observation_id)`；篡改游标或切换调用范围后重放游标返回 `422 VALIDATION_FAILED`。当存在 `delivery_gap` 或外部库部分不可达时，`partial` 设为 `true` 并在 `coverage.known_gaps` 中列出受影响时间区间与原因。

## 8. 面向只读目录与上层 MCP 的边界约定（D11）

上层平台封装只读 MCP Server 或 AI 检索服务时，使用 `caller_kind = "catalog_reader"` 调用本规范定义的 `/v1/resources`、`/v1/devices/{id}/view` 与 `/v1/history/query`：
1. **先鉴权后检索**：所有目录浏览与关键词搜索先按 `VerifiedScopeContext` 裁剪资源，未授权产线的设备名称、别名、描述一律不进入候选集。
2. **质量与时间强制伴随**：所有返回给 AI 适配层的当前值或历史值均携带 `unit`、`quality`、`freshness`、`value_sampled_at_ms`；当 `freshness = "stale"` 或 `quality != "good"` 时，响应显式保留状态，不返回裸数值。
3. **不可信文本声明**：设备名称、描述、维护备注、外部标签均作为纯数据字段返回，引擎在元数据规范中校验长度与控制字符，上层 AI 服务必须将其作为不可信数据隔离，不得拼接为系统指令。

## 9. 验收向量

- **NB01（已发布模型不可变）**：对 `state=published` 的 `model_revision_id` 发起修改或重复覆盖发布，返回 `409 REVISION_CONFLICT`。
- **NB02（资产迁址 ID 稳定与历史重定向）**：设备从车间 A 移至车间 B 后，`device_id` 与各属性 `resource_id` 不变；查询原 `canonical_uri` 返回 `matched_path_state="redirected"` 及新 URI，且保留期内其他设备无法注册原 URI。
- **NB03（未上报与未绑定零伪造）**：新建设备尚未上报数据时，`GET /v1/devices/{id}/view` 返回 `value: null, quality: "unknown", observation_state: "no_observation"`，不出现默认 `0` 或 `false`。
- **NB04（故障时间与测量时间分离）**：上报 `0.42 MPa` 后发生网关超时，`/view` 与 `/shadow` 中 `value` 仍为 `0.42`、`value_sampled_at_ms` 不变，而 `quality="bad"`、`quality_observed_at_ms` 更新为超时时间。
- **NB05（订阅边界 `S` 无缝衔接）**：在持续高频写入期间建立 WebSocket 订阅，合并快照与 `stream_seq > S` 的 `delta` 后，客户端最终状态与服务端最新 `resource_revision` 完全一致，无漏点。
- **NB06（慢消费溢出强制 `resync_required`）**：人为阻塞订阅客户端读取使服务端增量缓冲满，服务端返回 `resync_required` 并断开该订阅，不静默跳帧。
- **NB07（历史半开区间与同毫秒稳定分页）**：`[1000, 2000)` 包含 `source_at_ms=1000` 的记录、不包含 `2000`；同一毫秒内两条不同 `observation_id` 的记录跨页翻页时不重不漏。
- **NB08（跨版本单位不兼容拒绝聚合）**：某属性在版本 r1 单位为 `kPa`、版本 r2 单位为 `MPa`，跨越切换时刻执行 `mode="aggregate"` 返回 `422 CROSS_VERSION_INCOMPATIBLE`。
- **NB09（游标跨 Scope 防重放）**：用服务 A（产线 1 范围）获得的 `next_cursor` 在服务 B（产线 2 范围）下继续翻页，返回 `422 VALIDATION_FAILED`。
- **NB10（已知缺口显式披露）**：某时间段存在经授权隔离的 `delivery_gap`，历史查询覆盖该时段时返回 `partial: true` 及 `coverage.known_gaps` 明细。
