# 飞书 Bot 完整迁移设计

日期：2026-09-27
状态：待用户审阅；Telegram 阶段通过后实施

## 目标与边界

ARM Rust 接管 DMIT Python 共享飞书应用的双向消息/卡片，以及现有个人应用的绑定和推送。已有用户身份、机器人注册会话、应用凭据和订阅有效，无须用户重新绑定。其它飞书文档/OAuth 不能因密钥转换失效；Python 原件只作回滚，不能与 Rust 同时监听同一应用。

## 现状差异

- Python `/opt/vpush/src/app/feishu_bot.py` 用共享应用 WebSocket 处理私聊命令/交互卡片，`bot_core.py` 共用订阅/绑定逻辑。Rust `src/feishu_ws.rs` 只有个人应用注册时的临时监听，没有共享应用消息/卡片处理。
- Python `feishu_personal.py` 的个人应用和注册会话凭据为原始 Fernet token；用户凭据与文档 OAuth/app-secret 为 `enc1:` + Fernet。Rust `src/feishu_personal.rs` 使用 AES-256-GCM；相同密钥也不能直接打开旧密文。
- Python `channels.py` 按用户选择：个人应用优先，确定的权限/凭据错误降级共享应用，不确定网络错误仅重试以避免双发。Rust `src/push.rs` 目前折叠所有用户为一次 webhook 推送，失败记为 `user_id=0`。
- Python `notifiers/feishu.py` 有图片、组合、摘要、日报、DND 等卡片；Rust `src/feishu.rs` 仅基础群 webhook 卡片。共享应用凭据与 webhook 是启动配置，webhook 不能代替个人消息投递。

## 数据与密钥

1. 备份 DMIT 与 ARM SQLite，预检 `feishu_personal_bots`、`feishu_registration_sessions`、用户和文档凭据列的实际 schema；以**当前 ARM 数据**为待转换对象，DMIT 旧库不覆盖切换后的新绑定/订阅。统计个人应用、注册会话、用户凭据、文档密文的数量/状态/不可逆指纹；不记录明文。DMIT 原库不变。
2. 在隔离 ARM 副本逐条解密旧 Fernet，以 Rust AES-GCM 重加密；区分原始个人 token 与 `enc1:` 凭据。逐项往返验证、事务性应用到正式 ARM 库；任一失败即停止，不清空无法解密的字段。只迁必需的共享应用配置、webhook 与旧密钥，目标文件权限限制为服务必需范围。
3. 用户 `feishu_open_id`、`feishu_chat_id`、个人应用 `chat_id`、tenant brand 及绑定状态原样保留；个人投递优先使用 chat_id，不拿 open_id 冒充。

## Rust 行为

- 复用 `feishu_ws.rs` 现有连接/帧处理，新增共享应用单实例监听。私聊建立个人投递目标；群聊只回复，不绑定推送目标。与 Telegram 真正共用的订阅命令复用同一核心；卡片回调、绑定码、审批须有权限及幂等检查。
- 个人注册监听只接受私聊及严格的 `/bind <code>`，核对目标 open_id；先用候选应用测试发送再激活。已有 active 应用无须重新注册。
- 出站逐用户选择个人 active bot 或共享 app，群 webhook 保持独立用途。tenant token 缓存按应用隔离，Feishu/Lark 主机按 tenant brand 选择；仅确定不可用的个人应用可降级共享，其余进按用户/帖子重试。记录每位用户结果，不再折叠到 `user_id=0`。
- 对齐 Python 卡片、图片、组合/摘要/日报/DND 及纯文本降级；网络超时不得同时向个人和共享应用发送同一条消息。

## 验证与切换

- 离线测试密文转换、旧绑定/新绑定、私聊与群聊权限、回调幂等、个人/共享选择、确定错误降级、不确定错误不双发、消息格式，以及文档密钥仍可解开。
- 独立测试应用灰度验证 WebSocket 和真实 API；生产应用必须等 Python listener 已停、Rust 仅一个连接后才启用。原有个人应用逐个核对状态及实际可投递性，不能仅凭行数宣称无损。
- 遇任一密文失败、文档 OAuth 回归、错误收件人、重复发送、共享应用事件冲突或已有个人应用不可用即停止。回滚先停 Rust 监听/发送再起 Python，并核对期间新增的绑定/订阅，避免旧库覆盖新变更。
