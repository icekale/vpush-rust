# vpush Rust 正式切换 Runbook

状态：仅供审阅，默认不执行生产写操作、不修改 Caddy、不切流。
目标版本：`1.12.277`

## 当前基线

| 项目 | 当前值 |
|---|---|
| Python 生产主机 | `root@179.255.150.134` |
| Python 容器 | `vpush`，健康 |
| Python 数据库 | `/opt/vpush/data/dav.db`，容器内 `/data/dav.db` |
| Python 镜像 ID | `sha256:9b81c0114a6c1f13a624466d72bddc6bd23cb41bdcc5d7c461963f814b39c373` |
| Caddy upstream | `vpush:8000`，位于生产 Docker 网络 |
| Rust ARM 主机 | `ubuntu@167.234.213.74` |
| Rust 提交 | `7eaaa06` |
| Rust 镜像 digest | `vpush-rust@sha256:10323af342710a5b14292829ddd12b1cb6cd1c35c58b9f582e9a7bd20c0eabd9` |
| Rust staging 数据库 | `/opt/vpush-rust-staging/data/vpush.db`，当前空库 |
| Rust health | `/healthz` 返回 `{"status":"ok"}` |

## 硬阻塞

生产主机到 ARM `167.234.213.74:8000` 的只读连通性探测超时，当前没有可用的 Caddy upstream 路径。正式切换前必须由用户单独批准并验证以下一种方案：

- 已批准的生产到 ARM 反向 SSH 隧道，并让 Caddy 只连接本机隧道端点；或
- 已批准的 OCI 网络路径，使生产 Caddy 能访问 ARM 服务端口。

不能直接把 `reverse_proxy` 改为 ARM 公网地址后假定可用，也不能修改 OCI 安全列表或防火墙而不重新确认。

## 切换前门禁

全部满足后才能请求正式切流：

- [ ] 用户明确批准生产写入冻结和最终切流。
- [ ] 用户明确批准生产 Caddy/upstream 或隧道变更。
- [ ] ARM 到生产所需的网络路径已验证，Caddy 能访问健康端点。
- [ ] Rust 版本、镜像 digest 和配置审阅完成。
- [ ] 生产 SQLite 在线备份完成，`quick_check=ok`，原始备份只读保留。
- [ ] 归档文件已完成最终同步并通过文件数量/大小/一致性检查。
- [ ] Rust 最终数据库副本迁移完成，核心计数一致。
- [ ] staging smoke 通过：登录、权限、订阅、Feed、新闻、知识库、管理接口和拒绝分支。
- [ ] `WEB_ALLOW_REGISTER=0`、`VPUSH_FETCH=0` 和真实推送关闭状态已确认。
- [ ] 生产 Python 容器仍健康，回滚镜像和 Caddy 配置均可用。
- [ ] xincai、IMA pull、OpenList 等外部服务状态已记录，不纳入本次变更。

## 正式切换顺序

以下命令是模板，执行前必须替换路径并再次展示影响；本文件本身不执行这些命令。

### 1. 保存生产状态

```bash
ssh -i ~/.ssh/vpush_prod_key -o IdentitiesOnly=yes root@179.255.150.134
cd /opt/vpush
mkdir -p /opt/vpush/cutover-backups/<timestamp>
docker inspect vpush > /opt/vpush/cutover-backups/<timestamp>/vpush.inspect.json
docker inspect vpush-caddy > /opt/vpush/cutover-backups/<timestamp>/caddy.inspect.json
docker image inspect dav-subscription-vpush:latest > /opt/vpush/cutover-backups/<timestamp>/python-image.json
docker exec vpush sh -c 'sha256sum /data/dav.db /data/dav.db-wal /data/dav.db-shm 2>/dev/null || true' \
  > /opt/vpush/cutover-backups/<timestamp>/db-hashes.txt
```

不要把 `.env`、密钥或 token 写入备份报告或提交。

### 2. 冻结生产写入

在用户确认的维护窗口执行：

- 停止 Python 应用写入和调度任务，或使用已有维护开关阻止写请求。
- 确认 Caddy 仍保留旧 upstream，但不再接受会改变数据库的请求。
- 记录冻结开始时间和 `docker logs` 最后一条业务日志。

若无法证明写入已冻结，停止流程。

### 3. 生成最终 SQLite 副本

使用生产容器内 `sqlite3.Connection.backup()` 或等价在线备份写入 `/data` 下独立临时文件；不得写 `/tmp`，生产 `/tmp` 只有 64 MB。

```bash
docker exec vpush python -c 'import sqlite3; src=sqlite3.connect("file:/data/dav.db?mode=ro", uri=True); dst=sqlite3.connect("/data/vpush-cutover-<timestamp>.db"); src.backup(dst); print(dst.execute("pragma quick_check").fetchone()[0]); dst.close(); src.close()'
docker exec vpush sha256sum /data/vpush-cutover-<timestamp>.db
docker exec vpush sqlite3 -readonly /data/vpush-cutover-<timestamp>.db \
  'select (select count(*) from users),(select count(*) from posts),(select count(*) from kols),(select count(*) from subscriptions);'
```

把副本传到 ARM 独立目录，原始副本只读保留，校验 SHA-256 后再让 Rust 使用工作副本。

### 4. 准备 Rust 正式实例

- 停止当前 staging 写入或保持 staging 独立。
- 将最终副本复制为 Rust 正式工作库。
- 设置已批准的 `WEB_TOKEN_SECRET`、`VPUSH_STATIC`、`VPUSH_DB`、`WEB_ALLOW_REGISTER=0`、`VPUSH_FETCH=0`。
- 检查 `IMA_ARCHIVE_ROOT` 及归档挂载；缺失时必须保持对应功能明确不可用。
- 启动指定 digest 的 ARM 镜像，等待 Docker healthcheck。
- 只从 ARM 本机和已批准的 Caddy 路径验证 `/healthz`、`/api/version`。

### 5. 切换 upstream

仅在网络路径已验证且用户再次批准后执行 Caddy 配置变更。切换应具备：

- 新 upstream 仅指向 Rust 实例。
- 保留原 `vpush:8000` 配置副本。
- `caddy validate` 通过。
- 先 reload，再检查 Caddy 日志和外部 HTTPS 状态码。
- 不修改 DNS、OCI 安全列表、iptables 或 SSH 配置。

### 6. 切换后验收

按顺序检查：

```text
GET /healthz                         200
GET /api/version                    current=1.12.277
未登录访问受保护 API                 401/403
登录、/api/me、Feed、文章、新闻       成功且字段符合基线
订阅与 WebPush                       成功且用户隔离
管理员接口                           admin 成功，普通用户拒绝
注册                                保持关闭
抓取和真实推送                       保持关闭，除非另行批准
```

确认无误后再解除写入冻结。解除后立即观察 Rust 日志、SQLite WAL、推送队列和错误率。

## 停止条件

出现任一情况立即停止，不继续切流：

- Caddy 无法访问 ARM upstream。
- `quick_check` 非 `ok` 或核心计数不一致。
- 健康检查失败、迁移重复执行产生异常、或登录/权限契约变化。
- 任何外部推送、抓取或注册意外启动。
- 归档文件缺失、权限错误或 Rust 指向生产原库。
- 需要修改 SSH、sshd、防火墙、OCI 安全列表或 DNS 才能继续。
