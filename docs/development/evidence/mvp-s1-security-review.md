# S1：认证与 Scope 安全只读审查

日期：2026-11-10。任务 `task_88550cb44b7d`；仅供 M0/M1/M2 决策参考，不冻结契约，不是产品或 HTTP 安全验收。范围是当前 dirty checkout 的原型源码和服务身份、入站、北向契约；与其他工作并行，源码可能继续变化，合并时须重新核对。允许修改仅本报告，禁止实现、规范、状态及计划修改；未运行 cargo。停止条件：遇范围冲突或需要本审查直接修改实现则交协调者，不越权。

## 阻断项与结论

**当前不能宣称本地演示 HTTP 安全通过，更不能对外部署或宣称生产认证完成。** HTTP 层、绑定地址、凭据真实校验、不可伪造上下文和端到端拒绝无副作用均未实测。以下源码问题需 M1 在暴露相应功能前处理、M2 独立验证：

1. `prototype/src/query/northbound.rs:578` 的 `query_history` 只调用 `require_action`，未调用 `validate_active`，也没有时间参数。反例：过期但有 `history:query` 和资源授权的上下文直接调用可能读取历史。适配层即使先验证也不能代替明确的调用边界及回归；应确保每次实际请求用可信当前时间校验，包含翻页。
2. `prototype/src/ingest/validate.rs:98` 的 `verify_source_grant` 读取 `valid_from_ms` 却丢弃，只检查结束时间。反例：授权从未来开始，当前时间早于起点仍可通过；应按契约确认并执行起止区间，并在任何查重、WAL 或持久化前拒绝。
3. `prototype/src/core/scope.rs:25` 的上下文字段公开且可反序列化，`validate_active` 只检查非空及过期，`require_action` 接受 `*`。这是依赖可信调用者的内部类型，不是凭据验证器：若 HTTP 直接反序列化请求头/体/查询到该类型，攻击者即可自报 `tenant_wide`、资源列表或 `*`。M1 应仅在真实校验开发凭据后从服务端固定映射构造上下文，绝不接受客户端传入的授权上下文；检查所有路由及错误/日志。

## 已证实的源码与契约

- `docs/specifications/service-identity-and-scope-contract.md:12` 要求验证服务身份、可信租户和范围、默认拒绝；请求内租户/source 仅做一致性检查。`docs/architecture/design-specification.md:529` 要求批次原子性及服务端认证主体决定幂等域。
- `prototype/src/core/scope.rs:96` 对资源先比较租户，空范围默认拒绝，非空维度为并集；`prototype/src/core/scope.rs:41` 实施路径段边界匹配。这些仅在上下文和资源元数据可信时成立，不是 HTTP 认证证据。
- `prototype/src/ingest/validate.rs:81` 校验调用者种类及服务 ID 与 source 相等，再按租户/source/device 查 grant；`prototype/src/ingest/validate.rs:141` 与 `:509` 先检查动作和 grant，再进行消息校验。单个遥测 envelope 含一个 device 及多个 point；每个 point、整批拒绝和写入时序仍须验证。
- `prototype/src/query/northbound.rs:263` 的设备视图逐属性应用 Scope；`:593` 历史查询逐个资源验证，任一越界统一 404，再限定请求资源返回历史记录；`:661` gap 也按已验证的资源逐项查询。这是源码路径观察，不证明实际 HTTP 接入和原子性。
- `prototype/src/query/northbound.rs:485` 游标使用秘密前缀 SHA-256 并绑定租户、Scope hash、查询指纹和最后位置；`:500` 校验签名与绑定。其未包含快照边界或有效期，不能据此宣称 Q08 完成；密码学构造、密钥管理和稳定分页也未实测。
- `prototype/src/lib.rs:1` 暴露库模块，`prototype/Cargo.toml:1` 展示当前原型依赖；这些不能证明外部无其他服务入口。`prototype/src/query/subscription.rs:84` 是订阅逻辑而非已验证的实际 WebSocket 服务，演示若不实现 WS 必须明确标为未实现，不把内部订阅逻辑当作 WS 验收。

## M0：最小演示 profile 建议（待决议）

- 仅绑定明确的 IPv4/IPv6 loopback 地址，不接受通配或非 loopback 配置；验证实际监听而非仅检查配置，代理、容器和端口转发另列边界。仅本机演示、虚构数据，开发凭据不复用生产凭据。
- 设独立开发调用者凭据；对每个请求真实验证凭据，缺失、错误、过期拒绝。服务端以固定映射决定调用者、租户、动作、source 与具体资源；禁止 `*` 和 `tenant_wide` 默认授权。不要把静态开发凭据描述为生产签名令牌、mTLS 或完整撤销系统。
- Header、body、query 中的 tenant、scope、role、caller、资源列表均非授权来源；目标参数仅能收窄既有授权，source/tenant 不一致拒绝；禁止混合资源请求部分返回或部分写入。错误及日志不包含凭据、其他租户值或资源细节，越权和不存在一致处理。
- 默认只读设备业务能力；南向模拟数据入站仅限明确授权的演示 source 与 device，不开启设备控制。未实现接口不暴露，尤其 WS 不做则标未实现。
- `orca-mvp-plan.md` 本轮在仓库内未发现，演示具体接口与 profile 待 M0 核对，不阻塞本报告。生产 Q02（issuer/audience、算法、签发、轮换、撤销、代理信任等）及 Q08 仍未关闭；不以生产 12Q 全冻结作为有限本地演示的无限期前提，但也不降低演示必要的认证隔离要求。

## M1：逐项风险与建议

- `prototype/src/ingest/validate.rs:81`：修复 grant 未生效起点；检查源身份与 source 映射不应假定相同字符串可适配所有未来身份。测试撤权后重复消息先拒绝再查重，避免历史状态探测。
- `prototype/src/query/northbound.rs:578`：历史入口在动作与资源判断前校验当前有效身份；所有未来 raw/latest 和批量入口逐项审阅，不依赖仅在某一上游路由调用校验。`:263` 使用拼接资源 ID，需与权威资源记录及实际 latest 关联核对，避免 ID 碰撞或不同标识产生错误授权；此处是风险而非已证明越权。
- `prototype/src/core/scope.rs:54`：`issued_at_ms` 未参与校验；开发凭据有效期起点语义由 M0 决议，并在信任边界验证。`:75` 接受通配动作，只可由可信映射显式配置，本演示建议不用。
- `prototype/src/ingest/validate.rs:91` 错误包含调用者、请求 source 或 device；虽通常来自可信身份或请求自身，适配层仍应统一审查响应/日志，禁止携带凭据、查询到的域外详情及内部异常。数据库错误向外暴露情况未验证。
- `prototype/src/query/northbound.rs:485`：生产游标采用经审查的标准 MAC、密钥管理、有效期和快照边界，绑定 Scope/查询；演示若无法保证则明确限制或不开放分页，不将当前构造宣称为生产安全。

## M2：建议正反测试（均未执行）

1. 启动服务并实测监听：loopback 可达，非 loopback 配置启动失败且非 loopback 地址不可连接；验证 IPv4/IPv6 及代理边界。
2. 有效开发凭据访问授权资源成功；无凭据、错误、空值、篡改、过期拒绝；分别以 header/body/query 伪造 tenant、role、caller、scope、`tenant_wide`、资源数组，无凭据仍拒绝，有低权凭据也不能扩大范围。检查响应和日志无凭据及域外数据。
3. 两租户、同租户两个设备和同设备两个属性的夹具：raw/latest/view/shadow/history 单项分别测试授权、域外、跨租户、不存在，确认域外和不存在同等 404、不泄漏名称、值或计数。每种混合批次中把越权项置于首、中、尾；整体拒绝且不返回授权部分。
4. 南向分别测不同 source、未授权 device、未来起点、到期边界、已撤销 grant、混合合法/非法 point，重复 key 在撤权后仍先拒绝；比较前后 WAL、去重、历史、latest 和状态副作用，全部不变。
5. 对翻页测试过期后续页、修改游标、跨 tenant/Scope/查询重放、越权资源混合和空范围；测试拒绝与无泄露。仅当实际实现 WS 时才测认证、订阅范围、过期/撤权终止及无后续推送，否则记录未实现。

## 环境、命令与限制

- macOS 27.0 (26A428), Darwin arm64；git 2.54.0 (Apple Git-157)；rustc 1.98.1 (48a229cea 2026-09-01)。
- codebase-memory skill 已读取；本会话未提供其 graph 工具，MCP 资源清单为空，不能声称索引覆盖；按要求使用 Semble 搜索加定点源码读取，否定结论限于已读文件。仓库存在并发 dirty 更改，不代表本报告引入。
- 只读命令按实际执行顺序：`printf 'ORCA_AGENT_SESSION_ID=%s\n' "$ORCA_AGENT_SESSION_ID"; cat /Users/fengsl/.codex/skills/codebase-memory/SKILL.md /Users/fengsl/.agents/skills/karpathy-guidelines/SKILL.md /Users/fengsl/.agents/skills/orchestration/SKILL.md; cat '/Users/fengsl/Documents/Obsidian Vault/Orca/00-规范/Orca 工作记录同步规范.md' '/Users/fengsl/Documents/Obsidian Vault/WorkBuddy/40-技术档案/Obsidian 知识库规范.md'`（0，输出截断）；`git status --short; printf '\n--- environment ---\n'; sw_vers; git --version; printf '\n--- instructions ---\n'; find docs -name AGENTS.md -print; printf '\n--- required docs ---\n'; for f in docs/architecture/data-engine-scope.md README.md docs/specifications/README.md docs/adr/0001-mvp-boundaries.md docs/development/roadmap.md docs/development/acceptance.md PRODUCT.md docs/development/ai-execution-plan.md docs/development/specification-decisions.md; do echo "--- $f"; cat "$f"; done`（0，输出截断）。
- 其余只读命令完整记录：`cat docs/specifications/service-identity-and-scope-contract.md; cat prototype/src/core/scope.rs; cat prototype/src/lib.rs; cat prototype/Cargo.toml; if test -f orca-mvp-plan.md; then cat orca-mvp-plan.md; else echo 'orca-mvp-plan.md absent'; fi; printf '\n--- scoped instruction files ---\n'; for f in docs/AGENTS.md docs/development/AGENTS.md docs/development/evidence/AGENTS.md; do if test -f "$f"; then cat "$f"; fi; done`（0，输出截断）；`rg -n '^(#{1,4}) |身份|Scope|scope|认证|鉴权|12Q' docs/architecture/design-specification.md docs/development/ai-execution-plan.md docs/development/acceptance.md docs/development/roadmap.md docs/development/specification-decisions.md PRODUCT.md docs/specifications/northbound-api-and-subscription-contract.md`（0，输出截断）。
- `printf '%s\n' '--- scope ---'; nl -ba prototype/src/core/scope.rs | sed -n '1,175p'; printf '%s\n' '--- ingest ---'; nl -ba prototype/src/ingest/validate.rs | sed -n '85,330p'; printf '%s\n' '--- query ---'; nl -ba prototype/src/query/northbound.rs | sed -n '110,240p;300,460p;570,760p'; printf '%s\n' '--- source layout ---'; ls -la prototype/src prototype/src/ingest prototype/src/query; printf '%s\n' '--- telemetry contract relevant ---'; rg -n '批次|逐|source_device_grant|scope|Scope' docs/specifications/p0-telemetry-contract.md docs/specifications/service-identity-and-scope-contract.md | head -90`（0，输出截断）。
- `rg -n 'pub fn |validate_active|require_action|source_id|grant|is_resource_in_scope|allows_resource|cursor|resource_ids' prototype/src/ingest/validate.rs prototype/src/query/northbound.rs prototype/src/ingest/mod.rs | head -200; cat prototype/src/ingest/mod.rs; ls -la prototype/src; printf '\n--- query target ---\n'; sed -n '140,205p;320,455p;580,720p' prototype/src/query/northbound.rs; printf '\n--- ingest target ---\n'; sed -n '90,240p;500,605p' prototype/src/ingest/validate.rs`（0，输出截断）。
- `nl -ba prototype/src/query/northbound.rs | sed -n '145,180p;260,280p;575,690p'; nl -ba prototype/src/ingest/validate.rs | sed -n '80,130p;140,155p'; rg -n 'pub fn |validate_telemetry|validate_active|verify_source_grant' prototype/src/ingest/runtime.rs prototype/src/ingest/*.rs | head -90; ls prototype/src/ingest; ls prototype/src; rg -n '^##|Q02|Q08' docs/development/ai-execution-plan.md docs/development/specification-decisions.md | head -70`（整体 shell 0；其中 `prototype/src/ingest/runtime.rs` 不存在，管道掩盖了该子命令错误；未依据该路径作结论，后续定位改读实际文件）。
- `nl -ba prototype/src/query/northbound.rs | sed -n '485,575p;640,690p'; nl -ba prototype/src/ingest/validate.rs | sed -n '81,130p'; nl -ba prototype/src/ingest/mod.rs | sed -n '1,220p'; printf '\n--- toolchain ---\n'; uname -sm; rustc --version; printf '\n--- report directory ---\n'; ls -ld docs/development/evidence; printf '\n--- plans ---\n'; find . -name orca-mvp-plan.md -print`（0）。
- 未执行：HTTP 安全实测、测试套件、cargo、运行服务、生产身份/撤销验证、真实设备与生产数据测试。以上是只读源码审查，非产品验收。

## 独立复验：2026-11-10 02:29 CST

- 本次 Task `task_6092cc544634`，dispatch `ctx_112666f9b512`，同一 checkout，仅复验首轮失效的入站路径检索及报告空白；首轮 `task_88550cb44b7d` 失败及其命令错误保留，不以本次结果改写。
- 环境：macOS 27.0 (26A428)、Darwin arm64、zsh 5.9 (arm64-apple-darwin26.0)。先确认路径：`set -o pipefail; printf 'time='; date '+%Y-%m-%d %H:%M:%S %Z'; printf 'environment='; uname -sm; printf 'zsh='; zsh --version; printf '%s\n' '--- exact ingest paths ---'; ls -1 prototype/src/ingest; printf '%s\n' '--- current report state ---'; git status --short -- docs/development/evidence/mvp-s1-security-review.md`，退出码 0；实际文件为 `mod.rs`、`storage.rs`、`validate.rs`、`wal.rs`，报告未跟踪。
- 使用确认后的确切路径：`set -o pipefail; rg -n 'pub fn |validate_telemetry|validate_active|verify_source_grant' prototype/src/ingest/mod.rs prototype/src/ingest/storage.rs prototype/src/ingest/validate.rs prototype/src/ingest/wal.rs | head -90`，退出码 0；输出确认 `validate.rs:81`、`:88`、`:150`、`:517` 及 `storage.rs:137`、`:148` 等调用位置。本次管道启用 pipefail，未重用不存在的 `runtime.rs`。
- 报告全文空白和 EOF 检查命令：`set -o pipefail; python3 -c 'import pathlib,sys; p=pathlib.Path("docs/development/evidence/mvp-s1-security-review.md"); b=p.read_bytes(); lines=b.splitlines(); bad=[i for i,line in enumerate(lines,1) if line.endswith((b" ",b"\t"))]; ok=not bad and b.endswith(b"\n") and not b.endswith(b"\n\n") and b"\r" not in b; print("trailing_whitespace_lines=",bad,"single_final_newline=",b.endswith(b"\n") and not b.endswith(b"\n\n"),"no_CR=",b"\r" not in b); sys.exit(0 if ok else 1)'`；退出码 0，输出 `trailing_whitespace_lines= [] single_final_newline= True no_CR= True`。因报告未跟踪，不能以普通 `git diff --check` 代替该检查。
- 产品安全仍未通过：历史查询有效期、grant 起点与可信认证边界须 M1 处理、M2 实测；没有执行 HTTP 验证或 cargo，生产 Q02/Q08 仍开放。
