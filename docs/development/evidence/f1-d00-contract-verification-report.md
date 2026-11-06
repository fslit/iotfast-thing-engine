# F1 & D00 契约可行性实测验证报告（遥测原子校验、确定性 ID、JCS 摘要、状态事件分离与 Scope 隔离）

## 1. 基本信息

- **验证编号**：F1 & D00（《P0 技术可行性验证计划》第 1、3、9 节）
- **验证日期**：2026-11-06
- **执行人**：F1 & D00 独立验证代理（f1-d00）
- **操作系统与硬件**：macOS Darwin 27.0.0 (`arm64`, Apple Silicon `RELEASE_ARM64_T6000`)
- **工具链版本**：
  - Rust：`rustc 1.98.1 (48a229cea 2026-09-01)` / `cargo 1.98.1 (797e8a9bc 2026-08-05)`
  - Python：`Python 3.14.7`
  - 核心依赖：`serde 1.0.229`、`serde_json 1.0.154`、`sha2 0.10.9`、`data-encoding 2.11.1`、`ryu 1.0.23`、`hex 0.4.3`
- **基准提交 SHA**：`840c36f1b485266a595b6730aa5852b1b56ce126`

---

## 2. 假设、输入数据来源、规模及安全隔离

### 2.1 已验证事实 vs 设计假设
- **已验证事实（本次实测覆盖）**：
  1. D00 确定性 `observation_id` 派生算法（`"obs-" + Base32LowerNoPad(SHA-256("iotfast.obs.v1\0" + tenant_id + "\0" + source_id + "\0" + source_epoch + "\0" + message_id + "\0" + property)[0..16])`）与 `sha256-jcs-v1`（RFC 8785 受限子集）业务摘要算法在 Rust 与 Python 3 下对 6 组黄金测试向量（共 13 个点位）计算结果 100% 逐字节一致。
  2. `points[].delivery.replayed` 翻转（`false -> true`）、HTTP `X-Request-Id` 改变、多点消息 `points` 数组输入顺序调换均不改变规范化 JCS 字符串、不改变 `payload_hash` 与各点 `observation_id`。
  3. F1 遥测入站整消息原子校验（`T01–T06`、`T08`、`SC04–SC09`）、状态事件分离（`POST /v1/source-state` 不伪造历史测量、不覆盖 `latest.value` 及 `value_sampled_at_ms`）与北向 `VerifiedScopeContext` 隔离、URI 段边界安全及游标绑定（`SC01–SC03`、`T10`、`NB03–NB04`、`NB07–NB09`）全部通过自动化测试验证。
- **设计假设與边界说明（不夸大为已完成的现场联调）**：
  1. 本次 F1 & D00 验证使用模拟网关报文与纯内存状态机核验契约与算法语义，**未连接真实 PLC/仪表或生产现场网关**（真实网关协议与版本确认属 F4 范畴）。
  2. DuckDB 持久事务与进程强杀恢复属 F0/F2 范畴，WAL 磁盘同步与订阅流控属 F3/F6 范畴，不在本报告中宣称。

### 2.2 输入数据来源与安全隔离
- 所有测试输入（租户 `ten-8iic`、网关 `gateway-01`、设备 `dev-oven-02` / `pump-001` / `pump-b01` / `pump-ab01`、寄存器 `40001`、工程值 `245.6 °C` / `0.42 MPa`）均为脱敏虚构数据，不包含任何真实产线 IP、密钥或客户设备参数。

---

## 3. 测试步骤与可复现命令

```bash
# 1. 生成黄金测试向量 test_vectors.json
CARGO_TARGET_DIR=/tmp/iotfast-target-f1d00 cargo run \
  --manifest-path feasibility/f1_d00_contract/Cargo.toml \
  --bin generate_vectors

# 2. 运行 Python 3 独立实现交叉验证 Rust 生成的黄金向量
python3 feasibility/f1_d00_contract/verify_vectors.py

# 3. 运行完整契约与边界向量测试套件（共 12 个集成测试函数，覆盖全部必测向量）
CARGO_TARGET_DIR=/tmp/iotfast-target-f1d00 cargo test \
  --manifest-path feasibility/f1_d00_contract/Cargo.toml
```

---

## 4. 期望、实际结果与测试向量核对明细

### 4.1 跨语言黄金测试向量核验（Rust vs Python 3）

文件位置：`feasibility/f1_d00_contract/test_vectors.json`  
核验脚本：`feasibility/f1_d00_contract/verify_vectors.py`

| 黄金向量 ID | 场景说明 | Rust & Python `payload_hash` | 关键 `observation_id` | 跨语言一致性 |
|---|---|---|---|---|
| `GV-01-BASELINE-T01` | 单点 `zone1_temp = 245.6`（`replayed=false`，`X-Request-Id=req-001`） | `sha256:dc89fe0d45b22e9d33aa5982077fc0683a6963d74c755239ede3a857527de630` | `zone1_temp`: `obs-hzg4jz6lu5qzfhygt5o3wtyrpi` | 100% 一致（通过） |
| `GV-02-REPLAY-INVARIANCE-SC04` | 同 GV-01，但 `replayed=true`、`X-Request-Id=req-retry-999` | `sha256:dc89fe0d45b22e9d33aa5982077fc0683a6963d74c755239ede3a857527de630`（与 GV-01 完全相等） | `zone1_temp`: `obs-hzg4jz6lu5qzfhygt5o3wtyrpi` | 100% 一致（通过） |
| `GV-03-TAMPERED-VALUE-SC05` | 同 `message_key`，温度由 `245.6` 篡改为 `245.7` | `sha256:f7a5704efc9e03ba5e60b53287e3b83baffd5753fc5d076df72be750b7f938fe`（与 GV-01 不同，触发 `409`） | `zone1_temp`: `obs-hzg4jz6lu5qzfhygt5o3wtyrpi` | 100% 一致（通过） |
| `GV-04-MULTIPOINT-ORDER-A` | 多点乱序输入 `[zone2_temp(250.0), conveyor_speed_rpm(1200), alarm_active(false)]` | `sha256:4b080e9213620a2dfb7e95e951274e441c8426706ece6bce44f2717af98944dc` | 3 个点各自独立确定性派生 | 100% 一致（通过） |
| `GV-05-MULTIPOINT-ORDER-B` | 同 GV-04，但输入数组逆序且 `replayed=true` | `sha256:4b080e9213620a2dfb7e95e951274e441c8426706ece6bce44f2717af98944dc`（与 GV-04 完全相等） | 与 GV-04 完全相等 | 100% 一致（通过） |
| `GV-06-JCS-FLOAT-AND-STRING-EDGES` | RFC 8785 边界：`-0.0 -> 0`、`0.42`、`1e-7`、转义文本、`u64::MAX` 序列号 | `sha256:c5daef460795004023469e7b3fbbd12bfdf8af52621f6af594e67215167465f9` | 4 个点各自独立确定性派生 | 100% 一致（通过） |

### 4.2 契约与边界测试向量（`T01–T06`, `T08`, `T10`, `SC01–SC09`, `NB03–NB04`, `NB07–NB09`）

| 向量编号 | 验证内容与输入条件 | 期望结果 | 实测结果 | 状态 |
|---|---|---|---|---|
| **T01** | 绑定含 `factor: 0.1`、`normalization_owner: "gateway"`，网关上报工程值 `245.6`；另测配置 `server_scale_factor` 时的校验 | 引擎存储 `245.6` 不二次换算，保留 `br-oven-12`；禁止配置服务端二次倍率 | `latest` 与 `history` 均为 `245.6`，`binding_revision_id="br-oven-12"`；违规绑定返回 `422 DOUBLE_SCALING_FORBIDDEN` | 通过 |
| **T02** | 单消息含合法 `zone1_temp=245.6` 与未知属性 `zone9_temp=180.0` | 整条消息返回 `422 VALIDATION_FAILED`，合法点不得产生任何副作用 | 返回 `422`（`fields: points[1].property, issue: UNKNOWN_PROPERTY`），`history_count=0`，`zone1_temp` 保持 `null / no_observation` | 通过 |
| **T03 / SC04** | 同 `message_key` 重传，`replayed: false -> true`，`points` 顺序反转，`X-Request-Id` 改变 | `payload_hash` 与 `observation_id` 不变，返回 `200 duplicate` 及原 `ingest_id`，历史只写一次 | 首次 `202 accepted_durable`（`ing-000001`），重试 `200 duplicate`（`ing-000001`），历史记录数保持 2 条不重复 | 通过 |
| **T04 / SC05** | 同 `message_key` 将 `zone1_temp` 从 `245.6` 改为 `245.7` | 摘要不同，返回 `409 IDEMPOTENCY_CONFLICT`，不覆盖原值 | 返回 `409 IDEMPOTENCY_CONFLICT`（`retryable=false`），`latest` 与历史保持 `245.6` | 通过 |
| **T05 / SC06** | 先收 `10:00:02`（`25.0`），后补传 `10:00:01`（`24.0`）；同毫秒收 `source_seq="1000"`（`25.0`）与 `"999"`（`24.0`） | 历史保留全部合法观测，`latest` 保持 `25.0`；`source_seq` 按 `u64` 比较 `"1000" > "999"` | 历史保留两条观测，`latest` 保持 `25.0` 且采样时间为 `10:00:02`；同毫秒下 `"1000"` 胜出 | 通过 |
| **T06 / SC08 / NB04** | 上报 `outlet_pressure=0.42`（`1793869100120`）后，在 `1793869200000` 发送 `iotfast.source_state.v1` 超时事件 | `/view` 与 `/shadow` 中 `quality="bad"`、`quality_observed_at_ms=1793869200000`，`value=0.42` 与 `value_sampled_at_ms=1793869100120` 不变，历史不新增伪造测量行 | `history_count` 保持 1，`quality_events` 新增 1 条，`value=0.42`、`value_sampled_at_ms=1793869100120`、`quality="bad"`、`quality_observed_at_ms=1793869200000` | 通过 |
| **SC09 / NB03** | 未上报/未绑定属性查看 `/view`；向 `/v1/telemetry` 发送 `value: null` | `/view` 返回 `value: null, quality: "unknown", observation_state: "no_observation"`（不填 `0` 或 `false`）；测量传 `null` 返回 `422` | 未上报及未绑定属性均为 `null / unknown / no_observation`；测量传 `null` 返回 `422 INVALID_POINT_VALUE` | 通过 |
| **SC07 / T08** | `source_at_ms` 超前服务端时间 2 小时；激活新绑定 `br-oven-13` 后收到无法证明旧窗口的 `br-oven-12` 报文 | 未来漂移返回 `422 CLOCK_SKEW_EXCEEDED` 且不污染 `latest`；旧绑定冲突返回 `409 REVISION_CONFLICT` | 分别返回 `422 CLOCK_SKEW_EXCEEDED` 与 `409 REVISION_CONFLICT`，`latest` 与 `history` 零污染 | 通过 |
| **SC01–SC03 / T10** | 无凭证请求；`uns://8iic/plant-a/` 访问 `uns://8iic/plant-ab/pump-01`；仅授权产线 1 的服务单点/批量查询产线 2 设备 | 无凭证返回 `401 UNAUTHENTICATED`；`plant-ab` 被段边界拦截返回 `404`；单点与批量越权统一返回 `404 RESOURCE_NOT_FOUND`；目录检索先过滤后匹配 | 全部按契约返回 `401` / `404`，批量查询含越权资源时整批拒绝不静默漏点，目录搜索不泄露产线 2 信息 | 通过 |
| **NB07–NB09** | `[1000, 2000)` 半开区间与同毫秒分页；跨模型版本 `MPa` vs `kPa` 聚合；跨 Scope 重放 `next_cursor` | 半开区间含头不含尾、同毫秒按 `observation_id` 稳定翻页；跨单位聚合返回 `422 CROSS_VERSION_INCOMPATIBLE`；跨 Scope 游标重放返回 `422 VALIDATION_FAILED` | `[1000, 2000)` 精确返回 3 条并稳定分为 `2+1` 页；跨版本聚合返回 `422 CROSS_VERSION_INCOMPATIBLE`；跨 Scope 游标返回 `422 CURSOR_SCOPE_MISMATCH` | 通过 |

---

## 5. 结论、限制与后续任务

- **验证结果**：**通过（PASS）** — 12 项 Rust 契约测试与 6 组黄金测试向量的 Rust/Python 交叉核验全部通过（100% 匹配）。
- **交付产物清单**：
  - `feasibility/f1_d00_contract/Cargo.toml`
  - `feasibility/f1_d00_contract/src/lib.rs`
  - `feasibility/f1_d00_contract/src/bin/generate_vectors.rs`
  - `feasibility/f1_d00_contract/tests/contract_vectors.rs`
  - `feasibility/f1_d00_contract/verify_vectors.py`
  - `feasibility/f1_d00_contract/test_vectors.json`
  - `docs/development/evidence/f1-d00-contract-verification-report.md`
- **后续衔接任务**：
  1. 在 D01/D05/D07 正式工程实现中直接复用本验证固定的 `sha256-jcs-v1` 与 `observation_id` 派生实现及 `test_vectors.json` 回归测试。
  2. 在 F4 阶段取得现有采集网关真实报文样本后，核实其是否原生支持 `binding_revision_id` 与独立 `iotfast.source_state.v1` 事件或需通过本地持久桥接器转换。
