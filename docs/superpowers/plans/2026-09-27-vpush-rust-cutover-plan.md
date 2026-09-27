# vpush Rust 完整功能切换实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补齐 Rust 与 Python `1.12.277` 的生产功能差异，在隔离 staging 完成验收后，准备可回滚的短暂冻结切换。

**Architecture:** 保持 Rust 现有 Axum + SQLx + SQLite 结构，按用户核心、管理功能、外部集成三个边界逐组补齐。生产 Python 在最终切换前保持唯一写入者；所有迁移和行为测试先使用数据库副本。

**Tech Stack:** Rust 2021、Axum 0.8、SQLx SQLite、Tokio、Docker ARM64、Python FastAPI 生产基线、SQLite online backup。

---

## Task 1: 建立可靠的兼容性基线

**Files:**
- Create: `tools/route_inventory.py`
- Create: `docs/superpowers/artifacts/route-inventory.md`
- Create: `docs/superpowers/artifacts/config-inventory.md`
- Read-only source: production `/opt/vpush/src/app/api.py`, Rust `src/main.rs`, all `src/*.rs`

- [x] **Step 1: 提取 Python 路由**

  在生产主机只读读取 `api.py`，解析 `@router.get/post/put/delete/patch`，输出方法、完整 `/api` 路径、认证依赖和源码行号。多行装饰器和带 `dependencies` 的声明必须保留。

- [x] **Step 2: 提取 Rust 路由**

  不用脆弱的单行正则；按 Rust `.route("...")` 字符串边界提取链式 `get/post/put/delete/patch`，并把同一路径的多个方法合并。对无法解析的声明直接报错，不静默丢弃。

- [x] **Step 3: 生成差异报告**

  报告分成 exact match、Python-only、Rust-only、同路径不同方法，并按用户核心/管理/集成分组。每条 Python-only 必须标记“补齐”或“明确不适用”，不能直接当作缺失实现。

- [x] **Step 4: 生成配置变量报告**

  只记录变量名、生产是否存在、Rust 使用位置、必需性和验证状态；禁止记录值。检查生产 `.env` 与容器环境变量，但不复制文件。

- [x] **Step 5: Commit**

  ```bash
  git add tools/route_inventory.py tools/test_route_inventory.py docs/superpowers/artifacts
  git commit -m "建立 Python Rust 兼容性基线"
  ```

## Task 2: 补齐用户核心 API

**Files:**
- Modify: `src/main.rs`
- Modify: `src/db.rs`
- Modify: `src/webpush.rs`
- Modify: `src/auth.rs`
- Test: existing `src/main.rs` and `src/db.rs` test modules

- [x] **Step 1: 为每个 Python 用户核心接口建立失败测试**

  覆盖 `/api/me`、WebPush、Android device、订阅 `PUT/DELETE`、隐藏图片、分类/KOL 详情、用户 Feed、新闻详情和文章图片。测试只使用临时 SQLite，不调用外部服务。

- [x] **Step 2: 对照 Python 请求/响应契约实现最小 Rust handler**

  复用现有 `require_user`、`require_admin`、`Db` 方法和 JSON 类型；错误状态码与字段名按 Python 基线对齐，不新增平行认证系统。

- [x] **Step 3: 运行核心测试**

  ```bash
  cargo test --all-targets
  ```

  预期：全部通过，且新增测试覆盖每个补齐接口至少一个成功和一个拒绝分支。

- [x] **Step 4: Commit**

  ```bash
  git add src/main.rs src/db.rs src/webpush.rs src/auth.rs
  git commit -m "补齐 Rust 用户核心接口"
  ```

## Task 3: 补齐管理、新闻、备份和代理 API

**Files:**
- Modify: `src/main.rs`
- Modify: `src/db.rs`
- Modify: `src/news.rs`
- Modify: `src/backup.rs`
- Modify: `src/proxy_admin.rs`
- Modify: `src/imgbed.rs`
- Test: corresponding inline test modules

- [ ] **Step 1: 按基线报告写管理接口失败测试**

  覆盖注册码、用户/KOL/分类、标签、新闻 sources/feeds/articles、Turnstile、图床、proxy pools/routes、备份 restore 和 test-push。所有外部副作用用测试配置关闭或 mock。

- [ ] **Step 2: 实现请求/响应和权限契约**

  管理接口必须统一走 `require_admin`；删除、批量更新、恢复接口先在临时数据库验证事务和失败回滚。

- [ ] **Step 3: 验证数据库副作用**

  对每个写接口检查预期表变化、重复调用幂等性和错误输入不产生部分写入。

- [ ] **Step 4: Commit**

  ```bash
  git add src/main.rs src/db.rs src/news.rs src/backup.rs src/proxy_admin.rs src/imgbed.rs
  git commit -m "补齐 Rust 管理与运维接口"
  ```

## Task 4: 补齐外部集成和配置映射

**Files:**
- Modify: `src/feishu*.rs`
- Modify: `src/ima*.rs`
- Modify: `src/cicc.rs`
- Modify: `src/push.rs`
- Modify: `src/xueqiu.rs`, `src/weibo.rs`, `src/twitter.rs`, `src/llm.rs`
- Modify: `compose.yaml`
- Create: `docs/superpowers/artifacts/config-mapping.md`

- [ ] **Step 1: 为缺失的飞书/IMA/CICC/OAuth 路由补失败测试**

  测试配置缺失、无权限、过期会话、外部服务失败和成功响应；禁止真实发送消息或启动采集任务。

- [ ] **Step 2: 对齐配置变量**

  为每个变量明确 Rust 名称、来源、默认值、启动时是否必需和运行时校验。不要把生产 `.env` 加入 repo；部署时只在 ARM 本机写入已批准的变量。

- [ ] **Step 3: 验证外部副作用开关**

  `WEB_ALLOW_REGISTER=0`、`VPUSH_FETCH=0`、推送关闭时，应用不得启动抓取或发信；每个开关增加测试或日志证据。

- [ ] **Step 4: Commit**

  ```bash
  git add src compose.yaml docs/superpowers/artifacts/config-mapping.md
  git commit -m "补齐 Rust 外部集成配置映射"
  ```

## Task 5: 数据迁移和 staging 全流程验收

**Files:**
- Create: `scripts/rehearse-migration.sh`
- Create: `scripts/staging-smoke.sh`
- Modify: `README.md`

- [ ] **Step 1: 在生产只读生成 online backup**

  使用生产容器内 Python `sqlite3.Connection.backup()` 写入 `/data` 下临时副本；先说明命令和影响，副本通过 `quick_check` 后再传输。

- [ ] **Step 2: 在 ARM 隔离目录执行迁移**

  使用独立 Compose、独立数据库和 `127.0.0.1` 端口；不使用 `docker compose run` 共享主 staging SQLite，不触碰现有 staging 数据。

- [ ] **Step 3: 运行 staging smoke**

  检查 health/version、未认证拒绝、登录/权限、订阅、文章/新闻、管理接口、备份和集成开关；记录状态码、响应契约和数据库计数。

- [ ] **Step 4: 清理演练资源**

  只删除本次演练容器、数据库 sidecar 和临时 Compose；保留主 staging 和其他 ARM 服务。`xincai` 故障单独记录，不擅自修复。

- [ ] **Step 5: Commit**

  ```bash
  git add scripts README.md
  git commit -m "增加迁移演练和 staging 验收脚本"
  ```

## Task 6: 切换演练与正式切流准备

**Files:**
- Create: `docs/superpowers/artifacts/cutover-runbook.md`
- Create: `docs/superpowers/artifacts/rollback-runbook.md`
- Do not modify production until separately approved

- [x] **Step 1: 记录生产状态**

  只读记录 Python 镜像 digest、容器健康、Caddy upstream、数据库文件/WAL、Rust 镜像 digest 和配置状态。

- [x] **Step 2: 演练最终同步**

  在隔离环境模拟短暂写入冻结、SQLite backup、Rust 迁移、健康检查和失败回滚；记录每一步命令、预期输出和停止条件。

- [x] **Step 3: 写正式 runbook**

  明确冻结范围、备份校验、启动顺序、Caddy 变更、验收 URL、回滚顺序和责任边界。红线操作全部标为“需用户明确批准”。

- [x] **Step 4: 完成最终审查**

  只有路由、配置、业务验收、外部依赖和回滚演练全部通过，才提交正式切流请求；否则保持 Python 流量不变。

## Verification

每个任务提交前运行：

```bash
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
git diff --check
```

正式切换前额外要求：Rust ARM release build、迁移副本 `quick_check=ok`、核心计数一致、staging smoke 全通过、生产写入冻结和回滚演练通过。任何不满足项都阻止切流。
