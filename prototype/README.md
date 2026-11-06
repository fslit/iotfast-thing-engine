# P0 设计验证原型

> **这不是生产代码。** 本目录包含 D01–D08 设计规范的可执行验证原型，用于在真实网关接入之前，通过 TDD 证明设计方案的可行性与规则自洽性。

## 状态

- 15/15 集成测试通过
- 9 项独立代码审查修复已验证
- 跨语言黄金向量（Rust + Python）100% 一致

## 目录结构

```
prototype/
├── Cargo.toml              # Rust crate 配置
├── src/                    # 原型引擎模块
│   ├── config.rs           # 配置
│   ├── error.rs            # 统一错误体
│   ├── d00.rs              # D00 确定性标识与 JCS 摘要
│   ├── scope.rs            # VerifiedScopeContext
│   ├── health.rs           # 引擎状态与健康检查
│   ├── schema.rs           # DuckDB 表定义
│   ├── model_catalog.rs    # 物模型生命周期
│   ├── asset_uns.rs        # 资产树与 UNS 路径
│   ├── binding.rs          # 绑定版本状态机
│   ├── wal.rs              # 应用层 WAL
│   ├── ingest.rs           # 遥测校验
│   ├── storage_duckdb.rs   # DuckDB 存储与恢复
│   ├── northbound.rs       # 北向查询
│   └── subscription.rs     # 实时订阅
├── tests/                  # 集成测试（d01–d08 + review_fixes）
└── feasibility/            # 技术可行性验证 crate（F0–F3/F6）
```

## 运行测试

```bash
cd prototype/
cargo test        # 15 个集成测试
```

## 与正式实现的关系

正式实现将基于本原型验证的设计结论，采用规范的 Rust 工程结构（workspace、子 crate、分层模块）重新构建。本原型的价值在于：

1. 证明了 20 张 DuckDB 表结构可行
2. 验证了 WAL 持久边界与崩溃恢复逻辑
3. 确认了 D00 确定性标识的跨语言一致性
4. 测试覆盖了设计规范 §27.2 中 T01–T09 的核心场景

详见 [测试结论总结](../docs/development/evidence/p0-core-engine-test-summary.md)。
