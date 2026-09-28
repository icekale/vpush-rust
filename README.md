# vpush-rust

Rust 重写进行中：登录、注册、会话、目录、订阅、动态流、雪球抓取，以及飞书群机器人推送。管理员 `POST /api/kols` 加大V，用户订阅后后台写入 posts，新帖推到飞书。

```sh
ln -sfn "/Users/kale/Documents/微信小程序大 v 订阅/dav-subscription/app/static" static
WEB_ADMIN_PASSWORD='至少10位' cargo run
```

打开 <http://127.0.0.1:8000>。`admin` 只在库里还没有这个用户时创建。监听非回环地址时必须配置 `WEB_TOKEN_SECRET`；本机开发未配置时才会回退到 sqlite 的 `settings` 表。备份恢复需要宿主机安装 `sqlite3` 命令。

| 变量 | 默认 |
|---|---|
| `VPUSH_DB` | `data/vpush.db` |
| `VPUSH_STATIC` | `static` |
| `HOST` / `PORT` | `127.0.0.1:8000` |
| `WEB_ALLOW_REGISTER` | 开启；`0` 关闭 |
| `FEISHU_WEBHOOK_URL` | 空。飞书自定义机器人地址，也可写在 settings 的 `feishu_webhook` |
| `VPUSH_FETCH` | 开启；`0` 关闭雪球抓取 |
| `CICC_LAB_LOG_DIR` | 未设置则不读 ARM 日志。没有 `local/.cicc/status.json` 时 `/api/admin/cicc/status` 仍是 `{"available":true,"stale":true}`。Compose 默认 `/app/cicc-lab-logs` |
| `CICC_LAB_MANIFEST` | 未设置。若设置，该文件的 mtime 是第二条新鲜度信号：文件缺失，或老于 `CICC_LAB_STALE_HOURS`，也算过期。Compose 不挂载清单 |
| `CICC_LAB_STALE_HOURS` | `36`。只作用于 ARM 日志信号 |
| `TRUSTED_PROXIES` | 只信任回环（`127.0.0.0/8`、`::1/128`）。Caddy 在 Docker 网段时要显式写成该网段，例如 `172.16.0.0/12`。私网不会默认信任 |
| `TRUST_X_REAL_IP` | 关闭。设为 `1` 后，受信代理没给出可用的 `X-Forwarded-For` 时才回落 `X-Real-IP` |
| `SSRF_ALLOW_FAKE_IP` | 关闭，拦截 `198.18.0.0/15`。只有生产 DNS 使用 fake-ip（常见于 Clash/sing-box）时才设为 `1` |

## 上线注意

- 升级前先备份 `vpush.db`。旧库会把 `users` 迁成 `AUTOINCREMENT`：保留现有 id、约束、索引和触发器，并清掉 `user_id` 对不上现有用户的孤儿行。如果建表语句不是 `INTEGER PRIMARY KEY`，或迁移后外键校验失败，进程会停在启动并提示人工处理，原库事务会回滚。
- 这次改了会话 token，所有人需要重新登录。
- `compose.yaml` 把端口绑在 `127.0.0.1:8000`。反代若不是本机回环，设置 `TRUSTED_PROXIES` 为 Caddy 所在网段，否则登录限流和图片代理配额会把整段内网当成同一个客户端，或者反过来把转发头忽略掉。
- 启动日志会打出当前生效的信任代理列表，以及是否信任 `X-Real-IP`。

中金日常采集不写 `status.json`。ARM 上的 systemd timer 每天 03:00（Asia/Shanghai）跑同步，日志在宿主机 `/data/vpush-ima-cache/logs/cicc-host-sync-YYYYMMDD-HHMMSS.log`：开头一行 `start <iso> ...`，结尾一行 `done rc=N <iso>`。可选清单是 `/data/vpush-ima-cache/manifest/compress_state_cicc.json`。`compose.yaml` 把日志目录只读挂进容器：宿主机 `${CICC_LAB_LOG_HOST:-/data/vpush-ima-cache/logs}` → `/app/cicc-lab-logs`。

这些日志在宿主机上是用户 `ubuntu` 的 mode `600`。容器用户是 `vpush`（uid 10001），默认读不到。需要给容器用户读权限：加组，或 ACL。例如：

```sh
sudo setfacl -m u:10001:rX /data/vpush-ima-cache/logs
sudo setfacl -d -m u:10001:r /data/vpush-ima-cache/logs
sudo setfacl -m u:10001:r /data/vpush-ima-cache/logs/cicc-host-sync-*.log
```

目录本身也要能进入。默认 ACL 让之后新建的日志也能被容器读到。读不到或没有日志时，接口返回 `source=arm-lab` 且 `stale=true`，但不会写顶层 `ts`。存储机那条「超过 `stale_minutes`（默认 30 分钟）未刷新」告警因此不会因为每天一次的成功同步而触发。ARM 信号自己的过期告警键是 `lab_stale`，冷却仍是 24 小时：退出码非 0、日志年龄超过小时阈值，或目录里没有可读的 `cicc-host-sync-*.log` 才会发。成功且未过期不发。
