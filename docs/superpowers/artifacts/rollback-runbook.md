# vpush Rust 回滚 Runbook

状态：仅供审阅，默认不执行生产回滚。

## 触发条件

满足任意一项即回滚：

- Rust healthcheck 失败或容器反复重启。
- Caddy 5xx、登录失败、权限扩大/收窄、Feed/文章不可用。
- SQLite 写入错误、迁移异常、WAL 持续增长或数据库完整性检查失败。
- 发生未批准的注册、抓取、推送或外部写入。
- 归档路径不可读，或发现 Rust 使用了错误数据库。

## 回滚原则

- 优先恢复 Python upstream，不先删除 Rust 容器或数据库。
- 不覆盖生产原始数据库。
- 不删除最终备份、切换前状态快照或失败日志。
- 回滚涉及生产 Caddy/upstream 和写入状态，必须先获得用户明确批准。
- 不修改 SSH、sshd、防火墙、OCI 安全列表或 DNS。

## 回滚顺序

### 1. 冻结 Rust 写入

```bash
# 在 ARM 主机执行；确认影响后再执行
cd /opt/vpush-rust-staging
# 停止正式 Rust 服务或启用已批准的维护模式
# 保留容器、日志和数据库文件，禁止 rm -rf
```

记录：容器状态、镜像 digest、Rust 日志、SQLite `quick_check`、WAL/SHM 文件大小。

### 2. 恢复 Python upstream

在生产主机保存当前 Caddyfile 后恢复切换前副本：

```bash
cd /opt/vpush
cp Caddyfile Caddyfile.failed-<timestamp>
cp cutover-backups/<timestamp>/Caddyfile Caddyfile
caddy validate --config /etc/caddy/Caddyfile  # 视部署方式调整
# 仅在 validate 成功且用户已批准时 reload Caddy
```

如果使用 Docker Caddy：

```bash
docker exec vpush-caddy caddy validate --config /etc/caddy/Caddyfile
# 用户批准后：docker compose up -d --no-deps caddy
```

验证生产 Python：

```bash
docker inspect -f '{{.State.Health.Status}}' vpush
docker exec vpush sqlite3 -readonly /data/dav.db 'pragma quick_check;'
```

### 3. 判断数据库处理方式

- 如果 Rust 只读失败或尚未产生有效写入：保留 Rust 数据库，Python 继续使用原数据库。
- 如果 Rust 已产生业务写入：停止并保留 Rust 数据库，禁止直接把它覆盖回 Python 原库；先导出差异并人工判断。
- 如果最终切换前已做生产写入冻结：按冻结窗口记录决定是否解除冻结。
- 永远不使用未经校验的 Rust SQLite 文件覆盖 `/opt/vpush/data/dav.db`。

### 4. 回滚后验收

```text
Python 容器 healthy
Python upstream 可访问
登录、/api/me、Feed、文章和管理员登录正常
SQLite quick_check=ok
无异常 Rust 推送/抓取任务
Caddy 日志无持续 5xx
```

连续观察一个完整业务周期后，再决定是否清理 Rust 容器和临时文件。清理由用户另行批准，不作为回滚自动步骤。

## 事故记录

记录以下内容，不写入 token、密码或私钥：

- 切换开始/回滚时间
- Python/Rust 镜像 ID 或 digest
- Caddy 配置摘要和 upstream
- SQLite 文件大小、SHA-256、`quick_check`
- HTTP 状态码与错误摘要
- Rust/Python/Caddy 容器日志路径
- 是否产生写入、推送、抓取或外部副作用
