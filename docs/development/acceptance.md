# 测试与验收入口

权威的测试向量 T01–T10 和 F01–F05 见 [设计规范 27.2 节](../architecture/design-specification.md#272-必须通过的测试向量)，故障恢复边界见 23 节。本文件用于组织验证，不代表任何项目测试已经执行。

| 领域 | 验收重点 |
|---|---|
| 契约 | 数据类型、未知字段、版本匹配、整消息拒绝及单位。 |
| 模型 / 目录 | 不可变已发布版本、资产迁址身份不变、授权范围。 |
| 数据质量 | 工程值只换算一次、旧数据不覆盖新状态、超时不伪造新测量。 |
| 持久可靠 | durable ACK 前同步、消息幂等、日志 / 事务 / checkpoint 各故障点重放。 |
| 实时订阅 | 一致快照与增量边界；溢出必须明确要求重新同步。 |
| 历史 | 时间半开区间、稳定分页、质量过滤、版本语义兼容性。 |
| AI / 权限 | 目录和工具按服务端身份过滤；显示质量与采样时间。 |
| 上层参考体验 | 按接入旅程及 UX01–UX12 验证 API 事实；页面验收由上层项目执行，不纳入引擎页面交付。 |
| 现场控制（非引擎） | 独立上层项目验收，明确人工确认、联锁、物理反馈与未知状态；不得因此要求引擎新增控制功能。 |

任务级验收与 T / SC / NB / F 追踪见[AI 执行任务包](ai-execution-plan.md)，体验向量见[接入体验规范](../product/integration-experience.md)。总体 F01–F05 与可行性 F0–F6 不是同一编号空间，见[决策台账](specification-decisions.md)。报告逐项标记通过 / 失败 / 未执行 / 不适用及理由，未执行不能计作通过。

## 本地受控闭环与扩展验证一键可复现命令（2026-11-11）

完整接口与运行说明见 [设备物模型与 UNS 数据管理引擎 MVP 运行手册](mvp-thing-model-runbook.md)：

```bash
# 1. 原型全量单元与集成测试套件（30 passed，覆盖 D01–D11、存储分层 SD-STORAGE-01/SD-STORAGE-02、ST-02/ST-04/凭据脱敏、D01.2 单实例锁、D10.1 在线备份恢复、UX01–UX12、P1 能力与 HTTP/WS 端到端闭环）
cargo test --locked --manifest-path prototype/Cargo.toml

# 2. D00 黄金向量契约测试（13 passed）
cargo test --locked --manifest-path prototype/feasibility/f1_d00_contract/Cargo.toml

# 3. Rust ↔ Python 跨语言黄金向量对账（7 组向量 / 14 个 observation_id 100% 匹配）
python3 prototype/feasibility/f1_d00_contract/verify_vectors.py

# 4. D00.3 机器契约校验（4 份 JSON Schema + OpenAPI 3.1 的 49 条路径 / 57 个操作 / 63 个组件 Schema / 58 项路由与 $ref 检查 + 66 项正反断言）
python3 scripts/verify-machine-contracts.py

# 5. D11 上层只读 MCP 适配器自检（只读工具集、R09 属性裁剪、控制指令拒绝与注入防护）
python3 scripts/iotfast_mcp_readonly_adapter.py --self-test

# 6. 端到端物模型、UNS 数据管理、存储适配器切换与在线快照备份校验一键验收演示（8 步全链路）
python3 scripts/mvp-thing-model-demo.py
```

## 验证证据报告索引（`docs/development/evidence/`）

1. [P0 核心数据引擎与全量回归测试结论总结](evidence/p0-core-engine-test-summary.md)
2. [D00.3 机器契约（JSON Schema 2020-12 & OpenAPI 3.1）验证报告](evidence/d00-3-machine-contract-verification.md)
3. [SD-STORAGE-01 业务元数据（SQLite）与设备时序数据（DuckDB / TDengine）分层存储验证报告](evidence/storage-sqlite-duckdb-tdengine-report.md)
4. [D09 离线工业网关断连缓存、乱序补传与绑定切换对账报告](evidence/d09-offline-gateway-fixture-report.md)
5. [D10 开发态故障矩阵（F01–F05）与容量水位/延迟基线报告](evidence/d10-dev-fault-capacity-report.md)
6. [D11 上层只读 MCP 适配器与北向安全边界验证报告](evidence/d11-mcp-readonly-adapter-report.md)
7. [U01–U08 接入旅程与 UX01–UX12 状态矩阵验证报告](evidence/ux01-ux12-integration-journey-report.md)
8. [P1 可插拔时序存储能力边界与物模型派生指标（OEE / 单位能耗）验证报告](evidence/p1-capability-and-derived-metrics-report.md)

功能验收至少包含正例、拒绝无副作用、范围隔离与相应并发 / 恢复路径；有限原型断言不能代替 HTTP / WS 传输、签名认证或真实设备验收。每项证据携带任务 ID、规范版本、代码版本与工作区修改、命令及退出码、输入预期与实际结果、环境和未覆盖项。

性能和恢复报告必须记录操作系统、硬件、磁盘、Rust 和 DuckDB 版本、数据模式、负载、持续时间、确认语义及 P50/P95/P99、丢失 / 重复数。进程强杀不等同真实断电测试。
