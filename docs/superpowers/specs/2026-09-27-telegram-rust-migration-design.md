# Telegram Bot 完整迁移设计

日期：2026-09-27
状态：待用户审阅

## 目标与边界

DMIT Python 停止后，ARM Rust 直接承接共享 Telegram bot 的双向交互、自建 bot 推送，以及原有富媒体和订阅操作。已有绑定、订阅、令牌、失败重试和用户权限不丢失；Python 数据库和配置保留为回滚点。本阶段不启用飞书共享机器人。

“无损”指配置与持久数据可用、功能等价、同一 bot 不被两个进程同时消费。Telegram 外部 API 对发送请求没有端到端幂等保证，因此网络超时后不能保证物理意义上的恰好发送一次；须记录不确定结果并控制重试。

## 现状差异

- Python `/opt/vpush/src/app/telegram_bot.py` 的 `getUpdates` 长轮询处理私聊 `/start`、`/help`、`/list`、`/search`、`/sub`、`/unsub`、`/mysubs`、`/ask`、`/bind`、深链绑定、按钮分页/订阅/审批；`bot_core.py` 实现通用命令。Rust `src/push.rs` 只有纯文本发送和绑定时的一次性 `getUpdates`，无入站 worker。
- Python 共用 token 由 `TELEGRAM_BOT_TOKEN` 提供；`users.telegram_bot_token` 中的自建 token 可能是 `enc1:` + Fernet 密文，由 `FEISHU_CREDENTIAL_KEY` 解密。Rust 目前直接按明文校验，照搬密文会静默回退到共享 bot。
- Python `notifiers/telegram.py`、`telegram_rich.py` 支持 HTML、按钮、图片/视频、摘要/组合/日报/DND 汇总及失败回退；Rust 当前只发纯文本。数据库已有 `config_telegram_rich_messages` 设置但发送端未使用。
- Python polling offset 只存内存；Rust 改为 SQLite 游标以防重启重放。不能用生产 token 同时启动两套 poller。

## 数据与密钥

1. 备份 DMIT Python 数据库和当前 ARM 数据库；以**当前 ARM 数据**为唯一待转换对象，DMIT 旧库只作参考与回滚，不覆盖切换后新增的绑定/订阅。统计 Telegram 绑定、自建 token、订阅、绑定码、重试和推送设置。只记录数量或不可逆指纹，不输出明文。
2. 在 ARM 的隔离副本上用旧 `FEISHU_CREDENTIAL_KEY` 解开 `enc1:` 凭据，再转换为 Rust 可读的加密格式；逐条往返校验。遇到不能解密的字段立即停止，不当作明文，也不回退共享 bot。DMIT 原库不写入。凭据不能进入仓库、shell 历史或日志。
3. 转换应幂等，保留已有绑定、token 哈希、用户设置。正式 ARM 库变更前备份和完整性检查。灰度使用独立测试 bot，生产共享 bot 始终只有一个监听者。

## Rust 行为

- 独立 Telegram 入站适配器复用 `Db::consume_bind_code`、现有订阅/可见性接口和发送函数；只接收私聊消息/回调。未绑定用户按旧规则创建账号；绑定码单次消费、限速；管理员审批校验权限。
- 对齐命令语义、分页、搜索、按钮 `callback_data`、订阅类型及提示。自建 bot 绑定发现不能抢走共享 bot 的 `getUpdates` 游标。
- 单实例长轮询：SQLite 事务成功后记录 update_id；失败保留可重试状态，订阅和审批回调幂等。记录外部回复失败但不记录 token、明文消息或聊天内容。
- 出站逐用户选择“自建 bot 优先、共享 bot 回退”，校验 chat_id；对齐 HTML 转义/长度、富消息/媒体降级、DND/次要/日报和按用户/帖子重试。不改其他渠道。

## 验证与切换

- 隔离 SQLite 和录制的 Telegram updates/响应覆盖命令、按钮、错误、权限、绑定配额、游标重启及重复 update；富媒体快照对照 Python 输出。
- ARM 使用独立测试 bot 验证私聊、绑定、订阅、按钮、富媒体和失败处理。生产 poller 在唯一监听检查、备份后才启用，不能与 Python 并行。
- 启用前确认自建 token 可解密数量与旧库一致；受控推送检查收件人、发送结果和重试。遇解密失败、错绑、错误审批、token 泄漏、重复 poller、关键命令不兼容即停止。
- 回滚先停止 Rust poller/出站 worker，再恢复 Python 轮询；保留 ARM 迁移前副本。灰度期间新增的绑定/订阅需导出核对，不直接用旧 Python 库覆盖新数据。
