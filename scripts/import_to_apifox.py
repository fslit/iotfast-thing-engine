#!/usr/bin/env python3
"""
Build the Apifox-optimized OpenAPI bundle (External Northbound/Control-Plane +
Internal Southbound/Ops/Dev-Profile APIs and URLs) and optionally import it into
Apifox via the Apifox Open API (`POST /v1/projects/{projectId}/import-openapi`).

Security rule (AGENTS.md):
- Never hardcode or commit real Apifox access tokens into repository files.
- Pass token via `--token` or `APIFOX_ACCESS_TOKEN` environment variable.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent
SOURCE_OPENAPI_PATH = REPO_ROOT / "docs" / "specifications" / "openapi-v1.json"
APIFOX_BUNDLE_PATH = REPO_ROOT / "docs" / "specifications" / "apifox-openapi-bundle.json"
API_CATALOG_DOC_PATH = REPO_ROOT / "docs" / "specifications" / "api-url-catalog.md"


# Folder definitions for Apifox directory tree organization
FOLDER_DEFINITIONS: List[str] = [
    "对外API-北向数据面/01-设备业务视图与实时订阅",
    "对外API-北向数据面/02-UNS资源目录与历史查询",
    "对外API-物模型控制面/03-产品与物模型管理",
    "对外API-物模型控制面/04-资产树与跨树拓扑关系",
    "对外API-物模型控制面/05-设备实例化与生命周期",
    "对外API-物模型控制面/08-边缘适配器能力与转换",
    "对外API-物模型控制面/09-派生指标与流式计算",
    "对外API-物模型控制面/09b-存储架构与时序适配器",
    "内部与南向API/06-属性数据引用与来源授权",
    "内部与南向API/07-南向遥测与状态入站",
    "内部与南向API/10-网关自检与探针",
    "内部与南向API/11-容器健康与就绪探针",
]

# Folder classification (External vs Internal) for Apifox
ROUTE_METADATA: Dict[Tuple[str, str], Dict[str, Any]] = {
    # --- 01. 对外北向：设备业务视图、影子与实时订阅 ---
    ("get", "/v1/devices/{device_id}/view"): {
        "folder": "对外API-北向数据面/01-设备业务视图与实时订阅",
        "tag": "01-对外北向-设备业务视图与实时订阅",
        "cn_summary": "获取设备完整物模型业务视图 (DeviceBusinessView)",
        "scope_visibility": "external_northbound",
        "required_action": "view:read",
    },
    ("get", "/v1/devices/{device_id}/shadow"): {
        "folder": "对外API-北向数据面/01-设备业务视图与实时订阅",
        "tag": "01-对外北向-设备业务视图与实时订阅",
        "cn_summary": "获取设备实时影子视图 (DeviceShadowView)",
        "scope_visibility": "external_northbound",
        "required_action": "shadow:read",
    },
    ("get", "/v1/subscriptions"): {
        "folder": "对外API-北向数据面/01-设备业务视图与实时订阅",
        "tag": "01-对外北向-设备业务视图与实时订阅",
        "cn_summary": "WebSocket 实时快照与增量订阅 (WS /v1/subscriptions)",
        "scope_visibility": "external_northbound",
        "required_action": "subscription:connect",
    },
    # --- 02. 对外北向：UNS 资源目录与历史查询 ---
    ("get", "/v1/resources"): {
        "folder": "对外API-北向数据面/02-UNS资源目录与历史查询",
        "tag": "02-对外北向-UNS资源目录与历史查询",
        "cn_summary": "查询 UNS 稳定资源目录或按资产子树枚举",
        "scope_visibility": "external_northbound",
        "required_action": "view:read",
    },
    ("get", "/v1/resources:resolve"): {
        "folder": "对外API-北向数据面/02-UNS资源目录与历史查询",
        "tag": "02-对外北向-UNS资源目录与历史查询",
        "cn_summary": "按 UNS URI 精确解析稳定 resource_id (含重定向状态)",
        "scope_visibility": "external_northbound",
        "required_action": "view:read",
    },
    ("post", "/v1/history/query"): {
        "folder": "对外API-北向数据面/02-UNS资源目录与历史查询",
        "tag": "02-对外北向-UNS资源目录与历史查询",
        "cn_summary": "查询设备属性历史观测与聚合分桶 (支持水位线稳定游标)",
        "scope_visibility": "external_northbound",
        "required_action": "history:query",
    },
    # --- 03. 对外控制面：产品与物模型目录 ---
    ("post", "/v1/products"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "创建产品定义 (Product)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("get", "/v1/products"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "列出当前租户下的产品目录",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    ("post", "/v1/products/{product_id}/models"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "在指定产品下创建物模型版本草稿",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("post", "/v1/models"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "创建物模型版本草稿 (ModelRevision Draft)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("get", "/v1/models"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "列出物模型版本摘要列表",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    ("get", "/v1/models/{model_revision_id}"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "获取指定物模型版本完整属性定义",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    ("post", "/v1/models/{model_revision_id}:validate"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "校验物模型草稿属性、标量类型与工程单位",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("post", "/v1/models/{model_revision_id}:publish"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "发布并冻结不可变物模型版本 (Published)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("get", "/v1/models/{model_revision_id}:diff"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "对比两个物模型版本的兼容性差异 (GET :diff)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    ("post", "/v1/models:compare"): {
        "folder": "对外API-物模型控制面/03-产品与物模型管理",
        "tag": "03-对外控制面-产品与物模型管理",
        "cn_summary": "对比两个物模型版本的兼容性差异 (POST :compare)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    # --- 04. 对外控制面：层级资产树与跨树拓扑关系 ---
    ("post", "/v1/assets"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "创建 UNS 层级资产树节点 (自动计算 uns_path)",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:write",
    },
    ("get", "/v1/assets"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "查询 UNS 层级资产树节点列表",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:read",
    },
    ("post", "/v1/assets/relations"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "创建跨树逻辑拓扑关系 (FEEDS / BACKUP_FOR / MONITORS)",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:write",
    },
    ("get", "/v1/assets/{asset_id}/relations"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "查询指定资产关联拓扑边列表",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:read",
    },
    ("post", "/v1/asset-relations"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "创建跨树逻辑拓扑关系 (FEEDS / BACKUP_FOR / MONITORS)",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:write",
    },
    ("get", "/v1/asset-relations"): {
        "folder": "对外API-物模型控制面/04-资产树与跨树拓扑关系",
        "tag": "04-对外控制面-资产树与跨树拓扑关系",
        "cn_summary": "查询资产关联拓扑边列表",
        "scope_visibility": "external_control_plane",
        "required_action": "asset:read",
    },
    # --- 05. 对外控制面：设备实例化、迁址与平滑升级 ---
    ("post", "/v1/devices"): {
        "folder": "对外API-物模型控制面/05-设备实例化与生命周期",
        "tag": "05-对外控制面-设备实例化与生命周期",
        "cn_summary": "基于已发布物模型实例化设备并分配稳定 resource_id",
        "scope_visibility": "external_control_plane",
        "required_action": "device:write",
    },
    ("get", "/v1/devices"): {
        "folder": "对外API-物模型控制面/05-设备实例化与生命周期",
        "tag": "05-对外控制面-设备实例化与生命周期",
        "cn_summary": "查询当前租户下的设备实例列表",
        "scope_visibility": "external_control_plane",
        "required_action": "device:read",
    },
    ("get", "/v1/devices/{device_id}"): {
        "folder": "对外API-物模型控制面/05-设备实例化与生命周期",
        "tag": "05-对外控制面-设备实例化与生命周期",
        "cn_summary": "获取指定设备实例详情与绑定状态",
        "scope_visibility": "external_control_plane",
        "required_action": "device:read",
    },
    ("post", "/v1/devices/{device_id}:relocate"): {
        "folder": "对外API-物模型控制面/05-设备实例化与生命周期",
        "tag": "05-对外控制面-设备实例化与生命周期",
        "cn_summary": "设备跨资产节点迁址 (保持 resource_id 不变，保护历史 URI)",
        "scope_visibility": "external_control_plane",
        "required_action": "device:write",
    },
    ("post", "/v1/devices/{device_id}:upgrade-model"): {
        "folder": "对外API-物模型控制面/05-设备实例化与生命周期",
        "tag": "05-对外控制面-设备实例化与生命周期",
        "cn_summary": "设备物模型版本平滑升级 (保留既有 resource_id，拒绝破坏性变更)",
        "scope_visibility": "external_control_plane",
        "required_action": "device:write",
    },
    # --- 08. 对外控制面：边缘适配器能力与转换 ---
    ("post", "/v1/adapters"): {
        "folder": "对外API-物模型控制面/08-边缘适配器能力与转换",
        "tag": "08-对外控制面-边缘适配器能力与转换",
        "cn_summary": "注册或更新边缘协议适配器能力与转换模型 (EdgeAdapterCapability)",
        "scope_visibility": "external_control_plane",
        "required_action": "binding:write",
    },
    ("get", "/v1/adapters"): {
        "folder": "对外API-物模型控制面/08-边缘适配器能力与转换",
        "tag": "08-对外控制面-边缘适配器能力与转换",
        "cn_summary": "列出当前租户已注册的边缘协议适配器能力目录",
        "scope_visibility": "external_control_plane",
        "required_action": "binding:read",
    },
    ("post", "/v1/adapters/{adapter_id}:validate-binding"): {
        "folder": "对外API-物模型控制面/08-边缘适配器能力与转换",
        "tag": "08-对外控制面-边缘适配器能力与转换",
        "cn_summary": "校验属性数据引用与指定边缘适配器能力兼容性 (ValidateAdapterBindingRequest -> ValidateAdapterBindingResponse)",
        "scope_visibility": "external_control_plane",
        "required_action": "binding:write",
    },
    # --- 09. 对外控制面：派生指标与流式计算 ---
    ("post", "/v1/derived-metrics"): {
        "folder": "对外API-物模型控制面/09-派生指标与流式计算",
        "tag": "09-对外控制面-派生指标与流式计算",
        "cn_summary": "创建或更新物模型确定性派生指标定义 (DerivedMetricDefinition)",
        "scope_visibility": "external_control_plane",
        "required_action": "model:write",
    },
    ("get", "/v1/derived-metrics"): {
        "folder": "对外API-物模型控制面/09-派生指标与流式计算",
        "tag": "09-对外控制面-派生指标与流式计算",
        "cn_summary": "列出当前租户下的物模型派生指标定义目录",
        "scope_visibility": "external_control_plane",
        "required_action": "model:read",
    },
    ("post", "/v1/derived-metrics/{metric_id}:evaluate"): {
        "folder": "对外API-物模型控制面/09-派生指标与流式计算",
        "tag": "09-对外控制面-派生指标与流式计算",
        "cn_summary": "基于设备实时业务视图与质量守卫评估派生指标并输出输入血缘 (EvaluateDerivedMetricRequest -> EvaluateDerivedMetricResponse)",
        "scope_visibility": "external_control_plane",
        "required_action": "history:query / view:read",
    },
    # --- 09b. 对外控制面：存储架构与可插拔时序适配器 ---
    ("get", "/v1/storage/status"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "查看 SQLite 业务存储与可插拔时序存储适配器（DuckDB / TDengine 等）状态 (StorageArchitectureStatus)",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("post", "/v1/storage/timeseries-adapter"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "动态切换或配置设备时序数据存储适配器（支持 duckdb、tdengine、timescaledb、clickhouse；一期默认启用 SQLite + DuckDB + AppWal；外部 TDengine (taosd) 集群实连验收（ST-06）规划在二期启用）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("post", "/v1/storage/query-plan"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "预览当前时序存储适配器（DuckDB / TDengine 等）生成的原生历史/聚合查询 SQL (BuildQueryPlanRequest -> TimeSeriesQueryPlan)",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("get", "/v1/storage/capabilities"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "查询各时序存储后端能力快照（ST-02）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("get", "/v1/storage/checkpoints"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "查询时序投递连续检查点与未决空洞（ST-04）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("post", "/v1/storage/checkpoints:reconcile"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "对账或隔离未决投递空洞并推进连续检查点（ST-04）",
        "scope_visibility": "external_control_plane",
        "required_action": "binding:write / authenticated scope token",
    },
    ("post", "/v1/storage/backups"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "创建 SQLite + DuckDB + WAL 一致性快照备份（D10.1）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("get", "/v1/storage/backups"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "查询一致性快照备份列表（D10.1）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    ("post", "/v1/storage/backups/{backup_id}:verify"): {
        "folder": "对外API-物模型控制面/09b-存储架构与时序适配器",
        "tag": "09b-对外控制面-存储架构与时序适配器",
        "cn_summary": "在隔离沙箱中校验快照备份 SHA-256 与恢复完整性（D10.1）",
        "scope_visibility": "external_control_plane",
        "required_action": "authenticated scope token",
    },
    # --- 06. 内部与南向配置：属性数据引用与来源授权 ---
    ("post", "/v1/sources/grants"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "授予南向网关上报指定设备的半开时间窗权限 (source_device_grant)",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:write",
    },
    ("post", "/v1/devices/{device_id}/bindings"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "为设备创建属性数据引用草稿 (动态采集 points + 静态配置 static_points)",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:write",
    },
    ("post", "/v1/bindings"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "创建属性数据引用草稿 (通用入口)",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:write",
    },
    ("get", "/v1/bindings/{binding_revision_id}"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "查询指定属性数据引用版本快照",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:read",
    },
    ("post", "/v1/bindings/{binding_revision_id}:validate"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "校验属性数据引用草稿与物模型类型/单位一致性",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:write",
    },
    ("post", "/v1/bindings/{binding_revision_id}:activate"): {
        "folder": "内部与南向API/06-属性数据引用与来源授权",
        "tag": "06-内部与南向配置-属性数据引用与来源授权",
        "cn_summary": "激活数据引用版本 (凭 gateway_ack 或 manual_stop_window)",
        "scope_visibility": "internal_southbound_config",
        "required_action": "binding:write",
    },
    # --- 07. 内部与南向入站：遥测上报、状态事件与双维状态对账 ---
    ("post", "/v1/telemetry"): {
        "folder": "内部与南向API/07-南向遥测与状态入站",
        "tag": "07-内部与南向入站-遥测与状态上报",
        "cn_summary": "南向网关上报工程值遥测批次 (iotfast.telemetry.v1)",
        "scope_visibility": "internal_southbound_ingest",
        "required_action": "telemetry:ingest",
    },
    ("post", "/v1/source-state"): {
        "folder": "内部与南向API/07-南向遥测与状态入站",
        "tag": "07-内部与南向入站-遥测与状态上报",
        "cn_summary": "南向网关上报连接状态与点位故障事件 (iotfast.source_state.v1)",
        "scope_visibility": "internal_southbound_ingest",
        "required_action": "telemetry:ingest",
    },
    ("get", "/v1/ingestions/{ingest_id}"): {
        "folder": "内部与南向API/07-南向遥测与状态入站",
        "tag": "07-内部与南向入站-遥测与状态上报",
        "cn_summary": "查询入站批次双维持久与历史状态 (status + history_status)",
        "scope_visibility": "internal_southbound_ingest",
        "required_action": "view:read / telemetry:ingest",
    },
    # --- 10. 内部与南向探针：网关自检与鉴权连通性探针 ---
    ("get", "/v1/ping"): {
        "folder": "内部与南向API/10-网关自检与探针",
        "tag": "10-内部与南向-网关自检与探针",
        "cn_summary": "北向与控制面鉴权连通性探针 (PingResponse)",
        "scope_visibility": "internal_ops",
        "required_action": "authenticated scope token",
    },
    ("get", "/v1/ingest/ping"): {
        "folder": "内部与南向API/10-网关自检与探针",
        "tag": "10-内部与南向-网关自检与探针",
        "cn_summary": "南向采集网关入站连通性与 WAL 就绪探针 (PingResponse)",
        "scope_visibility": "internal_southbound_ingest",
        "required_action": "telemetry:ingest",
    },
    # --- 11. 内部运维与容器健康/就绪探针 ---
    ("get", "/health"): {
        "folder": "内部与南向API/11-容器健康与就绪探针",
        "tag": "11-内部运维-容器健康与就绪探针",
        "cn_summary": "引擎运行健康检查 (根路径 /health，兼容 dev_server 与 iotfast_server)",
        "scope_visibility": "internal_ops",
        "required_action": "none (public health probe)",
    },
    ("get", "/v1/health"): {
        "folder": "内部与南向API/11-容器健康与就绪探针",
        "tag": "11-内部运维-容器健康与就绪探针",
        "cn_summary": "引擎版本化运行健康检查 (/v1/health，返回 DuckDB v2.0.0、SQLite WAL 与目录状态)",
        "scope_visibility": "internal_ops",
        "required_action": "none (public health probe)",
    },
    ("get", "/healthz"): {
        "folder": "内部与南向API/11-容器健康与就绪探针",
        "tag": "11-内部运维-容器健康与就绪探针",
        "cn_summary": "容器存活探针 (Liveness Probe /healthz)",
        "scope_visibility": "internal_ops",
        "required_action": "none (public health probe)",
    },
    ("get", "/readyz"): {
        "folder": "内部与南向API/11-容器健康与就绪探针",
        "tag": "11-内部运维-容器健康与就绪探针",
        "cn_summary": "容器就绪探针 (Readiness Probe /readyz)",
        "scope_visibility": "internal_ops",
        "required_action": "none (public health probe)",
    },
}


# Default ready-to-send path/query parameter examples for Apifox debugging
PARAMETER_EXAMPLES: Dict[str, Any] = {
    "device_id": "pump-001",
    "model_revision_id": "pump-model-r1",
    "binding_revision_id": "bind-pump-001-r1",
    "asset_id": "asset-room-1",
    "ingest_id": "ing-000001",
    "product_id": "prod-pump",
    "adapter_id": "adapter-modbus-gw01",
    "metric_id": "metric-specific-power",
    "target": "pump-model-r1",
    "backend": "duckdb",
    "backup_id": "bkp-seed-001",
    "uri": "uns://ten-8iic/plant-a/room-1/pump-001/outlet_pressure",
    "uri_prefix": "uns://ten-8iic/plant-a",
    "include_descendants": True,
    "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
    "Prefer": "durability=durable",
}

# Ready-to-send JSON request body examples for every POST endpoint
POST_REQUEST_EXAMPLES: Dict[str, Dict[str, Any]] = {
    "/v1/products": {
        "product_id": "prod-pump",
        "product_key": "booster-pump",
        "name": "智能变频增压泵",
    },
    "/v1/products/{product_id}/models": {
        "model_revision_id": "pump-model-r1",
        "product_id": "prod-pump",
        "model_version": "1.0.0",
        "properties": [
            {
                "identifier": "outlet_pressure",
                "name": "出口压力",
                "value_type": "float64",
                "unit_code": "MPa",
                "unit_symbol": "MPa",
                "min_value": 0.0,
                "max_value": 4.0,
                "required": True,
            },
            {
                "identifier": "rated_power",
                "name": "额定功率",
                "value_type": "float64",
                "unit_code": "kW",
                "unit_symbol": "kW",
                "min_value": 0.0,
                "max_value": 500.0,
                "required": False,
            },
        ],
    },
    "/v1/models": {
        "model_revision_id": "pump-model-r1",
        "product_id": "prod-pump",
        "model_version": "1.0.0",
        "properties": [
            {
                "identifier": "outlet_pressure",
                "name": "出口压力",
                "value_type": "float64",
                "unit_code": "MPa",
                "unit_symbol": "MPa",
                "min_value": 0.0,
                "max_value": 4.0,
                "required": True,
            },
            {
                "identifier": "rated_power",
                "name": "额定功率",
                "value_type": "float64",
                "unit_code": "kW",
                "unit_symbol": "kW",
                "min_value": 0.0,
                "max_value": 500.0,
                "required": False,
            },
        ],
    },
    "/v1/models:compare": {
        "from_revision_id": "pump-model-r1",
        "to_revision_id": "pump-model-r2",
    },
    "/v1/models/{model_revision_id}:validate": {
        "previous_published_revision_id": None,
    },
    "/v1/models/{model_revision_id}:publish": {
        "published_at_ms": 12000,
    },
    "/v1/assets": {
        "asset_id": "asset-room-1",
        "parent_asset_id": "asset-plant-a",
        "slug": "room-1",
        "name": "一号泵房",
    },
    "/v1/assets/relations": {
        "relation_id": "rel-room1-feeds-north",
        "from_asset_id": "asset-room-1",
        "relation_type": "FEEDS",
        "to_asset_id": "asset-header-north",
        "valid_from_ms": 16000,
        "valid_to_ms": None,
        "source": "process_topology",
    },
    "/v1/asset-relations": {
        "relation_id": "rel-room1-feeds-north",
        "from_asset_id": "asset-room-1",
        "relation_type": "FEEDS",
        "to_asset_id": "asset-header-north",
        "valid_from_ms": 16000,
        "valid_to_ms": None,
        "source": "process_topology",
    },
    "/v1/devices": {
        "device_id": "pump-001",
        "device_key": "pump-001",
        "device_name": "1号增压主泵",
        "product_id": "prod-pump",
        "model_revision_id": "pump-model-r1",
        "asset_id": "asset-room-1",
    },
    "/v1/devices/{device_id}:relocate": {
        "target_asset_id": "asset-room-2",
        "relocated_at_ms": 30000,
        "redirect_retention_ms": 86400000,
    },
    "/v1/devices/{device_id}:upgrade-model": {
        "target_model_revision_id": "pump-model-r2",
        "expected_device_revision": 1,
        "upgraded_at_ms": 19000,
    },
    "/v1/devices/{device_id}/bindings": {
        "binding_revision_id": "bind-pump-001-r1",
        "device_id": "pump-001",
        "model_revision_id": "pump-model-r1",
        "source_id": "gw-01",
        "normalization_owner": "gateway",
        "server_scale_factor": None,
        "points": [
            {
                "property": "outlet_pressure",
                "register_address": "40001",
                "factor": 0.01,
                "unit_code": "MPa",
                "deadband": 0.02,
                "heartbeat_ms": 60000,
            }
        ],
        "static_points": [
            {
                "property": "rated_power",
                "value": 55.0,
                "unit_code": "kW",
            }
        ],
    },
    "/v1/sources/grants": {
        "source_id": "gw-01",
        "device_id": "pump-001",
        "valid_from_ms": 20000,
    },
    "/v1/bindings": {
        "binding_revision_id": "bind-pump-001-r1",
        "device_id": "pump-001",
        "model_revision_id": "pump-model-r1",
        "source_id": "gw-01",
        "normalization_owner": "gateway",
        "server_scale_factor": None,
        "points": [
            {
                "property": "outlet_pressure",
                "register_address": "40001",
                "factor": 0.01,
                "unit_code": "MPa",
                "deadband": 0.02,
                "heartbeat_ms": 60000,
            }
        ],
        "static_points": [
            {
                "property": "rated_power",
                "value": 55.0,
                "unit_code": "kW",
            }
        ],
    },
    "/v1/bindings/{binding_revision_id}:validate": {},
    "/v1/bindings/{binding_revision_id}:activate": {
        "expected_device_revision": 1,
        "activated_at_ms": 23000,
        "gateway_ack": None,
        "manual_stop_window": {
            "window_start_ms": 22500,
            "window_end_ms": 23500,
            "operator_id": "op-apifox",
        },
    },
    "/v1/telemetry": {
        "spec": "iotfast.telemetry.v1",
        "message_id": "msg-apifox-001",
        "source_id": "gw-01",
        "source_epoch": "epoch-01",
        "device_id": "pump-001",
        "model_revision_id": "pump-model-r1",
        "binding_revision_id": "bind-pump-001-r1",
        "points": [
            {
                "property": "outlet_pressure",
                "value": 1.65,
                "source_at_ms": 30000,
                "source_time_origin": "device_clock",
                "source_seq": "10",
                "quality": {
                    "state": "good",
                    "source_protocol": "modbus",
                    "source_code": None,
                },
                "delivery": {
                    "replayed": False,
                },
            }
        ],
    },
    "/v1/source-state": {
        "spec": "iotfast.source_state.v1",
        "event_id": "evt-apifox-001",
        "source_id": "gw-01",
        "source_epoch": "epoch-01",
        "device_id": "pump-001",
        "binding_revision_id": "bind-pump-001-r1",
        "observed_at_ms": 35000,
        "source_time_origin": "gateway_sample",
        "source_seq": "11",
        "connection_state": "connected",
        "point_states": [
            {
                "property": "outlet_pressure",
                "quality_state": "good",
                "quality_reason": "normal",
                "source_protocol": "modbus",
                "source_code": None,
            }
        ],
        "delivery": {
            "replayed": False,
        },
    },
    "/v1/history/query": {
        "resource_ids": [
            "res-pump-001-outlet-pressure",
        ],
        "range": {
            "start_ms": 0,
            "end_ms": 4102444800000,
        },
        "mode": "raw",
        "aggregation": None,
        "quality_states": [
            "good",
            "uncertain",
            "bad",
        ],
        "order": "asc",
        "limit": 100,
        "cursor": None,
    },
    "/v1/storage/timeseries-adapter": {
        "backend": "tdengine",
        "strict_lossless": False,
        "tdengine": {
            "endpoint": "http://127.0.0.1:6041",
            "database": "iotfast_tsdb",
            "super_table": "stb_observations",
            "username": "root",
            "password": "taosdata",
            "execution_mode": "local_mirror_with_sql_spool",
        },
    },
    "/v1/storage/query-plan": {
        "resource_ids": [
            "res-pump-001-outlet-pressure",
        ],
        "start_ms": 0,
        "end_ms": 4102444800000,
        "bucket_ms": 60000,
    },
    "/v1/storage/checkpoints:reconcile": {
        "target_backend": "duckdb",
        "wal_position": "0000000000000001:0000000000000000",
        "resolution": "mark_committed",
        "reason": "Verified observation_id roundtrip against target store",
        "operator_id": "op-apifox",
    },
    "/v1/storage/backups": {
        "backup_id": "bkp-apifox-001",
    },
    "/v1/storage/backups/{backup_id}:verify": {},
    "/v1/adapters": {
        "adapter_id": "adapter-modbus-gw01",
        "protocol": "modbus_tcp",
        "supported_value_types": [
            "float64",
            "int64",
            "bool",
            "text",
        ],
        "supported_transforms": [
            "gateway_normalized",
            "linear_scale",
        ],
        "max_batch_size": 500,
        "supports_deadband": True,
    },
    "/v1/adapters/{adapter_id}:validate-binding": {
        "value_type": "float64",
        "transform": "gateway_normalized",
        "batch_size": 100,
        "requires_deadband": True,
    },
    "/v1/derived-metrics": {
        "metric_id": "metric-specific-power",
        "name": "单位压力功率比",
        "product_id": "prod-pump",
        "model_revision_id": "pump-model-r1",
        "expression": "rated_power / outlet_pressure",
        "input_properties": [
            "rated_power",
            "outlet_pressure",
        ],
        "output_unit_code": "kW_per_MPa",
        "quality_policy": "reject_on_bad",
    },
    "/v1/derived-metrics/{metric_id}:evaluate": {
        "device_id": "pump-001",
        "evaluated_at_ms": 30500,
    },
}


def build_apifox_bundle() -> Dict[str, Any]:
    raw = json.loads(SOURCE_OPENAPI_PATH.read_text(encoding="utf-8"))
    bundle = copy.deepcopy(raw)

    # Apifox OpenAPI importer supports 3.0.3 / 3.1.0; enrich servers & tags
    bundle["info"]["title"] = "IoTFast 物模型与 UNS 数据管理引擎 API（对外北向/控制面 + 内部南向/运维全集）"
    bundle["info"]["description"] = (
        "八位魔方 / IoTFast 设备物模型与 UNS 数据管理引擎完整接口规范（已对齐 D00.2 Q01–Q12 裁决）。\n\n"
        "- **对外北向数据面**：设备完整业务视图 (`/v1/devices/{id}/view`)、设备影子 (`/v1/devices/{id}/shadow`)、"
        "UNS 稳定资源目录 (`/v1/resources`)、接收水位线稳定分页历史查询 (`/v1/history/query`)、RFC 6455 WebSocket 实时订阅 (`WS /v1/subscriptions`)。\n"
        "- **对外物模型与资产控制面**：产品目录 (`/v1/products`)、不可变物模型版本与兼容性比对 (`/v1/models*`)、"
        "层级资产树与跨树拓扑关系 (`/v1/assets`, `/v1/asset-relations`)、设备实例化与迁址/平滑模型升级 (`/v1/devices*`)、"
        "存储架构与可插拔时序存储适配器管理 (`/v1/storage/status`, `/v1/storage/timeseries-adapter`, `/v1/storage/query-plan`)。\n"
        "- **内部与南向接口**：南向网关授权 (`/v1/sources/grants`)、属性数据引用绑定 (`/v1/devices/{id}/bindings`, `/v1/bindings*`)、"
        "南向工程值遥测上报 (`/v1/telemetry`)、网关状态事件上报 (`/v1/source-state`)、双维入站状态对账 (`/v1/ingestions/{ingest_id}`) 与健康诊断 (`/health`, `/v1/health`)。"
    )
    bundle["servers"] = [
        {
            "url": "http://127.0.0.1:8080",
            "description": "对外与内部完整引擎服务 (iotfast_server 默认监听地址)",
        },
        {
            "url": "http://127.0.0.1:18090",
            "description": "本地端到端验收演示服务 (scripts/mvp-thing-model-demo.py)",
        },
        {
            "url": "http://127.0.0.1:18080",
            "description": "内部最小只读开发 Profile 服务 (dev_server 烟雾验证端口)",
        },
    ]

    # Add /v1/health alias alongside /health so both URLs are documented in Apifox
    if "/health" in bundle["paths"] and "/v1/health" not in bundle["paths"]:
        v1_health = copy.deepcopy(bundle["paths"]["/health"])
        v1_health["get"]["operationId"] = "getV1Health"
        bundle["paths"]["/v1/health"] = v1_health

    tags_seen: List[Dict[str, str]] = []
    tag_names = set()

    for path_key, path_item in bundle["paths"].items():
        if not isinstance(path_item, dict):
            continue
        for method_key, op in path_item.items():
            if method_key.lower() not in {"get", "post", "put", "delete", "patch"}:
                continue
            if not isinstance(op, dict):
                continue
            meta = ROUTE_METADATA.get((method_key.lower(), path_key))
            if meta:
                tag_name = meta["tag"]
                op["tags"] = [tag_name]
                op["x-apifox-folder"] = meta["folder"]
                op["x-iotfast-visibility"] = meta["scope_visibility"]
                op["x-iotfast-required-action"] = meta["required_action"]
                orig_summary = op.get("summary", "")
                op["summary"] = meta["cn_summary"]
                op["description"] = (
                    f"**中文说明**：{meta['cn_summary']}\n\n"
                    f"- **接口分类**：`{meta['scope_visibility']}`\n"
                    f"- **所需 Scope Action**：`{meta['required_action']}`\n"
                    f"- **英文摘要**：{orig_summary}"
                )
                if tag_name not in tag_names:
                    tag_names.add(tag_name)
                    tags_seen.append({"name": tag_name, "description": meta["folder"]})

            # Populate default examples for path/query/header parameters
            for param in op.get("parameters", []):
                if not isinstance(param, dict):
                    continue
                pname = param.get("name")
                if pname in PARAMETER_EXAMPLES:
                    ex_val = PARAMETER_EXAMPLES[pname]
                    param["example"] = ex_val
                    if isinstance(param.get("schema"), dict):
                        param["schema"]["example"] = ex_val
                        param["schema"]["default"] = ex_val

            # Populate ready-to-send JSON requestBody example for every POST operation
            if method_key.lower() == "post" and path_key in POST_REQUEST_EXAMPLES:
                ex_body = copy.deepcopy(POST_REQUEST_EXAMPLES[path_key])
                if "requestBody" not in op or not isinstance(op["requestBody"], dict):
                    op["requestBody"] = {
                        "required": False,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "additionalProperties": False,
                                },
                                "example": ex_body,
                            }
                        },
                    }
                else:
                    content = op["requestBody"].setdefault("content", {})
                    app_json = content.setdefault("application/json", {})
                    app_json["example"] = ex_body

    bundle["tags"] = sorted(tags_seen, key=lambda x: x["name"])
    APIFOX_BUNDLE_PATH.write_text(
        json.dumps(bundle, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return bundle


def write_api_catalog_markdown() -> None:
    lines = [
        "# IoTFast 物模型与 UNS 数据管理引擎：对外与内部 API 及 URL 清单",
        "",
        "状态：2026-11-10 已同步至 [`docs/specifications/apifox-openapi-bundle.json`](apifox-openapi-bundle.json) 与 [`docs/specifications/openapi-v1.json`](openapi-v1.json)。",
        "",
        "## 1. 服务端基础 URL 与运行环境",
        "",
        "| 环境名称 | 基础 URL | 对应守护进程 | 用途说明 |",
        "| :--- | :--- | :--- | :--- |",
        "| **标准完整引擎服务（默认）** | `http://127.0.0.1:8080` | `iotfast_server` | 对外北向数据面、物模型/资产控制面、南向遥测与状态入站全量 API |",
        "| **北向 WebSocket 实时订阅** | `ws://127.0.0.1:8080/v1/subscriptions` | `iotfast_server` | RFC 6455 实时快照衔接（`snapshot_begin/chunk/end`）与增量推送（`delta`） |",
        "| **端到端演示环境** | `http://127.0.0.1:18090` | `iotfast_server` | `python3 scripts/mvp-thing-model-demo.py` 一键演示默认端口 |",
        "| **内部最小只读开发 Profile** | `http://127.0.0.1:18080` | `dev_server` | 内部最小烟雾测试（仅开放 `/health`、`/v1/telemetry` volatile、`/v1/devices/{id}/view`、`/v1/history/query`） |",
        "",
        "---",
        "",
        "## 2. 对外 API（External Northbound & Control Plane）",
        "",
        "| 分类目录 | HTTP 方法 | URL 路径 | 所需 Scope Action | 功能说明 |",
        "| :--- | :---: | :--- | :--- | :--- |",
    ]

    for (method, path), meta in ROUTE_METADATA.items():
        if meta["scope_visibility"].startswith("external_"):
            lines.append(
                f"| `{meta['folder']}` | `{method.upper()}` | `{path}` | `{meta['required_action']}` | {meta['cn_summary']} |"
            )

    lines.extend(
        [
            "",
            "---",
            "",
            "## 3. 内部与南向 API（Internal Southbound & Ops）",
            "",
            "| 分类目录 | HTTP 方法 | URL 路径 | 所需 Scope Action | 功能说明 |",
            "| :--- | :---: | :--- | :--- | :--- |",
        ]
    )

    for (method, path), meta in ROUTE_METADATA.items():
        if meta["scope_visibility"].startswith("internal_"):
            lines.append(
                f"| `{meta['folder']}` | `{method.upper()}` | `{path}` | `{meta['required_action']}` | {meta['cn_summary']} |"
            )

    lines.extend(
        [
            "",
            "---",
            "",
            "## 4. 北向 AI 只读 MCP 工具映射（内部/上层 AI 适配层）",
            "",
            "实现脚本：[`scripts/iotfast_mcp_readonly_adapter.py`](../../scripts/iotfast_mcp_readonly_adapter.py)",
            "",
            "| MCP 工具名 | 底层调用的对外只读 URL | 强制安全约束 |",
            "| :--- | :--- | :--- |",
            "| `list_uns_resources` | `GET /v1/resources` | 禁止自报 Scope，仅返回令牌授权范围内的 UNS 资源 |",
            "| `get_device_business_view` | `GET /v1/devices/{device_id}/view` | 保留 `quality`、`freshness`、`source_at_ms`、`binding_state`，对 `stale/bad/null` 附加 `is_realtime_normal=False` |",
            "| `query_resource_history` | `POST /v1/history/query` | 强制 `[start_ms, end_ms)` 半开区间与 `limit <= 1000` |",
            "| `list_asset_topology` | `GET /v1/assets` + `GET /v1/asset-relations` | 只读查询层级资产树与跨树拓扑边（如 `FEEDS`） |",
            "",
            "---",
            "",
            "## 5. 导入到 Apifox 的方法",
            "",
            "### 方式 A：通过脚本与 Apifox 个人访问令牌自动导入",
            "```bash",
            "export APIFOX_ACCESS_TOKEN=\"<你的_afxp_开头令牌>\"",
            "python3 scripts/import_to_apifox.py --project-id <你的Apifox项目ID>",
            "```",
            "",
            "### 方式 B：在 Apifox 桌面端手工一键导入",
            "1. 打开本机 **Apifox** 客户端 -> 进入目标项目 -> **项目设置** -> **导入数据**。",
            "2. 选择 **OpenAPI / Swagger** 格式，上传本仓库生成的 [`docs/specifications/apifox-openapi-bundle.json`](apifox-openapi-bundle.json)。",
            f"3. 导入后即可在左侧目录树看到按 **对外API-北向数据面**、**对外API-物模型控制面**、**内部与南向API** 自动分组的全部 {len(ROUTE_METADATA)} 个接口及预设环境 URL。",
        ]
    )

    API_CATALOG_DOC_PATH.write_text("\n".join(lines) + "\n", encoding="utf-8")


def http_json(
    method: str,
    url: str,
    token: str,
    body: Optional[Dict[str, Any]] = None,
    api_version: str = "2024-03-28",
) -> Tuple[int, Any]:
    headers = {
        "Authorization": f"Bearer {token}",
        "X-Apifox-Api-Version": api_version,
        "Accept": "application/json",
        "User-Agent": "iotfast-apifox-importer/1.0",
    }
    data_bytes = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data_bytes = json.dumps(body, ensure_ascii=False).encode("utf-8")

    req = urllib.request.Request(url, data=data_bytes, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=15) as resp:
            raw = resp.read().decode("utf-8", errors="replace")
            try:
                return resp.status, json.loads(raw)
            except Exception:
                return resp.status, raw
    except urllib.error.HTTPError as e:
        raw = e.read().decode("utf-8", errors="replace")
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw


def discover_local_apifox_projects() -> List[Dict[str, Any]]:
    """
    Inspect local macOS Apifox desktop logs/cache (read-only) to find recently opened project IDs
    if the user did not pass `--project-id`.
    """
    candidates: List[Dict[str, Any]] = []
    home = Path.home()
    search_roots = [
        home / "Library" / "Application Support" / "apifox",
        home / "Library" / "Application Support" / "Apifox",
        home / "Library" / "Logs" / "apifox",
        home / "Library" / "Logs" / "Apifox",
        home / ".apifox",
    ]
    import re

    seen_ids = set()
    proj_pat = re.compile(r'"projectId"\s*:\s*(\d{4,12})')
    url_pat = re.compile(r"/projects/(\d{4,12})\b")

    for root in search_roots:
        if not root.exists():
            continue
        for p in sorted(root.rglob("*"), key=lambda x: x.stat().st_mtime if x.exists() else 0, reverse=True):
            if not p.is_file():
                continue
            try:
                if p.stat().st_size > 15 * 1024 * 1024:
                    continue
                content = p.read_text(encoding="utf-8", errors="ignore")
                for m in proj_pat.finditer(content):
                    pid = int(m.group(1))
                    start = max(0, m.start() - 120)
                    end = min(len(content), m.end() + 120)
                    snippet = content[start:end].replace("\n", " ")
                    if pid > 0 and pid not in seen_ids:
                        seen_ids.add(pid)
                        candidates.append({"id": pid, "source": str(p), "snippet": snippet})
                for m in url_pat.finditer(content):
                    pid = int(m.group(1))
                    start = max(0, m.start() - 120)
                    end = min(len(content), m.end() + 120)
                    snippet = content[start:end].replace("\n", " ")
                    if pid > 0 and pid not in seen_ids:
                        seen_ids.add(pid)
                        candidates.append({"id": pid, "source": str(p), "snippet": snippet})
            except Exception:
                continue
    return candidates


def discover_remote_teams(token: str) -> List[Dict[str, Any]]:
    """Query Apifox endpoints to discover the user's teams and their exact teamId."""
    endpoints = [
        ("https://api.apifox.com/api/v1/user-teams?locale=zh-CN", "2024-03-28"),
        ("https://api.apifox.com/v1/teams?locale=zh-CN", "2024-03-28"),
        ("https://api.apifox.com/api/v1/teams?locale=zh-CN", "2024-03-28"),
    ]
    for url, ver in endpoints:
        try:
            st, body = http_json("GET", url, token, None, ver)
            if st == 200 and isinstance(body, dict):
                data = body.get("data")
                if isinstance(data, list) and data:
                    return data
                if isinstance(data, dict) and isinstance(data.get("list"), list):
                    return data["list"]
        except Exception:
            continue
    return []


def discover_remote_projects(token: str) -> List[Dict[str, Any]]:
    """Query Apifox Open API / internal endpoints to discover accessible projects."""
    endpoints = [
        ("https://api.apifox.com/api/v1/user-projects", "2024-03-28"),
        ("https://api.apifox.com/v1/projects", "2024-03-28"),
        ("https://api.apifox.com/v1/user-projects", "2024-03-28"),
        ("https://api.apifox.cn/api/v1/user-projects", "2024-03-28"),
    ]
    found: List[Dict[str, Any]] = []
    for url, ver in endpoints:
        try:
            st, body = http_json("GET", url, token, None, ver)
            if st == 200 and isinstance(body, dict):
                data = body.get("data")
                if isinstance(data, list):
                    for item in data:
                        if isinstance(item, dict) and "id" in item:
                            found.append(item)
                elif isinstance(data, dict) and isinstance(data.get("list"), list):
                    for item in data["list"]:
                        if isinstance(item, dict) and "id" in item:
                            found.append(item)
            if found:
                return found
        except Exception:
            continue
    return found


def create_project_in_team(token: str, team_id: int, project_name: str) -> Optional[int]:
    """Create a new HTTP project under the specified Apifox teamId and return its projectId."""
    candidate_requests = [
        (
            "https://api.apifox.com/api/v1/projects?locale=zh-CN",
            {
                "name": project_name,
                "teamId": team_id,
                "type": "HTTP",
                "visibility": "private",
                "description": "IoTFast 设备物模型与 UNS 数据管理引擎 API（对外北向/控制面 + 内部南向/运维全集）",
            },
        ),
        (
            "https://api.apifox.com/v1/projects?locale=zh-CN",
            {
                "name": project_name,
                "teamId": team_id,
                "type": "HTTP",
                "visibility": "private",
            },
        ),
        (
            f"https://api.apifox.com/api/v1/teams/{team_id}/projects?locale=zh-CN",
            {
                "name": project_name,
                "teamId": team_id,
                "type": "HTTP",
            },
        ),
    ]
    for url, payload in candidate_requests:
        st, resp = http_json("POST", url, token, payload, api_version="2024-03-28")
        print(f"Create project attempt ({url}) -> HTTP {st}: {json.dumps(resp, ensure_ascii=False) if isinstance(resp, dict) else resp}")
        if st in (200, 201) and isinstance(resp, dict):
            data = resp.get("data")
            if isinstance(data, dict) and "id" in data:
                return int(data["id"])
            if "id" in resp:
                return int(resp["id"])
    return None


def import_bundle_to_apifox(
    token: str,
    project_id: int,
    bundle: Dict[str, Any],
) -> Tuple[int, Any]:
    url = f"https://api.apifox.com/v1/projects/{project_id}/import-openapi?locale=zh-CN"
    payload = {
        "input": json.dumps(bundle, ensure_ascii=False),
        "options": {
            "targetEndpointFolderId": 0,
            "targetSchemaFolderId": 0,
            "endpointOverwriteBehavior": "OVERWRITE_EXISTING",
            "schemaOverwriteBehavior": "OVERWRITE_EXISTING",
            "updateFolderOfChangedEndpoint": True,
            "prependBasePath": False,
        },
    }
    return http_json("POST", url, token, payload, api_version="2024-03-28")


def main() -> int:
    parser = argparse.ArgumentParser(description="Build and import IoTFast OpenAPI bundle into Apifox")
    parser.add_argument("--token", default=os.environ.get("APIFOX_ACCESS_TOKEN", ""), help="Apifox Access Token (afxp_...)")
    parser.add_argument("--project-id", type=int, default=0, help="Target Apifox Project ID")
    parser.add_argument("--create-in-team-id", type=int, default=0, help="Create a new project under this Apifox teamId and import into it")
    parser.add_argument("--create-in-team-name", default="", help="Create a new project under the team with this exact name and import into it")
    parser.add_argument("--project-name", default="物模型与UNS数据引擎 (iotfast-thing-engine)", help="Project name when creating a new project")
    parser.add_argument("--all-projects", action="store_true", help="Import into all discovered Apifox projects")
    parser.add_argument("--list-projects", action="store_true", help="List all discovered local and remote Apifox projects")
    parser.add_argument(
        "--build-only",
        "--generate-only",
        dest="build_only",
        action="store_true",
        help="Only generate apifox-openapi-bundle.json and api-url-catalog.md",
    )
    args = parser.parse_args()

    bundle = build_apifox_bundle()
    write_api_catalog_markdown()
    print(f"[1/2] Generated Apifox OpenAPI bundle: {APIFOX_BUNDLE_PATH} ({len(bundle['paths'])} paths)")
    print(f"[2/2] Generated External & Internal API Catalog: {API_CATALOG_DOC_PATH}")

    if args.build_only:
        return 0

    token = args.token.strip()
    if args.list_projects:
        print("Local discovered projects:", json.dumps(discover_local_apifox_projects(), ensure_ascii=False, indent=2))
        if token:
            print("Remote discovered teams:", json.dumps(discover_remote_teams(token), ensure_ascii=False, indent=2))
            print("Remote discovered projects:", json.dumps(discover_remote_projects(token), ensure_ascii=False, indent=2))
        return 0
    if not token:
        print("INFO: No --token or APIFOX_ACCESS_TOKEN provided; skipping remote import.")
        return 0

    project_ids: List[int] = []
    target_team_id = args.create_in_team_id
    if args.create_in_team_name.strip():
        teams = discover_remote_teams(token)
        print("Discovered teams:", json.dumps(teams, ensure_ascii=False))
        for t in teams:
            if isinstance(t, dict) and t.get("name") == args.create_in_team_name.strip():
                target_team_id = int(t["id"])
                print(f"Matched team '{args.create_in_team_name}' -> teamId={target_team_id}")
                break

    if target_team_id > 0:
        existing = [
            p for p in discover_remote_projects(token)
            if int(p.get("teamId", 0)) == target_team_id and p.get("name") == args.project_name
        ]
        if existing:
            new_pid = int(existing[0]["id"])
            print(f"Found existing project '{args.project_name}' (id={new_pid}) in team {target_team_id}")
            project_ids.append(new_pid)
        else:
            new_pid = create_project_in_team(token, target_team_id, args.project_name)
            if new_pid:
                print(f"Created project '{args.project_name}' (id={new_pid}) in team {target_team_id}")
                project_ids.append(new_pid)
            else:
                print("ERROR: Failed to create new project in team.")
                return 3
    elif args.project_id > 0:
        project_ids.append(args.project_id)
    else:
        local_projs = discover_local_apifox_projects()
        for lp in local_projs:
            if lp["id"] not in project_ids:
                project_ids.append(lp["id"])
        remote_projs = discover_remote_projects(token)
        for rp in remote_projs:
            pid = int(rp["id"])
            if pid not in project_ids:
                project_ids.append(pid)

    if not project_ids:
        print("WARN: Could not auto-discover Apifox projectId from local Apifox cache or remote API.")
        print("      Please pass --project-id <PROJECT_ID> (found in Apifox -> Project Settings -> Basic Settings).")
        return 2

    any_success = False
    for pid in project_ids:
        print(f"Attempting import into Apifox project_id={pid} ...")
        st, resp = import_bundle_to_apifox(token, pid, bundle)
        print(f"  -> HTTP {st}: {json.dumps(resp, ensure_ascii=False) if isinstance(resp, dict) else resp}")
        if st in (200, 201):
            any_success = True
            if not args.all_projects:
                break

    return 0 if any_success else 1


if __name__ == "__main__":
    sys.exit(main())
