# IoTFast 物模型与 UNS 数据管理引擎：对外与内部 API 及 URL 清单

状态：2026-11-10 已同步至 [`docs/specifications/apifox-openapi-bundle.json`](apifox-openapi-bundle.json) 与 [`docs/specifications/openapi-v1.json`](openapi-v1.json)。

## 1. 服务端基础 URL 与运行环境

| 环境名称 | 基础 URL | 对应守护进程 | 用途说明 |
| :--- | :--- | :--- | :--- |
| **标准完整引擎服务（默认）** | `http://127.0.0.1:8080` | `iotfast_server` | 对外北向数据面、物模型/资产控制面、南向遥测与状态入站全量 API |
| **北向 WebSocket 实时订阅** | `ws://127.0.0.1:8080/v1/subscriptions` | `iotfast_server` | RFC 6455 实时快照衔接（`snapshot_begin/chunk/end`）与增量推送（`delta`） |
| **端到端演示环境** | `http://127.0.0.1:18090` | `iotfast_server` | `python3 scripts/mvp-thing-model-demo.py` 一键演示默认端口 |
| **内部最小只读开发 Profile** | `http://127.0.0.1:18080` | `dev_server` | 内部最小烟雾测试（仅开放 `/health`、`/v1/telemetry` volatile、`/v1/devices/{id}/view`、`/v1/history/query`） |

---

## 2. 对外 API（External Northbound & Control Plane）

| 分类目录 | HTTP 方法 | URL 路径 | 所需 Scope Action | 功能说明 |
| :--- | :---: | :--- | :--- | :--- |
| `对外API-北向数据面/01-设备业务视图与实时订阅` | `GET` | `/v1/devices/{device_id}/view` | `view:read` | 获取设备完整物模型业务视图 (DeviceBusinessView) |
| `对外API-北向数据面/01-设备业务视图与实时订阅` | `GET` | `/v1/devices/{device_id}/shadow` | `shadow:read` | 获取设备实时影子视图 (DeviceShadowView) |
| `对外API-北向数据面/01-设备业务视图与实时订阅` | `GET` | `/v1/subscriptions` | `subscription:connect` | WebSocket 实时快照与增量订阅 (WS /v1/subscriptions) |
| `对外API-北向数据面/02-UNS资源目录与历史查询` | `GET` | `/v1/resources` | `view:read` | 查询 UNS 稳定资源目录或按资产子树枚举 |
| `对外API-北向数据面/02-UNS资源目录与历史查询` | `GET` | `/v1/resources:resolve` | `view:read` | 按 UNS URI 精确解析稳定 resource_id (含重定向状态) |
| `对外API-北向数据面/02-UNS资源目录与历史查询` | `POST` | `/v1/history/query` | `history:query` | 查询设备属性历史观测与聚合分桶 (支持水位线稳定游标) |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/products` | `model:write` | 创建产品定义 (Product) |
| `对外API-物模型控制面/03-产品与物模型管理` | `GET` | `/v1/products` | `model:read` | 列出当前租户下的产品目录 |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/products/{product_id}/models` | `model:write` | 在指定产品下创建物模型版本草稿 |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/models` | `model:write` | 创建物模型版本草稿 (ModelRevision Draft) |
| `对外API-物模型控制面/03-产品与物模型管理` | `GET` | `/v1/models` | `model:read` | 列出物模型版本摘要列表 |
| `对外API-物模型控制面/03-产品与物模型管理` | `GET` | `/v1/models/{model_revision_id}` | `model:read` | 获取指定物模型版本完整属性定义 |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/models/{model_revision_id}:validate` | `model:write` | 校验物模型草稿属性、标量类型与工程单位 |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/models/{model_revision_id}:publish` | `model:write` | 发布并冻结不可变物模型版本 (Published) |
| `对外API-物模型控制面/03-产品与物模型管理` | `GET` | `/v1/models/{model_revision_id}:diff` | `model:read` | 对比两个物模型版本的兼容性差异 (GET :diff) |
| `对外API-物模型控制面/03-产品与物模型管理` | `POST` | `/v1/models:compare` | `model:read` | 对比两个物模型版本的兼容性差异 (POST :compare) |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `POST` | `/v1/assets` | `asset:write` | 创建 UNS 层级资产树节点 (自动计算 uns_path) |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `GET` | `/v1/assets` | `asset:read` | 查询 UNS 层级资产树节点列表 |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `POST` | `/v1/assets/relations` | `asset:write` | 创建跨树逻辑拓扑关系 (FEEDS / BACKUP_FOR / MONITORS) |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `GET` | `/v1/assets/{asset_id}/relations` | `asset:read` | 查询指定资产关联拓扑边列表 |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `POST` | `/v1/asset-relations` | `asset:write` | 创建跨树逻辑拓扑关系 (FEEDS / BACKUP_FOR / MONITORS) |
| `对外API-物模型控制面/04-资产树与跨树拓扑关系` | `GET` | `/v1/asset-relations` | `asset:read` | 查询资产关联拓扑边列表 |
| `对外API-物模型控制面/05-设备实例化与生命周期` | `POST` | `/v1/devices` | `device:write` | 基于已发布物模型实例化设备并分配稳定 resource_id |
| `对外API-物模型控制面/05-设备实例化与生命周期` | `GET` | `/v1/devices` | `device:read` | 查询当前租户下的设备实例列表 |
| `对外API-物模型控制面/05-设备实例化与生命周期` | `GET` | `/v1/devices/{device_id}` | `device:read` | 获取指定设备实例详情与绑定状态 |
| `对外API-物模型控制面/05-设备实例化与生命周期` | `POST` | `/v1/devices/{device_id}:relocate` | `device:write` | 设备跨资产节点迁址 (保持 resource_id 不变，保护历史 URI) |
| `对外API-物模型控制面/05-设备实例化与生命周期` | `POST` | `/v1/devices/{device_id}:upgrade-model` | `device:write` | 设备物模型版本平滑升级 (保留既有 resource_id，拒绝破坏性变更) |
| `对外API-物模型控制面/08-边缘适配器能力与转换` | `POST` | `/v1/adapters` | `binding:write` | 注册或更新边缘协议适配器能力与转换模型 (EdgeAdapterCapability) |
| `对外API-物模型控制面/08-边缘适配器能力与转换` | `GET` | `/v1/adapters` | `binding:read` | 列出当前租户已注册的边缘协议适配器能力目录 |
| `对外API-物模型控制面/08-边缘适配器能力与转换` | `POST` | `/v1/adapters/{adapter_id}:validate-binding` | `binding:write` | 校验属性数据引用与指定边缘适配器能力兼容性 (ValidateAdapterBindingRequest -> ValidateAdapterBindingResponse) |
| `对外API-物模型控制面/09-派生指标与流式计算` | `POST` | `/v1/derived-metrics` | `model:write` | 创建或更新物模型确定性派生指标定义 (DerivedMetricDefinition) |
| `对外API-物模型控制面/09-派生指标与流式计算` | `GET` | `/v1/derived-metrics` | `model:read` | 列出当前租户下的物模型派生指标定义目录 |
| `对外API-物模型控制面/09-派生指标与流式计算` | `POST` | `/v1/derived-metrics/{metric_id}:evaluate` | `history:query / view:read` | 基于设备实时业务视图与质量守卫评估派生指标并输出输入血缘 (EvaluateDerivedMetricRequest -> EvaluateDerivedMetricResponse) |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `GET` | `/v1/storage/status` | `authenticated scope token` | 查看 SQLite 业务存储与可插拔时序存储适配器（DuckDB / TDengine 等）状态 (StorageArchitectureStatus) |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `POST` | `/v1/storage/timeseries-adapter` | `authenticated scope token` | 动态切换或配置设备时序数据存储适配器（支持 duckdb、tdengine、timescaledb、clickhouse；一期默认启用 SQLite + DuckDB + AppWal；外部 TDengine (taosd) 集群实连验收（ST-06）规划在二期启用） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `POST` | `/v1/storage/query-plan` | `authenticated scope token` | 预览当前时序存储适配器（DuckDB / TDengine 等）生成的原生历史/聚合查询 SQL (BuildQueryPlanRequest -> TimeSeriesQueryPlan) |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `GET` | `/v1/storage/capabilities` | `authenticated scope token` | 查询各时序存储后端能力快照（ST-02） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `GET` | `/v1/storage/checkpoints` | `authenticated scope token` | 查询时序投递连续检查点与未决空洞（ST-04） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `POST` | `/v1/storage/checkpoints:reconcile` | `binding:write / authenticated scope token` | 对账或隔离未决投递空洞并推进连续检查点（ST-04） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `POST` | `/v1/storage/backups` | `authenticated scope token` | 创建 SQLite + DuckDB + WAL 一致性快照备份（D10.1） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `GET` | `/v1/storage/backups` | `authenticated scope token` | 查询一致性快照备份列表（D10.1） |
| `对外API-物模型控制面/09b-存储架构与时序适配器` | `POST` | `/v1/storage/backups/{backup_id}:verify` | `authenticated scope token` | 在隔离沙箱中校验快照备份 SHA-256 与恢复完整性（D10.1） |

---

## 3. 内部与南向 API（Internal Southbound & Ops）

| 分类目录 | HTTP 方法 | URL 路径 | 所需 Scope Action | 功能说明 |
| :--- | :---: | :--- | :--- | :--- |
| `内部与南向API/06-属性数据引用与来源授权` | `POST` | `/v1/sources/grants` | `binding:write` | 授予南向网关上报指定设备的半开时间窗权限 (source_device_grant) |
| `内部与南向API/06-属性数据引用与来源授权` | `POST` | `/v1/devices/{device_id}/bindings` | `binding:write` | 为设备创建属性数据引用草稿 (动态采集 points + 静态配置 static_points) |
| `内部与南向API/06-属性数据引用与来源授权` | `POST` | `/v1/bindings` | `binding:write` | 创建属性数据引用草稿 (通用入口) |
| `内部与南向API/06-属性数据引用与来源授权` | `GET` | `/v1/bindings/{binding_revision_id}` | `binding:read` | 查询指定属性数据引用版本快照 |
| `内部与南向API/06-属性数据引用与来源授权` | `POST` | `/v1/bindings/{binding_revision_id}:validate` | `binding:write` | 校验属性数据引用草稿与物模型类型/单位一致性 |
| `内部与南向API/06-属性数据引用与来源授权` | `POST` | `/v1/bindings/{binding_revision_id}:activate` | `binding:write` | 激活数据引用版本 (凭 gateway_ack 或 manual_stop_window) |
| `内部与南向API/07-南向遥测与状态入站` | `POST` | `/v1/telemetry` | `telemetry:ingest` | 南向网关上报工程值遥测批次 (iotfast.telemetry.v1) |
| `内部与南向API/07-南向遥测与状态入站` | `POST` | `/v1/source-state` | `telemetry:ingest` | 南向网关上报连接状态与点位故障事件 (iotfast.source_state.v1) |
| `内部与南向API/07-南向遥测与状态入站` | `GET` | `/v1/ingestions/{ingest_id}` | `view:read / telemetry:ingest` | 查询入站批次双维持久与历史状态 (status + history_status) |
| `内部与南向API/10-网关自检与探针` | `GET` | `/v1/ping` | `authenticated scope token` | 北向与控制面鉴权连通性探针 (PingResponse) |
| `内部与南向API/10-网关自检与探针` | `GET` | `/v1/ingest/ping` | `telemetry:ingest` | 南向采集网关入站连通性与 WAL 就绪探针 (PingResponse) |
| `内部与南向API/11-容器健康与就绪探针` | `GET` | `/health` | `none (public health probe)` | 引擎运行健康检查 (根路径 /health，兼容 dev_server 与 iotfast_server) |
| `内部与南向API/11-容器健康与就绪探针` | `GET` | `/v1/health` | `none (public health probe)` | 引擎版本化运行健康检查 (/v1/health，返回 DuckDB v2.0.0、SQLite WAL 与目录状态) |
| `内部与南向API/11-容器健康与就绪探针` | `GET` | `/healthz` | `none (public health probe)` | 容器存活探针 (Liveness Probe /healthz) |
| `内部与南向API/11-容器健康与就绪探针` | `GET` | `/readyz` | `none (public health probe)` | 容器就绪探针 (Readiness Probe /readyz) |

---

## 4. 北向 AI 只读 MCP 工具映射（内部/上层 AI 适配层）

实现脚本：[`scripts/iotfast_mcp_readonly_adapter.py`](../../scripts/iotfast_mcp_readonly_adapter.py)

| MCP 工具名 | 底层调用的对外只读 URL | 强制安全约束 |
| :--- | :--- | :--- |
| `list_uns_resources` | `GET /v1/resources` | 禁止自报 Scope，仅返回令牌授权范围内的 UNS 资源 |
| `get_device_business_view` | `GET /v1/devices/{device_id}/view` | 保留 `quality`、`freshness`、`source_at_ms`、`binding_state`，对 `stale/bad/null` 附加 `is_realtime_normal=False` |
| `query_resource_history` | `POST /v1/history/query` | 强制 `[start_ms, end_ms)` 半开区间与 `limit <= 1000` |
| `list_asset_topology` | `GET /v1/assets` + `GET /v1/asset-relations` | 只读查询层级资产树与跨树拓扑边（如 `FEEDS`） |

---

## 5. 导入到 Apifox 的方法

### 方式 A：通过脚本与 Apifox 个人访问令牌自动导入
```bash
export APIFOX_ACCESS_TOKEN="<你的_afxp_开头令牌>"
python3 scripts/import_to_apifox.py --project-id <你的Apifox项目ID>
```

### 方式 B：在 Apifox 桌面端手工一键导入
1. 打开本机 **Apifox** 客户端 -> 进入目标项目 -> **项目设置** -> **导入数据**。
2. 选择 **OpenAPI / Swagger** 格式，上传本仓库生成的 [`docs/specifications/apifox-openapi-bundle.json`](apifox-openapi-bundle.json)。
3. 导入后即可在左侧目录树看到按 **对外API-北向数据面**、**对外API-物模型控制面**、**内部与南向API** 自动分组的全部 57 个接口及预设环境 URL。
