# S3 构建环境与依赖只读核验

日期：2026-11-10；任务：S3 / `task_079ea15f53bf`；HEAD：`c0a5a42d247c66e66e94475ad7a5d0abd210e1b3`。范围仅为当前 dirty checkout 的静态审查，不是构建、运行或验收。工作区包含多个已有修改、删除和未跟踪文件，详见下述 `git status`；并发工作可能改变后续状态。

## 实测与边界

- `uname -a`：Darwin 27.0.0，ARM64_T6000 / arm64；`sw_vers`：macOS 27.0，build 26A428。`rustc` 1.98.1 (48a229cea 2026-09-01)，`cargo` 1.98.1 (797e8a9bc 2026-08-05)；均来自 `/Users/fengsl/.cargo/bin`。
- `prototype/Cargo.toml` 声明 `duckdb = "1.20000.1"`；`prototype/Cargo.lock` 将 `duckdb` 和 `libduckdb-sys` 均锁为 `1.20000.1`。这是 Rust crate 版本，不能当作引擎版本。
- PATH 内未找到 DuckDB CLI，也未在 `/opt/homebrew/lib`、`/opt/homebrew/include` 及 `/opt/homebrew`、`/usr/local`、`/Users/fengsl/.local` 所检查的深度内找到已安装 DuckDB；这不是全盘搜索。系统实际已安装引擎版本未知。本轮未加载库、链接或查询引擎版本，实际链接引擎版本亦未知；因此无法断言与目标 DuckDB 2.0.0 一致或兼容。
- 历史产物 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib` 和头文件存在；旧 build-script 输出声称从 v2.0.0 地址下载并以该目录、`prototype/target/debug/build/libduckdb-sys-acf1d93a399cd675/out` 为 native 搜索路径，动态链接 `duckdb`。这只是历史线索，目录名和日志不证明库实际版本，也不证明当前 checkout 可构建。`otool` 显示库包含 arm64、x86_64，install name 为 `@rpath/libduckdb.dylib`；运行时需验证加载路径及实际加载库。该缓存不是系统安装。
- `pkg-config` 命令不存在；`cc`、`clang`、`otool`、`curl` 存在。检查的 `DUCKDB_LIB_DIR`、`DUCKDB_INCLUDE_DIR`、`DUCKDB_DOWNLOAD_LIB`、`DUCKDB_STATIC`、`CARGO_TARGET_DIR`、`DYLD_LIBRARY_PATH`、`PKG_CONFIG_PATH` 当前均未设置。`~/.cargo/config`、`~/.cargo/config.toml`、仓库及 prototype 的 `.cargo` 目录不存在。缓存源码显示 `libduckdb-sys` 支持显式库目录；没有它时可尝试系统链接，且默认特性包含可选下载机制；本轮未触发下载。
- 当前 manifest 和 lock 未声明 `axum` 或 `reqwest`；本机 registry 源码缓存中有 `axum` 0.7.9/0.8.9 和 `reqwest` 0.12.28/0.13.5，不代表被项目选用、依赖闭包完整、可离线构建或 HTTP 服务可用。HTTP/WS/认证选型仍待 D01 决定。
- `prototype/target` 和 Cargo registry 的 `src`、`cache` 目录存在，含相关 crate 源码和历史 rlib；不证明与当前源码、配置、工具链或任何目标平台匹配。既有 README 中测试通过是历史描述，不是本轮结果。

## M0 / M1 / M2 后续条件

- M0：保留本报告的版本与未知项，确认正式工程复用及 HTTP/认证选型；不得把静态审查当作构建通过。目标为 2.0.0，而当前无运行时版本证据，差距为**未验证**，不是已证实版本不一致。
- M1：待并发 Cargo 工作结束、获准验证且预置完整离线依赖后，先确认 `prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0` 库及头文件来源、完整性与适用性；显式提供库路径，并使用独立 target，避免复用历史产物和触发下载。以下从仓库根执行，`<独立绝对路径>` 为事先准备的专用目录；不可把建议当作本轮已执行结果：

  ```sh
  DUCKDB_DOWNLOAD_LIB=0 DUCKDB_LIB_DIR="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0" DUCKDB_INCLUDE_DIR="$PWD/prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0" CARGO_TARGET_DIR="<独立绝对路径>" cargo build --locked --offline --manifest-path prototype/Cargo.toml
  ```

  本机已有库是临时缓存，不是干净机器前提；依赖或库缺失即记录错误并停止，不自动安装。启动或测试还须解决 `@rpath` 运行时加载，并在实际进程中查询 DuckDB 版本、记录加载路径及退出码；不能仅以链接成功代替版本验证。当前原型是否提供可启动服务未验证。构建失败、缺包或加载失败均不是通过。
- M2 环境记录模板：任务 ID / 规范版本 / HEAD / dirty 状态；日期、OS/内核/架构、CPU、RAM、磁盘与文件系统；Rust/Cargo；锁定 crate 与实际加载 DuckDB 版本、库来源/校验值/链接和运行路径；HTTP/认证依赖版本；配置与非敏感参数；完整命令及退出码、输入、预期与实际、日志位置、未覆盖项。负载或恢复验证另记数据模式、频率、持续时间、确认语义、P50/P95/P99、丢失/重复数和故障方式；未知填未知。

## 命令与退出码

下列均在仓库根执行，除标注的退出码外为 0；管道只用于截取文本，不能证明被截断之外的内容。未输出敏感环境值。

| 完整命令 | 退出码 / 结果 |
|---|---|
| `git status --short` | 0；既有多处 docs、README、AGENTS、prototype 变更、删除及未跟踪文件，未清理。 |
| `uname -a` | 0；Darwin 27.0.0 / arm64。 |
| `sw_vers` | 0；macOS 27.0 (26A428)。 |
| `rustc --version` | 0；1.98.1。 |
| `cargo --version` | 0；1.98.1。 |
| `git rev-parse HEAD` | 0；见报告开头。 |
| `command -v duckdb` | 1；PATH 中不存在。 |
| `command -v pkg-config` | 1；PATH 中不存在。 |
| `command -v rustc; command -v cargo; command -v cc; command -v clang; command -v otool; command -v curl` | 0；依次为 `~/.cargo/bin/rustc`、`~/.cargo/bin/cargo`、`/usr/bin/cc`、`/usr/bin/clang`、`/usr/bin/otool`、`/usr/bin/curl`。 |
| `printenv CARGO_TARGET_DIR DUCKDB_DOWNLOAD_LIB DUCKDB_LIB_DIR DUCKDB_INCLUDE_DIR DUCKDB_STATIC DYLD_LIBRARY_PATH PKG_CONFIG_PATH` | 1；无输出，所列变量均未设置。 |
| `find /opt/homebrew/lib /usr/local/lib /opt/homebrew/include /usr/local/include -maxdepth 1 \( -name '*duckdb*' -o -name 'duckdb.h' \) -print` | 1；`/usr/local/lib`、`/usr/local/include` 不存在。 |
| `find /opt/homebrew/lib /opt/homebrew/include -maxdepth 1 \( -name '*duckdb*' -o -name 'duckdb.h' \) -print` | 0；无匹配。 |
| `find /opt/homebrew /usr/local /Users/fengsl/.local -maxdepth 5 \( -name 'libduckdb*.dylib' -o -name 'duckdb-*.dist-info' \) -print` | 0；无匹配。 |
| `ls -ld /opt/homebrew/lib/libduckdb* /usr/local/lib/libduckdb* /opt/homebrew/include/duckdb.h /usr/local/include/duckdb.h` | 1；zsh 报 `no matches found: /opt/homebrew/lib/libduckdb*`，未完成检查；以上 find 补查。 |
| `ls -ld prototype/target ~/.cargo/registry/src ~/.cargo/registry/cache ~/.cargo/config ~/.cargo/config.toml` | 1；前三项存在，后两项不存在。 |
| `find ~/.cargo/registry/src -maxdepth 2 \( -name 'duckdb-1.20000.1' -o -name 'libduckdb-sys-1.20000.1' -o -name 'axum-*' -o -name 'reqwest-*' \) -print; find prototype/target -maxdepth 4 \( -name '*duckdb*' -o -name 'libiotfast*' \) -print \| head -70` | 0；发现上述源码与历史产物。 |
| `cat prototype/target/debug/build/libduckdb-sys-044f33190bd4c8b4/output` | 1；该路径不存在；后用 find 定位另一目录。 |
| `find prototype/target/debug/build/libduckdb-sys-acf1d93a399cd675 prototype/target/debug/build/libduckdb-sys-044f33190bd4c8b4 -maxdepth 2 -type f -print` | 0；找到历史输出。 |
| `cat prototype/target/debug/build/libduckdb-sys-acf1d93a399cd675/output; otool -L prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib; otool -D prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib` | 0；历史链接线索及 install name 如上。 |
| `cat prototype/Cargo.lock \| sed -n '/^name = "duckdb"$/,+18p; /^name = "libduckdb-sys"$/,+20p; /^name = "reqwest"$/,+12p; /^name = "axum"$/,+12p'` | 0；锁定版本如上，未见指定 HTTP 包。 |
| `find docs prototype -name AGENTS.md -print; rg -n -i 'duckdb\|cargo\|http\|构建\|M0\|M1\|M2' docs/development/ai-execution-plan.md docs/development/specification-decisions.md docs/development/roadmap.md docs/development/acceptance.md docs/architecture/design-specification.md prototype/README.md \| head -160` | 0；未发现嵌套 AGENTS；筛查构建要求。 |
| `rg -n 'DUCKDB_DOWNLOAD_LIB\|DUCKDB_LIB_DIR\|DUCKDB_INCLUDE_DIR\|DUCKDB_STATIC\|pkg_config\|download' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/build.rs /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/Cargo.toml prototype/.cargo/config.toml` | 2；`prototype/.cargo/config.toml` 不存在；其余匹配输出已读取。 |
| `sed -n '1,200p' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/build.rs; sed -n '40,120p' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/duckdb-1.20000.1/Cargo.toml; ls -la prototype/.cargo .cargo` | 1；两个 `.cargo` 目录不存在，前两项输出可读。 |
| `sed -n '1,200p' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/src/build_linked.rs; sed -n '58,80p' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/Cargo.toml; cat prototype/target/debug/.fingerprint/libduckdb-sys-393ec37d407e76c7/lib-libduckdb_sys.json` | 0（末项）；首项路径不存在；后两项显示默认特性和历史 fingerprint。 |
| `sed -n '1,200p' /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/build_linked.rs` | 0；读取实际链接逻辑。 |
| `cat docs/architecture/data-engine-scope.md README.md docs/specifications/README.md docs/adr/0001-mvp-boundaries.md docs/development/roadmap.md docs/development/acceptance.md PRODUCT.md docs/development/ai-execution-plan.md docs/development/specification-decisions.md prototype/Cargo.toml prototype/README.md` | 0；输出被工具截断，相关构建条款另以筛查读取。 |
| `cat /Users/fengsl/.agents/skills/orchestration/SKILL.md; cat /Users/fengsl/.agents/skills/karpathy-guidelines/SKILL.md; cat '/Users/fengsl/Documents/Obsidian Vault/Orca/00-规范/Orca 工作记录同步规范.md'; cat '/Users/fengsl/Documents/Obsidian Vault/WorkBuddy/40-技术档案/Obsidian 知识库规范.md'` | 0（末项）；输出截断。 |
| `orca skills get orchestration` | 0；读取当前版本协作规范。 |
| `git diff --check -- docs/development/evidence/mvp-s3-build-review.md` | 0；文件未跟踪，普通 `git diff` 不检查其内容，不可视作该文件空白检查通过。 |

未执行 `cargo build/test/check/fetch/update/metadata`、联网下载、安装、服务启动、运行时版本查询、功能或性能测试；静态检查不能计为构建或功能通过。`pkg-config` 和 DuckDB CLI 缺失为观察结果，不是运行本审查的必要工具；显式库路径方案仍须实测。未修改全局配置或既有文件。

## 后续任务复验：task_f1c51d6d490a

原任务 `task_079ea15f53bf` 因原命令表存在退出 1/2 和分号命令掩盖前项错误，已由协调者纠正为 **failed**；原记录与非零结果保留，不因本节覆盖或改判。以下各命令独立执行，无管道及分号命令掩盖退出码，均为本次真实结果：

| 完整命令 | 退出码 / 实际结果 |
|---|---|
| `test -f prototype/Cargo.toml` | 0；manifest 存在。 |
| `test -f prototype/Cargo.lock` | 0；lock 存在。 |
| `test -f /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/build.rs` | 0；构建脚本存在。 |
| `test -f /Users/fengsl/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libduckdb-sys-1.20000.1/build_linked.rs` | 0；链接脚本存在。 |
| `test -f prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib` | 0；历史库存在。 |
| `test -f prototype/target/debug/build/libduckdb-sys-acf1d93a399cd675/output` | 0；历史构建输出存在。 |
| `python3 -c 'from pathlib import Path; paths=[Path.home()/".cargo/config",Path.home()/".cargo/config.toml",Path(".cargo"),Path("prototype/.cargo")]; [print(f"{path}: exists={path.exists()}") for path in paths]'` | 0；四个配置路径存在性均为 false，此为观察而非读取缺失文件。 |
| `python3 -c 'import tomllib; from pathlib import Path; manifest=tomllib.loads(Path("prototype/Cargo.toml").read_text()); lock=tomllib.loads(Path("prototype/Cargo.lock").read_text()); print("manifest duckdb:", manifest["dependencies"]["duckdb"]); [print("lock", package["name"], package["version"]) for package in lock["package"] if package["name"] in ("duckdb", "libduckdb-sys")]; print("HTTP packages in lock:", [package["name"] for package in lock["package"] if package["name"] in ("axum", "reqwest")])'` | 0；manifest 与 lock 的 DuckDB crate 均为 1.20000.1；lock 未列 axum 或 reqwest。 |
| `otool -D prototype/target/duckdb-download/aarch64-apple-darwin/v2.0.0/libduckdb.dylib` | 0；x86_64 和 arm64 均显示 `@rpath/libduckdb.dylib`。 |
| `cat prototype/target/debug/build/libduckdb-sys-acf1d93a399cd675/output` | 0；历史输出记录 v2.0.0 下载目录和动态链接搜索路径，仅属历史线索。 |

本次未执行构建、链接、服务启动或运行时版本查询；实际链接 DuckDB 引擎及与 2.0.0 目标的关系仍未知。下面的独立 Python 检查直接读取未跟踪报告，验证无尾随空格、恰好一个 EOF 换行且无 CR，不依赖 `git diff`：

```sh
python3 -c 'from pathlib import Path; data=Path("docs/development/evidence/mvp-s3-build-review.md").read_bytes(); assert b"\r" not in data, "CR found"; assert data.endswith(b"\n") and not data.endswith(b"\n\n"), "EOF newline"; assert all(not line.endswith((b" ", b"\t")) for line in data.split(b"\n")), "trailing whitespace"; print("format OK")'
```

该格式检查退出码 0，输出 `format OK`。本次复验不代表原任务成功，也不代表构建或功能通过。
