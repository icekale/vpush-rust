# vpush-rust

Rust 重写进行中：登录、注册、会话、目录、订阅、动态流、雪球抓取，以及飞书群机器人推送。管理员 `POST /api/kols` 加大V，用户订阅后后台写入 posts，新帖推到飞书。

```sh
ln -sfn "/Users/kale/Documents/微信小程序大 v 订阅/dav-subscription/app/static" static
WEB_ADMIN_PASSWORD='至少10位' cargo run
```

打开 <http://127.0.0.1:8000>。`admin` 只在库里还没有这个用户时创建。没设 `WEB_TOKEN_SECRET` 时，会话签名密钥写在 sqlite 的 `settings` 表。

| 变量 | 默认 |
|---|---|
| `VPUSH_DB` | `data/vpush.db` |
| `VPUSH_STATIC` | `static` |
| `HOST` / `PORT` | `127.0.0.1:8000` |
| `WEB_ALLOW_REGISTER` | 开启；`0` 关闭 |
| `FEISHU_WEBHOOK_URL` | 空。飞书自定义机器人地址，也可写在 settings 的 `feishu_webhook` |
| `VPUSH_FETCH` | 开启；`0` 关闭雪球抓取 |
