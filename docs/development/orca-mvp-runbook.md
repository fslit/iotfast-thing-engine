# Orca 本地 MVP smoke 与启动手册

本文只覆盖 `orca-mvp-dev-v1` 合成开发 profile，不是生产部署、真实网关或 P0/D00–D10 验收说明。

## 前置条件

- 在仓库根目录执行，已有 Rust/Cargo、Python 3 和 `curl`；smoke 本身只使用 Python 标准库与 DuckDB C API，不安装 Python 包。
- 先构建实际二进制：

```sh
cargo build --locked --manifest-path prototype/Cargo.toml --bin dev_server
```

- macOS 当前构建使用 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib`。若缓存中不能唯一定位库，设置 `IOTFAST_DUCKDB_LIBRARY=/绝对路径/libduckdb.dylib`；直接运行服务还需把其父目录放入 `DYLD_LIBRARY_PATH`。Linux 对应使用 `libduckdb.so` 与 `LD_LIBRARY_PATH`。
- 可使用 `IOTFAST_DEV_SERVER_BIN` 指向另一个已构建的 `dev_server`。不得把真实凭据、现场地址或生产数据目录用于本 profile。

## 独立 smoke 主命令

```sh
python3 scripts/orca-mvp-smoke.py
```

脚本每次创建独立 `/tmp/iotfast-m2-*` 目录和新随机 64 位十六进制开发秘密，选择空闲 loopback 端口，直接启动上述真实二进制并执行 HTTP 与独立只读 DuckDB 核对。它只向自己创建的进程发送 SIGINT，必要超时升级仅作用于同一 PID，不使用 `pkill`、固定端口或他人数据目录。

成功时清理自己的数据子目录与进程，保留脱敏 `results.json` 和服务日志；任一断言失败时退出 1，并保留完整临时现场。两种结果都会在末尾输出证据路径，日志与结果不包含开发秘密。连续两次验收：

```sh
python3 scripts/orca-mvp-smoke.py
python3 scripts/orca-mvp-smoke.py
```

覆盖启动拒绝、认证与固定 Scope、工程值与两设备隔离、质量/时间/顺序、原子拒绝、显式 volatile/durable 拒绝、有界 raw 查询、未实现路由，以及 SIGINT 后独立 `READ_ONLY` DuckDB 行数和确定性 observation ID。脚本不测试一小时真实到期、生产认证、真实队列/容量故障、联合 WAL 崩溃恢复、WS、动态配置或真实设备。

## 手工启动

以下命令仅作单次手工演示；每次使用新目录、新秘密和确认空闲的端口，不在终端打印秘密：

```sh
export IOTFAST_DEV_DATA_DIR="$(mktemp -d /tmp/iotfast-mvp.XXXXXX)"
export IOTFAST_DEV_BIND=127.0.0.1:18081
export IOTFAST_DEV_INGEST_TOKEN="$(openssl rand -hex 32)"
export IOTFAST_DEV_READER_TOKEN="$(openssl rand -hex 32)"
export IOTFAST_DEV_ALL_TOKEN="$(openssl rand -hex 32)"
export DYLD_LIBRARY_PATH="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0"
cargo run --locked --manifest-path prototype/Cargo.toml --bin dev_server
```

服务只接受字面量 `127.0.0.1:<1..65535>`，数据目录必须已存在、为空且为绝对路径。启动 stderr 的 `DuckDB SELECT version(): ...` 是运行时引擎证据；不要用 crate 版本替代。Ctrl-C 只停止本次服务。该 profile 明确为 `accepted_volatile`；即便 DuckDB 文件随后可读，也不代表 durable ACK、跨会话去重、应用 WAL 联合恢复或断电无损。

## 停止条件与范围外

任何 smoke 断言失败或命令非零都不得标为通过，应保留失败目录与精确复现并交实现 owner 修复，不能放宽测试或修改 wire。WebSocket/source-state/shadow/ingestion 查询/动态配置当前只验证明确拒绝；生产认证、容量、性能、故障恢复、真实网关联调和设备控制均未由本 smoke 验收。
