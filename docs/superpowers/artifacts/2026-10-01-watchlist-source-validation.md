# 行情数据源核验（2026-10-01）

实测时间：2026-09-30 23:25 UTC（北京时间 2026-10-01 07:25）。本轮仅使用 Rust 服务现有雪球 App 身份；不轮换身份、不改生产设置、不发送通知。

## 普通股票

- 报价接口：`GET https://stock.xueqiu.com/v5/stock/batch/quote.json?symbol=...&extend=detail`。
- 请求采用现有 Cookie 和 `Xueqiu Android 14.96.3`，以及项目现有 Accept/Origin/X-Requested-With/Referer 模式。
- 50 个不同的 A/H/美股代码，HTTP 200、`error_code=0`，返回 50 项，没有缺失。当前实现每批最多 20 项、后台每分钟至多一轮；50 是实测成功批量大小，不宣称服务端极限或长期稳定性。
- 数据位于 `data.items[].quote`。普通股类型：A 股 `type=11`，港股 `type=30`，美股 `type=0`；指数/ETF/OTC 类型不作为普通股接受。
- `timestamp` / `time` 为 Unix 毫秒，`current` / `percent` / `tick_size` 为数值。币种分别为 CNY/HKD/USD。
- 样本：`SH600519` 贵州茅台 1258.62 CNY，tick_size=0.01；`00700` 腾讯控股 431 HKD，tick_size=0.2；`AAPL` 苹果 333.02 USD，tick_size=0.01。港股阈值不能一律按分验证。
- 样本报价均处于休市，不能据此证明盘中更新延迟；提醒只接受对应常规交易时段内、年龄不超过120秒且非未来的报价。
- 搜索接口：`GET https://xueqiu.com/stock/search.json?code=<名称或代码>&size=5&page=1`，返回 `stocks[]`，其中 `code/name/type/current/percentage/exchange` 有效。中文名称「贵州茅台」与代码 `AAPL` 已验证。候选需再用批量报价核对类型、币种和代码。
- 本轮请求串行、间隔至少一秒，没有进行压测，也未确定服务端限流阈值；不将少量成功请求描述为稳定性保证。

## 股债利差门槛

- 相同认证批量接口查询 `SH000300` 返回沪深300指数、`type=12`、`current=4357.62`、`timestamp=1790751600000`，但 `pe_ttm=null`、`pe_forecast=null`。
- 因沪深300 PE-TTM 缺失，当前雪球报价契约不满足 `1/PE-TTM - 中国10年期国债收益率` 的输入要求。
- 即使找到国债收益率，也不得拼入替代 PE 或展示数值。股债指标保持明确不可用；不扩展计算模块或历史曲线。

## 中国国债收益率来源候选

- 官方候选：`GET https://yield.chinabond.com.cn/cbweb-pbc-web/pbc/historyQuery?startDate=2026-09-28&endDate=2026-10-01&gjqx=10&qxId=hzsylqx&locale=en_US`。
- HTML 表标题为 `ChinaBond Government Bond Yield Curve`，列名为 `10 Y`。成功提取三日原始数值：2026-09-28 `1.6790`、2026-09-29 `1.6670`、2026-09-30 `1.6822`；10月1日尚无发布行。
- `/pbc/historyDown` 同参数可返回 XLSX。当前只确认日期，没有独立核实发布时间、单位声明、更新频率或限流规则；原始数值不直接作为已核验的百分比使用，也未完成另一可信来源的三日对照。
- 因源契约未完整核验且 PE-TTM 已缺失，指标保持不可用；此候选不接入生产计算。

本地原始报价证据保存在临时目录，未包含 Cookie/身份字段；报告只保存接口契约与核验结果。
