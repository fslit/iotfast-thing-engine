# D09 离线合成网关 Fixture 回放、断网缓存补传与绑定切换对账报告

- **任务卡**：`D09`（离线网关回放、断网缓存补传与绑定切换对账验证）
- **证据命名空间**：`D09-OFFLINE-GATEWAY-FIXTURE`
- **日期**：`2026-11-10`
- **测试文件**：[`prototype/tests/d09_gateway_replay_fixture_test.rs`](../../../prototype/tests/d09_gateway_replay_fixture_test.rs)
- **声明（非现场签收）**：**本报告为离线合成网关 Fixture 验证证据，不替代真实现场网关签收；真实现场网关接入仍待 D09 现场门禁（[决策台账 §4](../specification-decisions.md)、[AI 执行任务包 §6.2](../ai-execution-plan.md)）确认。**

---

## 1. 验证目标与范围

基于 `iotfast_thing_engine::server::ServerHandle`（兼容沙箱双工流与本地 TCP HTTP/1.1 传输层），在不引入 Modbus 主站模拟器或设备控制路由的前提下，对同一边缘网关 `gw-edge-01` 采集下的两台同物模型水泵设备（`pump-101`、`pump-102`）进行端到端只读回放与对账验证：

1. **同物模型双设备隔离与禁止二次换算（T01）**：
   - 物模型 `pump-model-r1`（`1.0.0`）包含动态测量属性 `outlet_pressure`（`float64`, `MPa`）、`motor_current`（`float64`, `A`）与静态铭牌属性 `rated_power`（`float64`, `kW`）。
   - 反例：当 `normalization_owner = "gateway"` 时传入 `server_scale_factor = 0.01`，服务端拒绝并返回 `422 VALIDATION_FAILED`。
   - 正例：`pump-101`（`outlet_pressure = 1.25 MPa`, `motor_current = 28.5 A`, `rated_power = 45.0 kW`）与 `pump-102`（`outlet_pressure = 2.40 MPa`, `motor_current = 41.2 A`, `rated_power = 55.0 kW`）经同一网关 `gw-edge-01` 上报后，`GET /v1/devices/{id}/view` 工程值原样保留、零串台。
2. **网关断连状态上报与质量/新鲜度分离（Q04 / C02 / SC08 / T06）**：
   - 网关上报 `POST /v1/source-state`（`spec = "iotfast.source_state.v1"`, `connection_state = "disconnected"`, `point_states = []`），返回 `202 accepted`。
   - 北向 `GET /v1/devices/pump-101/view` 返回 `source_connection_state = "disconnected"`；动态属性 `outlet_pressure` 的 `freshness` 置为 `"stale"`，但 `quality` 保持 `"good"`、原采样值 `1.25` 与 `value_sampled_at_ms = 100000` 不被篡改；静态属性 `rated_power` 的 `freshness` 保持 `"static"`。
3. **断网缓存乱序补传、重复重传幂等与历史对账（T03 / T04 / T05）**：
   - 网关重连并上报 `connection_state = "connected"` 后，将断网窗口 `[110000, 150000]` 缓存的 5 条历史报文（`msg-p101-bf-01` 至 `msg-p101-bf-05`，`source_seq = "2".."6"`）按乱序 `[130000, 110000, 150000, 120000, 140000]` 携带 `delivery.replayed = true` 补传，全部返回 `202 accepted_durable`。
   - 对其中 2 条报文（`msg-p101-bf-01`、`msg-p101-bf-05`）原样重复重传，返回 `200 duplicate`（`T03`）；对 `msg-p101-bf-05` 篡改数值为 `1.99` 重传，返回 `409 IDEMPOTENCY_CONFLICT`（`T04`）。
   - 对账结果：
     - `GET /v1/devices/pump-101/view` 的 `latest_value` 保持最大 `(source_at_ms = 150000, source_seq_u64 = 6)` 的观测值 `1.68 MPa`，未被更晚到达的 `140000`（`1.52 MPa`）倒退覆盖（`T05`）。
     - `POST /v1/history/query`（区间 `[100000, 160000)`）完整找回 6 个采样点（1 条实时 + 5 条断网补传），按 `source_at_ms` 升序排列，100% 落库、0 重复。
4. **绑定版本切换与旧时段已提交消息重试幂等（Q09 / R08 / T08）**：
   - `pump-101` 通过 `gateway_ack` 激活新绑定 `bind-pump-101-r2`，原 `bind-pump-101-r1` 转为 `superseded`。
   - 旧绑定时段已成功落库的 `msg-p101-bf-03` 携带原 `binding_revision_id = "bind-pump-101-r1"` 再次重传，幂等返回 `200 duplicate`（`R08`）。
   - 全新 `message_id = "msg-p101-stale-bind-new"` 携带已失效的 `bind-pump-101-r1` 上报，被拒绝并返回 `409 REVISION_CONFLICT`（`T08`）。
   - 携带新绑定 `bind-pump-101-r2` 的实时报文 `msg-p101-r2-live-01` 正常返回 `202 accepted_durable`。

---

## 2. 对账汇总表

| 场景编号 | 规范向量 | 输入 / 激励 | 预期状态码与语义 | 实测结果 |
|---|---|---|---|---|
| 场景 (a)-反例 | `T01` | `normalization_owner="gateway"` 且 `server_scale_factor=0.01` | `422 VALIDATION_FAILED` | 通过（`422`） |
| 场景 (a)-正例 | `T01` | `pump-101` (`1.25 MPa`) 与 `pump-102` (`2.40 MPa`) 同网关上报 | `202 accepted_durable`，双设备视图隔离、无二次换算 | 通过 |
| 场景 (b) | `Q04 / SC08 / T06` | `POST /v1/source-state` (`connection_state="disconnected"`) | 动态点 `freshness="stale"`、`quality="good"`；静态点 `freshness="static"` | 通过 |
| 场景 (c)-补传 | `T03 / T05` | 5 条断网缓存 (`110k..150k`) 乱序 `replayed=true` 补传 + 2 条重传 | 首次 `202 accepted_durable`，重传 `200 duplicate`；历史 6 点 0 重复，`latest=1.68` | 通过 |
| 场景 (c)-冲突 | `T04` | 同 `message_id="msg-p101-bf-05"` 篡改 `value=1.99` | `409 IDEMPOTENCY_CONFLICT` | 通过（`409`） |
| 场景 (d)-切换 | `Q09 / R08 / T08` | 切换至 `bind-pump-101-r2` 后重试旧 `msg-p101-bf-03` vs 新消息用旧绑定 | 旧已落库消息 `200 duplicate`；新消息用旧绑定 `409 REVISION_CONFLICT` | 通过 |

---

## 3. 执行命令与验证输出

```bash
cargo test --locked --manifest-path prototype/Cargo.toml --test d09_gateway_replay_fixture_test
```

- **退出码**：`0`
- **标准输出摘要**：
  ```text
  running 1 test
  test d09_offline_gateway_disconnect_backfill_and_binding_switch_reconciliation ... ok

  test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
  ```

---

## 4. 门禁与未完成项说明

- **已完成**：在受控离线测试环境中通过合成网关 Fixture 完整验证 `T01 / T03 / T04 / T05 / T06 / T08 / Q04 / Q09 / R08` 的 HTTP 全链路闭环对账。
- **未完成（阻塞于现场输入门禁）**：真实工业网关硬件/固件脱敏报文、现场点表与只读联调窗口尚未由网关负责人与现场负责人提供，本报告不构成 D09 现场生产验收签认。
