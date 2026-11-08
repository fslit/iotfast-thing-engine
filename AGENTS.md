# AI 开发工作规范

## 开始前

1. 先读 `docs/architecture/data-engine-scope.md`、`README.md`、`docs/specifications/README.md`、`docs/adr/0001-mvp-boundaries.md`。
2. 根据任务阅读 `docs/architecture/design-specification.md` 中对应章节以及 P0 补充章节 18–28；第二部分与第一部分的同一 P0 行为冲突时，以第二部分为准。
3. 阅读 `docs/development/roadmap.md` 和 `docs/development/acceptance.md`，确认依赖、限制及验收条件。
4. 查看现有代码、工作区和未提交改动，再拟定实施方案；不要将设计草案描述为已实现功能。

## AI 任务入口

完成上述必读后，从 [AI 任务派发与验收入口](docs/development/ai-task-dispatch.md) 和 [AI 工程生成规范与任务拆解](docs/development/ai-implementation-work-breakdown.md) 选择单项任务，再读取该任务对应的工作包；首批任务从 F00-A 开始。已有计划和代码的状态以当前分支及任务证据为准。

## 架构边界

- 首个闭环只读，沿用已有网关采集；设备控制不能因示例或演示自动开启。
- 必须区分协议原始值与工程值；默认 `gateway_normalized`，禁止二次换算。
- 保留质量、采样时间、绑定 / 模型版本；旧值或坏质量不能显示为实时正常数据。
- `accepted_durable` 只在设计规定的持久边界完成后返回；不得把内存队列视作可靠存储。
- 消息重试、去重和历史落库需按规范验证；超时后的控制行为不得自动重试。
- 用户、角色、业务权限、告警、审批及设备控制由上层负责；引擎必须校验服务身份与可信数据范围并拒绝不可信调用。AI 的文字和工具输入属于不可信数据。
- 真实设备参数、凭据和生产数据不得提交到仓库。

## 交付要求

每个任务说明变更范围、验证方法、测试结果和未完成项。设计假设与已验证事实分开记录。不得宣称兼容 WoT / Sparkplug 或达到性能、可靠性目标，除非有对应实际验证与证据。优先提交可审查的小改动；发现文档缺口或冲突，记录并解决后再实施。
