# ETF 溢价双向穿越提醒 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans (recommended) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 ETF 溢价提醒扩展为可独立配置“涨破高位”和“跌破低位”的双向穿越提醒，并保持旧单阈值数据兼容。

**Architecture:** 在现有 `etf_premium_alerts` 行上增加跌破配置、双方向基线和最近触发方向；保留现有 ETF API 路径，PUT 一次保存两组设置。行情通过现有价格、IOPV、交易时段和 120 秒新鲜度校验后，分别用 SQLite 条件更新领取涨破或跌破事件。设置页继续使用现有行式 UI，每行包含两个方向区块。

**Tech Stack:** Rust、Axum、SQLite/SQLx、Tokio、serde_json、浏览器 ES modules、Node `node:test`。不增加依赖。

---

## 文件结构

- Modify: `/Users/kale/vpush-rust/src/db.rs`：创建时的 ETF 提醒表定义、增量列迁移和数据库初始化调用。
- Modify: `/Users/kale/vpush-rust/src/etf_premium.rs`：双方向设置读写、基线/claim、推送文本、行情可提醒判断和单测。
- Modify: `/Users/kale/vpush-rust/src/main.rs`：双方向 PUT 请求体、校验、错误信息及 API 集成测试。
- Modify: `/Users/kale/vpush-rust/static-assets/views/push-settings.js`：两组开关/阈值草稿、保存和错误状态。
- Modify: `/Users/kale/vpush-rust/static-assets/views/etf-premium.js`：最近触发方向展示，并移除错误的“不会发送”状态。
- Modify: `/Users/kale/vpush-rust/static-assets/style.css`：双方向设置区块的桌面/窄屏布局。
- Create: `/Users/kale/vpush-rust/tools/etf_premium_render_test.mjs`：前端渲染和请求体的最小 Node 测试。
- Modify: `/Users/kale/vpush-rust/static-assets/index.html`、`/Users/kale/vpush-rust/static-assets/sw.js`：按仓库现有流程更新变更模块的资源指纹。

## Task 1: 先写后端失败测试

**Files:** Modify `/Users/kale/vpush-rust/src/etf_premium.rs`; Modify `/Users/kale/vpush-rust/src/main.rs`。

- [ ] **Step 1: 为阈值校验写失败测试**

在 `src/etf_premium.rs` 测试模块增加以下断言，先按目标签名调用尚不存在的 `valid_settings`：

```rust
assert!(valid_settings("513100", true, 8.0, true, -2.0));
assert!(!valid_settings("513100", true, 101.0, false, 0.0));
assert!(!valid_settings("513100", false, 8.0, true, -101.0));
assert!(!valid_settings("513100", true, 2.0, true, 3.0));
assert!(valid_settings("513100", false, 0.0, true, -2.0));
assert!(!valid_settings("000001", true, 8.0, false, 0.0));
```

- [ ] **Step 2: 运行阈值测试确认失败**

运行：

```bash
cargo test etf_premium::tests::valid_settings -- --nocapture
```

预期：编译失败，提示 `valid_settings` 尚未定义。

- [ ] **Step 3: 为双方向 API 形状写失败测试**

在 `src/main.rs` 现有 ETF API 测试区域增加请求体断言，目标 JSON 为：

```json
{
  "above": {"enabled": true, "threshold_pct": 8.0},
  "below": {"enabled": true, "threshold_pct": -2.0}
}
```

测试 GET 响应包含 `above.enabled`、`above.threshold_pct`、`below.enabled`、`below.threshold_pct`；旧数据库记录的 `enabled=true, threshold_pct=13.58` 映射到 `above`，且 `below.enabled=false`。

- [ ] **Step 4: 为穿越状态写失败测试**

在 `src/etf_premium.rs` 测试模块增加异步测试，沿用已有 `Db::open(Path::new(":memory:"))` 和 `save_setting` 测试样式，断言以下序列：

```rust
// high=8: 7 -> 8 不触发，8 -> 8.01 触发，9 -> 9.01 不重复，7.99 后再 8.01 再触发
// low=-2: -1 -> -2 不触发，-2 -> -2.01 触发，-3 -> -3.01 不重复，-1.99 后再 -2.01 再触发
// 第一次有效观察只建立 baseline，即使当前值已在阈值外也不触发
```

每次评估后检查 `latest_alert` 的方向、当前溢价率和阈值；同时断言关闭方向不触发。

- [ ] **Step 5: 运行新增测试确认失败**

运行：

```bash
cargo test etf_premium::tests -- --nocapture
```

预期：新增测试因 API 结构、参数和双方向状态尚未实现而失败；现有测试失败也要记录具体断言，下一任务一并更新为新语义。

## Task 2: 数据库迁移与双方向设置 API

**Files:** Modify `/Users/kale/vpush-rust/src/db.rs`; Modify `/Users/kale/vpush-rust/src/etf_premium.rs`; Modify `/Users/kale/vpush-rust/src/main.rs`。

- [ ] **Step 1: 扩展新库表定义**

在 `/Users/kale/vpush-rust/src/db.rs` 的 `CREATE TABLE IF NOT EXISTS etf_premium_alerts` 中加入：

```sql
below_enabled INTEGER NOT NULL DEFAULT 0,
below_threshold_pct REAL NOT NULL DEFAULT 0.0,
below_threshold INTEGER NOT NULL DEFAULT 0,
above_initialized INTEGER NOT NULL DEFAULT 0,
below_initialized INTEGER NOT NULL DEFAULT 0,
last_direction TEXT NOT NULL DEFAULT '',
```

跌破阈值约束使用 `CHECK (below_threshold_pct BETWEEN -100 AND 100)`；旧 `threshold_pct` 保持原有 `0..100` 约束。

- [ ] **Step 2: 添加增量列迁移**

新增 `ensure_etf_premium_columns(pool: &SqlitePool)`，用 `pragma_table_info('etf_premium_alerts')` 判断列是否存在，逐列调用现有 `add_column`：

```rust
("below_enabled", "INTEGER NOT NULL DEFAULT 0")
("below_threshold_pct", "REAL NOT NULL DEFAULT 0.0")
("below_threshold", "INTEGER NOT NULL DEFAULT 0")
("above_initialized", "INTEGER NOT NULL DEFAULT 0")
("below_initialized", "INTEGER NOT NULL DEFAULT 0")
("last_direction", "TEXT NOT NULL DEFAULT ''")
```

在数据库初始化调用链中紧接现有列迁移函数调用它。不要重建表，不要改写已有 `enabled` 或 `threshold_pct`。

- [ ] **Step 3: 运行迁移相关测试**

运行：

```bash
cargo test db::tests -- --nocapture
```

预期：数据库测试通过；旧表缺列时启动初始化后可以查询上述六列，重复初始化不报 `duplicate column name`。

- [ ] **Step 4: 实现双方向校验和设置读取**

在 `/Users/kale/vpush-rust/src/etf_premium.rs` 将校验函数改为：

```rust
pub fn valid_settings(
    symbol: &str,
    above_enabled: bool,
    above_threshold: f64,
    below_enabled: bool,
    below_threshold: f64,
) -> bool
```

校验 ETF 代码、有限数值、涨破 `0..=100`、跌破 `-100..=100`；两个方向同时启用时要求 `below_threshold < above_threshold`。

`settings()` 查询两组字段，返回：

```json
{"symbol":"513100","name":"纳指ETF国泰","above":{"enabled":true,"threshold_pct":13.58},"below":{"enabled":false,"threshold_pct":0.0}}
```

没有数据库行时默认 `above.enabled=false, above.threshold_pct=5.0, below.enabled=false, below.threshold_pct=0.0`。

- [ ] **Step 5: 实现原子双方向保存**

将 `save_setting` 改成接收四个方向参数，一条 `INSERT ... ON CONFLICT DO UPDATE` 同时写入两组设置。每个方向的配置变化分别清零对应 `*_threshold` 和 `*_initialized`；更新 `last_direction` 不需要清空，因为它仅代表历史最近触发。

保存语义必须是整组校验通过后一次提交，不能先保存 above 再保存 below。

- [ ] **Step 6: 更新 Axum 请求体和错误响应**

在 `/Users/kale/vpush-rust/src/main.rs` 用嵌套结构替换旧的 `EtfAlertIn`：

```rust
#[derive(Deserialize)]
struct EtfAlertSideIn { enabled: bool, threshold_pct: f64 }

#[derive(Deserialize)]
struct EtfAlertIn { above: EtfAlertSideIn, below: EtfAlertSideIn }
```

`save_etf_alert_settings` 调用 `valid_settings` 和新的 `save_setting`；校验失败返回 `400`，错误文本明确指出涨破 `0～100%`、跌破 `-100～100%` 以及低阈值必须小于高阈值。保留认证、未知 ETF 和数据库错误路径。

- [ ] **Step 7: 运行 API 测试确认设置部分通过**

运行：

```bash
cargo test etf_premium::tests main::tests -- --nocapture
```

预期：设置读取/保存和旧数据兼容测试通过；只剩触发状态测试或旧消息文本断言需要下一任务处理。

## Task 3: 双方向行情触发与推送

**Files:** Modify `/Users/kale/vpush-rust/src/etf_premium.rs`。

- [ ] **Step 1: 让行情可提醒但保留新鲜度保护**

将 `Quote::alertable(&self, now)` 从固定 `false` 改为同时满足：`mainland_open(now)`、`fresh(now)`、价格和 IOPV 为正有限值、溢价率可计算。将 `render()` 的 `alerts_enabled` 绑定到同样的判断结果；`reference_type` 和“IOPV实时性待盘中验证”来源说明保留。

- [ ] **Step 2: 拆分方向状态更新**

将当前单方向 `claim` 改为带方向参数的条件更新，方向只允许 `above` 或 `below`。两条 SQL 的语义分别是：

```sql
-- above：rate <= threshold 时 above_threshold=0；rate > threshold 时从 0 置 1 并领取
-- below：rate >= threshold 时 below_threshold=0；rate < threshold 时从 0 置 1 并领取
```

每个方向首次有效观察时只写入 `*_initialized=1` 和当前侧状态，不领取事件。只有已初始化且从阈值内侧穿越到外侧才把 `last_triggered_pct`、`last_threshold_pct`、`last_triggered_at`、`last_direction` 和 `delivery_status='pending'` 一起更新。

使用单条带条件的 SQLite `UPDATE` 保证同一用户、ETF、方向并发时只有一条领取成功。

- [ ] **Step 3: 更新评估用户查询和推送文本**

`evaluate()` 查询 `enabled=1 OR below_enabled=1` 的用户；对每个用户依次调用两个方向 claim。领取后读取对应的 `last_threshold_pct` 和 `last_direction`，按方向生成：

```text
【ETF溢价上涨穿越提醒】
【ETF溢价下跌穿越提醒】
```

正文包含 ETF、当前溢价率、对应阈值、触发时间、IOPV 值和 IOPV 更新时间。`mark_delivery` 以 `last_direction` 和触发时间限定更新，避免同一用户两个方向同时投递时互相覆盖状态。

- [ ] **Step 4: 更新最近触发返回值**

`latest_alert()` 查询 `last_direction` 并返回 `direction: "above" | "below"`；前端可直接用此字段显示“涨破/跌破”。保留 `delivery_status`。

- [ ] **Step 5: 更新并运行后端行为测试**

将原 `source_snapshot_and_sessions_guard_alerts` 改为断言新鲜有效报价可提醒、过期报价/缺 IOPV/非交易时段不可提醒。运行：

```bash
cargo test etf_premium::tests -- --nocapture
cargo test --locked --all-targets
```

预期：ETF 专项测试和全量 Rust 测试通过；首次基线、双方向穿越、去重、重新武装、负阈值、投递状态和用户隔离均有断言。

## Task 4: 设置页与市场页 UI

**Files:** Modify `/Users/kale/vpush-rust/static-assets/views/push-settings.js`; Modify `/Users/kale/vpush-rust/static-assets/views/etf-premium.js`; Modify `/Users/kale/vpush-rust/static-assets/style.css`。

- [ ] **Step 1: 改造设置页数据模型**

在 `renderEtfAlertSettings()` 中把单值草稿替换为：

```js
{ above: { enabled: boolean, threshold: string }, below: { enabled: boolean, threshold: string } }
```

旧响应缺少嵌套字段时临时按 `enabled`/`threshold_pct` 读取到 `above`，保证部署期间旧缓存不导致空白。

- [ ] **Step 2: 渲染两个可访问方向区块**

每个 ETF 行生成两个独立 `<fieldset>`：

```html
<fieldset class="etf-alert-side">
  <legend>涨破</legend>
  <label><input type="checkbox" data-etf-above-enabled> 启用</label>
  <input type="number" min="0" max="100" step=".01" data-etf-above-threshold>
</fieldset>
<fieldset class="etf-alert-side">
  <legend>跌破</legend>
  <label><input type="checkbox" data-etf-below-enabled> 启用</label>
  <input type="number" min="-100" max="100" step=".01" data-etf-below-threshold>
</fieldset>
```

保留每行一个保存按钮、保存中禁用当前行和 `role=status` 错误区域。提示文字改为“首次有效行情只建立基线，持续越界不会重复推送；涨破范围 0–100%，跌破范围 -100–100%”。

- [ ] **Step 3: 实现双方向草稿和保存校验**

输入事件分别更新 `draft.above` 或 `draft.below`。保存前将四个值转为数字并检查两方向范围和同时启用时的 `below < above`；失败时聚焦对应输入、保留草稿、不发请求。将请求体集中由一个导出的纯函数 `etfAlertPayload(aboveEnabled, aboveValue, belowEnabled, belowValue)` 生成，返回：

```js
{ above: { enabled: aboveEnabled, threshold_pct: aboveValue },
  below: { enabled: belowEnabled, threshold_pct: belowValue } }
```

成功后用返回的嵌套对象替换当前 ETF，失败后不删除 draft。

- [ ] **Step 4: 显示最近触发方向**

在 `/Users/kale/vpush-rust/static-assets/views/etf-premium.js` 中将最近触发文案改为：

```js
const direction = data.latest_alert.direction === "below" ? "跌破" : "涨破";
`最近触发：${name} · ${direction} ${rate}%（阈值 ${threshold}%）`
```

删除或改写 `alerts_enabled: false` 对应的“当前不发送提醒”文案；数据过期时仍显示状态而不推断提醒成功。

- [ ] **Step 5: 调整桌面和窄屏布局**

在现有 `.etf-alert-*` 样式基础上增加 `.etf-alert-sides` 和 `.etf-alert-side`，桌面使用两列方向区块，`600px` 以下改为单列；输入固定宽度 `82px`，保存按钮保持可见，不让长 ETF 名称挤压阈值输入。保留现有颜色、焦点轮廓和表单控件样式。

- [ ] **Step 6: 添加前端最小测试并运行**

创建 `tools/etf_premium_render_test.mjs`，从 `static-assets/views/push-settings.js` 导入 `etfAlertPayload`，用 Node 内置 `node:test` 验证：

```js
assert.deepEqual(etfAlertPayload(true, 8, true, -2), {
  above: { enabled: true, threshold_pct: 8 },
  below: { enabled: true, threshold_pct: -2 },
});
assert.match(readFileSync("static-assets/views/push-settings.js", "utf8"), /涨破/);
assert.match(readFileSync("static-assets/views/push-settings.js", "utf8"), /跌破/);
```

运行：

```bash
node --test tools/etf_premium_render_test.mjs
```

预期：测试通过且不引入第三方 DOM 或测试依赖。

## Task 5: 资源指纹、全量验证和提交

**Files:** Modify `/Users/kale/vpush-rust/static-assets/index.html`; Modify `/Users/kale/vpush-rust/static-assets/sw.js`；修改前述实现文件产生的指纹引用。

- [ ] **Step 1: 运行格式、专项和全量检查**

运行：

```bash
cargo fmt --all --check
cargo test --locked --all-targets
node --test tools/etf_premium_render_test.mjs
node --check static-assets/views/push-settings.js
node --check static-assets/views/etf-premium.js
```

预期：所有命令退出码为 0；`git diff --check` 无空白错误。

- [ ] **Step 2: 更新静态模块指纹**

使用现有指纹规则计算变更源文件的 SHA-256 前 12 位：

```bash
shasum -a 256 static-assets/views/push-settings.js | cut -c1-12
shasum -a 256 static-assets/views/etf-premium.js | cut -c1-12
```

只更新 `static-assets/index.html` import map 和 `static-assets/sw.js` 中两个模块的哈希字符串；服务端会将指纹 URL 映射到未指纹的源文件，不创建重复源文件。提交前用 `rg` 确认旧哈希只剩无关历史文件引用。

- [ ] **Step 3: 做浏览器验收**

启动本地 Rust 服务，在已登录浏览器中检查：

- 设置页加载两只 ETF 的两组控件，分别编辑涨破/跌破并刷新后保持。
- 非法高阈值、非法低阈值和低阈值不小于高阈值时，保存不发请求并保留草稿。
- 1440px 和 390px 窗口下无横向溢出；键盘焦点可到达两个开关、两个输入和保存按钮。
- 市场页最近触发记录显示“涨破”或“跌破”。
- 过期行情显示 stale/reference 状态，不显示已触发推断。

- [ ] **Step 4: 检查最终差异并提交**

运行：

```bash
git diff --check
git status --short
git diff --stat
```

确认只包含本功能的 Rust、前端、测试和指纹文件，不纳入既有 `src/ima_storage.rs`、`.superpowers/`、`static-assets/_thumb_cache_test.html` 或其他无关改动，然后提交：

```bash
git add src/db.rs src/etf_premium.rs src/main.rs static-assets/views/push-settings.js static-assets/views/etf-premium.js static-assets/style.css tools/etf_premium_render_test.mjs static-assets/index.html static-assets/sw.js
git commit -m "feat: add bidirectional ETF premium alerts"
```

## Self-review checklist

- [ ] 规格中的旧数据兼容对应 Task 2 的增量列迁移和旧字段映射。
- [ ] 涨破/跌破独立开关、范围和 `below < above` 校验对应 Task 2、Task 4。
- [ ] 首次基线、穿越、去重、重新武装、并发 claim 对应 Task 1、Task 3。
- [ ] 行情新鲜度、全局推送、免打扰和渠道投递沿用现有路径，对应 Task 3 和测试。
- [ ] 最近触发方向和错误文案对应 Task 3、Task 4。
- [ ] 旧资产指纹、Rust 全量测试、Node 测试、浏览器窄屏验收对应 Task 5。
- [ ] 计划中没有 `TBD`、`TODO`、`FIXME` 或未定义的函数名；实现名称统一使用 `valid_settings`、四参数 `save_setting`、带方向参数的 `claim`。
