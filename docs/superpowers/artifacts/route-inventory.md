# Python/Rust Route Inventory

> Generated from source. Configuration values are intentionally excluded.

- Python source: `/opt/vpush/src/app/api.py`
- Rust source: `src/main.rs`
- Python routes: **198**
- Rust routes: **180**
- Python route prefix removed for comparison: `/api`
- Exact route matches: **179**
- Python-only routes: **19**
- Rust-only routes: **1**

## Python-only

| Method | Path |
|---|---|
| `DELETE` | `/admin/register-codes/{}` |
| `DELETE` | `/me/android-devices/{}` |
| `GET` | `/admin/ima-storage/alerts` |
| `GET` | `/admin/ima-storage/consistency` |
| `GET` | `/admin/ima-storage/health` |
| `GET` | `/admin/proxy-pools/{}` |
| `GET` | `/admin/twitter-cookie` |
| `GET` | `/admin/xueqiu-cookie` |
| `GET` | `/admin/zsxq-cookie` |
| `GET` | `/ima-documents/{}/timeline` |
| `PATCH` | `/admin/register-codes/{}` |
| `POST` | `/admin/feishu-documents/oauth/callback` |
| `POST` | `/admin/ima-storage/backup` |
| `POST` | `/admin/ima-storage/consistency/run` |
| `POST` | `/admin/ima-storage/dedup` |
| `POST` | `/admin/ima-storage/refresh` |
| `PUT` | `/admin/ima-storage/alerts` |
| `PUT` | `/admin/proxy-pools/{}` |
| `PUT` | `/me/android-devices/{}` |

## Rust-only

| Method | Path |
|---|---|
| `GET` | `/healthz` |

## Configuration Names

### Python

- `IMA_COOKIE`
- `IMA_OPENAPI_APIKEY`
- `IMA_OPENAPI_CLIENTID`
- `IMA_PULL_URL`
- `IMA_STORAGE_REFRESH_REQUEST`
- `IMGBED_BASE_URL`
- `IMGBED_TOKEN`
- `TWITTER_COOKIE`
- `WEIBO_COOKIE`
- `XINCAI_INGEST_TOKEN`
- `XUEQIU_COOKIE`
- `ZSXQ_ACCESS_TOKEN`
- `ZSXQ_COOKIE`

### Rust

- `ALERTS_ENABLED`
- `FEISHU_ARCHIVE_ROOT`
- `FEISHU_CREDENTIAL_KEY`
- `FEISHU_DOCS_APP_SECRET`
- `FEISHU_PERSONAL_LISTENER`
- `FEISHU_WEBHOOK_URL`
- `HOST`
- `IMA_APP_VER`
- `IMA_ARCHIVE_ROOT`
- `IMA_BASE`
- `IMA_GUID`
- `IMA_IUA`
- `IMA_PULL_URL`
- `IMA_Q36`
- `PORT`
- `TELEGRAM_BOT_TOKEN`
- `TURNSTILE_HOSTNAMES`
- `TURNSTILE_SECRET`
- `TURNSTILE_SITE_KEY`
- `TWITTER_COOKIE`
- `VAPID_MAILTO`
- `VAPID_PRIVATE_KEY`
- `VAPID_PUBLIC_KEY`
- `VPUSH_DB`
- `VPUSH_FETCH`
- `VPUSH_POLL_SECONDS`
- `VPUSH_STATIC`
- `WEB_ADMIN_PASSWORD`
- `WEB_ALLOW_REGISTER`
- `WEB_TOKEN_SECRET`
- `WECHAT_APP_ID`
- `WECHAT_APP_SECRET`
- `WEIBO_COOKIE`
- `XINCAI_INGEST_TOKEN`
- `XUEQIU_APP_IDENTITY`
- `XUEQIU_DEVICE_ID`

## Source Routes

## Python

| Method | Path | Line |
|---|---|---:|
| `GET` | `/admin/backup` | 5239 |
| `GET` | `/admin/backup/download` | 5266 |
| `POST` | `/admin/backup/restore/upload` | 5291 |
| `POST` | `/admin/backup/restore/webdav` | 5280 |
| `PUT` | `/admin/backup/webdav` | 5245 |
| `POST` | `/admin/backup/webdav/test` | 5256 |
| `GET` | `/admin/cicc/schedule` | 4684 |
| `PUT` | `/admin/cicc/schedule` | 4693 |
| `GET` | `/admin/cicc/status` | 4659 |
| `POST` | `/admin/cicc/trigger` | 4668 |
| `DELETE` | `/admin/cookies/{kind}` | 3516 |
| `GET` | `/admin/dashboard` | 5317 |
| `GET` | `/admin/error-logs` | 5322 |
| `GET` | `/admin/feishu-documents` | 4109 |
| `POST` | `/admin/feishu-documents` | 4280 |
| `PUT` | `/admin/feishu-documents/config` | 4124 |
| `GET` | `/admin/feishu-documents/oauth/callback` | 4218 |
| `POST` | `/admin/feishu-documents/oauth/callback` | 4200 |
| `POST` | `/admin/feishu-documents/oauth/start` | 4177 |
| `POST` | `/admin/feishu-documents/preview` | 4267 |
| `DELETE` | `/admin/feishu-documents/{source_id}` | 4354 |
| `PATCH` | `/admin/feishu-documents/{source_id}` | 4296 |
| `POST` | `/admin/feishu-documents/{source_id}/sync` | 4338 |
| `GET` | `/admin/ima-arm` | 4757 |
| `GET` | `/admin/ima-collector` | 4367 |
| `PUT` | `/admin/ima-collector` | 4469 |
| `GET` | `/admin/ima-collector/cicc-categories` | 4714 |
| `PUT` | `/admin/ima-collector/cicc-categories` | 4736 |
| `POST` | `/admin/ima-collector/discover` | 4374 |
| `PUT` | `/admin/ima-collector/groups/{group_id}/acl` | 4429 |
| `GET` | `/admin/ima-collector/groups/{group_id}/folders` | 4384 |
| `POST` | `/admin/ima-collector/sync` | 4592 |
| `GET` | `/admin/ima-credentials` | 4968 |
| `POST` | `/admin/ima-credentials` | 4985 |
| `GET` | `/admin/ima-local-libraries` | 4895 |
| `POST` | `/admin/ima-local-libraries` | 4952 |
| `POST` | `/admin/ima-local-libraries/scan` | 4901 |
| `PUT` | `/admin/ima-local-libraries/{slug}` | 4932 |
| `PUT` | `/admin/ima-local-libraries/{slug}/enabled` | 4911 |
| `GET` | `/admin/ima-storage/alerts` | 4872 |
| `PUT` | `/admin/ima-storage/alerts` | 4878 |
| `POST` | `/admin/ima-storage/backup` | 4642 |
| `GET` | `/admin/ima-storage/consistency` | 4841 |
| `POST` | `/admin/ima-storage/consistency/run` | 4850 |
| `POST` | `/admin/ima-storage/dedup` | 4861 |
| `GET` | `/admin/ima-storage/health` | 4832 |
| `POST` | `/admin/ima-storage/refresh` | 4632 |
| `DELETE` | `/admin/imgbed` | 3384 |
| `GET` | `/admin/imgbed` | 3380 |
| `PUT` | `/admin/imgbed` | 3397 |
| `GET` | `/admin/inactive-users-policy` | 5962 |
| `PUT` | `/admin/inactive-users-policy` | 5972 |
| `GET` | `/admin/kol-requests` | 3128 |
| `POST` | `/admin/kol-requests/{request_id}/approve` | 3132 |
| `POST` | `/admin/kol-requests/{request_id}/reject` | 3141 |
| `GET` | `/admin/kols` | 5359 |
| `POST` | `/admin/kols/batch` | 5386 |
| `GET` | `/admin/logs` | 5232 |
| `GET` | `/admin/news/articles` | 2926 |
| `DELETE` | `/admin/news/articles/{article_id}` | 2940 |
| `POST` | `/admin/news/feeds/validate` | 2842 |
| `DELETE` | `/admin/news/feeds/{feed_id}` | 2917 |
| `PATCH` | `/admin/news/feeds/{feed_id}` | 2871 |
| `POST` | `/admin/news/feeds/{feed_id}/archive` | 2901 |
| `POST` | `/admin/news/feeds/{feed_id}/refresh` | 2948 |
| `POST` | `/admin/news/feeds/{feed_id}/restore` | 2909 |
| `POST` | `/admin/news/refresh` | 2961 |
| `GET` | `/admin/news/settings` | 2730 |
| `PATCH` | `/admin/news/settings` | 2743 |
| `GET` | `/admin/news/sources` | 2761 |
| `POST` | `/admin/news/sources` | 2773 |
| `DELETE` | `/admin/news/sources/{source_id}` | 2820 |
| `PATCH` | `/admin/news/sources/{source_id}` | 2787 |
| `POST` | `/admin/news/sources/{source_id}/archive` | 2806 |
| `POST` | `/admin/news/sources/{source_id}/feeds` | 2850 |
| `POST` | `/admin/news/sources/{source_id}/refresh` | 2827 |
| `POST` | `/admin/news/sources/{source_id}/restore` | 2813 |
| `GET` | `/admin/plaza-sources` | 3227 |
| `PUT` | `/admin/plaza-sources` | 3231 |
| `GET` | `/admin/polling-config` | 3241 |
| `PUT` | `/admin/polling-config` | 3245 |
| `GET` | `/admin/proxies` | 5122 |
| `POST` | `/admin/proxies` | 5126 |
| `DELETE` | `/admin/proxies/{proxy_id}` | 5135 |
| `POST` | `/admin/proxies/{proxy_id}/test` | 5143 |
| `GET` | `/admin/proxy-pools` | 5043 |
| `POST` | `/admin/proxy-pools` | 5047 |
| `DELETE` | `/admin/proxy-pools/{pool_id}` | 5094 |
| `GET` | `/admin/proxy-pools/{pool_id}` | 5068 |
| `PUT` | `/admin/proxy-pools/{pool_id}` | 5075 |
| `POST` | `/admin/proxy-pools/{pool_id}/extract` | 5111 |
| `POST` | `/admin/proxy-pools/{pool_id}/import` | 5102 |
| `GET` | `/admin/proxy-routes` | 5150 |
| `PUT` | `/admin/proxy-routes` | 5154 |
| `POST` | `/admin/register-code-batches/{batch_id}/revoke-unused` | 5189 |
| `GET` | `/admin/register-codes` | 3194 |
| `POST` | `/admin/register-codes` | 3149 |
| `POST` | `/admin/register-codes/batch` | 5198 |
| `DELETE` | `/admin/register-codes/{code}` | 5181 |
| `PATCH` | `/admin/register-codes/{code}` | 5219 |
| `POST` | `/admin/register-codes/{code}/revoke` | 5185 |
| `GET` | `/admin/system-logs` | 5339 |
| `POST` | `/admin/test-push` | 6265 |
| `GET` | `/admin/turnstile` | 3445 |
| `PUT` | `/admin/turnstile` | 3449 |
| `GET` | `/admin/twitter-cookie` | 3213 |
| `POST` | `/admin/twitter-cookie` | 3330 |
| `POST` | `/admin/users/batch` | 5981 |
| `PUT` | `/admin/users/{user_id}/ima-kb` | 4443 |
| `POST` | `/admin/weibo-qr/start` | 6290 |
| `GET` | `/admin/weibo-qr/status` | 6309 |
| `GET` | `/admin/xueqiu-cookie` | 3209 |
| `POST` | `/admin/xueqiu-cookie` | 3320 |
| `POST` | `/admin/zsxq-cache/purge` | 3526 |
| `GET` | `/admin/zsxq-cookie` | 3481 |
| `POST` | `/admin/zsxq-cookie` | 3495 |
| `POST` | `/auth/login` | 1810 |
| `POST` | `/auth/logout` | 2319 |
| `POST` | `/auth/register` | 1777 |
| `GET` | `/auth/turnstile` | 1771 |
| `POST` | `/auth/wechat` | 1856 |
| `GET` | `/catalog` | 2352 |
| `GET` | `/categories` | 5642 |
| `POST` | `/categories` | 5647 |
| `DELETE` | `/categories/{category_id}` | 5673 |
| `PUT` | `/categories/{category_id}` | 5659 |
| `GET` | `/ima-documents` | 3684 |
| `GET` | `/ima-documents/catalog` | 3742 |
| `DELETE` | `/ima-documents/groups/{group_id}/subscribe` | 3779 |
| `POST` | `/ima-documents/groups/{group_id}/subscribe` | 3762 |
| `GET` | `/ima-documents/tickers/{code}` | 3785 |
| `GET` | `/ima-documents/timeline/all` | 3924 |
| `GET` | `/ima-documents/{media_id}` | 3820 |
| `GET` | `/ima-documents/{media_id}/assets/{asset_id}` | 3981 |
| `GET` | `/ima-documents/{media_id}/pdf` | 4018 |
| `GET` | `/ima-documents/{media_id}/text` | 4001 |
| `GET` | `/ima-documents/{media_id}/timeline` | 3889 |
| `POST` | `/ima-documents/{media_id}/translate` | 3856 |
| `GET` | `/img-proxy` | 6341 |
| `POST` | `/kol-requests` | 3091 |
| `GET` | `/kols` | 5409 |
| `POST` | `/kols` | 5413 |
| `POST` | `/kols/batch` | 5504 |
| `DELETE` | `/kols/{kol_id}` | 5634 |
| `GET` | `/kols/{kol_id}` | 3040 |
| `PUT` | `/kols/{kol_id}` | 5588 |
| `GET` | `/kols/{kol_id}/holdings` | 3073 |
| `GET` | `/kols/{kol_id}/nav` | 3082 |
| `GET` | `/kols/{kol_id}/posts` | 3059 |
| `GET` | `/live/wscn` | 3016 |
| `GET` | `/market/indices` | 2980 |
| `GET` | `/me` | 1905 |
| `PUT` | `/me` | 1943 |
| `DELETE` | `/me/android-devices/{installation_id}` | 2180 |
| `PUT` | `/me/android-devices/{installation_id}` | 2150 |
| `POST` | `/me/bind-code` | 2190 |
| `DELETE` | `/me/feishu-personal` | 2313 |
| `POST` | `/me/feishu-personal/register` | 2271 |
| `GET` | `/me/feishu-personal/register/{session_id}` | 2280 |
| `POST` | `/me/feishu-personal/register/{session_id}/cancel` | 2305 |
| `POST` | `/me/feishu-personal/register/{session_id}/refresh-code` | 2287 |
| `POST` | `/me/llm-models` | 2100 |
| `POST` | `/me/llm-test` | 2113 |
| `POST` | `/me/password` | 2324 |
| `DELETE` | `/me/webpush` | 2145 |
| `POST` | `/me/webpush` | 2123 |
| `GET` | `/media/zsxq-file/{file_id}` | 3601 |
| `GET` | `/my/feed` | 2984 |
| `GET` | `/my/kol-requests` | 3124 |
| `GET` | `/my/subscriptions` | 2976 |
| `GET` | `/news` | 2540 |
| `GET` | `/news/magazine` | 2523 |
| `POST` | `/news/read-all` | 2595 |
| `POST` | `/news/read-all/undo` | 2606 |
| `POST` | `/news/seen` | 2589 |
| `GET` | `/news/sources` | 2491 |
| `GET` | `/news/{article_id}` | 2625 |
| `GET` | `/news/{article_id}/images/{index}` | 2638 |
| `POST` | `/news/{article_id}/read` | 2616 |
| `GET` | `/posts` | 5899 |
| `GET` | `/push-logs` | 5903 |
| `GET` | `/recommendations` | 2390 |
| `GET` | `/stats` | 6005 |
| `POST` | `/subscriptions` | 2409 |
| `DELETE` | `/subscriptions/{kol_id}` | 2465 |
| `PUT` | `/subscriptions/{kol_id}` | 2426 |
| `PUT` | `/subscriptions/{kol_id}/favorite` | 2437 |
| `PUT` | `/subscriptions/{kol_id}/hide-images` | 2453 |
| `PUT` | `/subscriptions/{kol_id}/secondary` | 2445 |
| `GET` | `/tags` | 5681 |
| `PUT` | `/tags` | 5712 |
| `POST` | `/tags/backfill` | 5881 |
| `POST` | `/tags/maintain` | 5841 |
| `GET` | `/users` | 5917 |
| `DELETE` | `/users/{user_id}` | 6254 |
| `PUT` | `/users/{user_id}` | 6207 |
| `GET` | `/version` | 1758 |
| `POST` | `/xincai/ingest` | 2669 |

## Rust

| Method | Path | Line |
|---|---|---:|
| `GET` | `/api/admin/backup` | 369 |
| `GET` | `/api/admin/backup/download` | 372 |
| `POST` | `/api/admin/backup/restore/upload` | 377 |
| `POST` | `/api/admin/backup/restore/webdav` | 373 |
| `PUT` | `/api/admin/backup/webdav` | 370 |
| `POST` | `/api/admin/backup/webdav/test` | 371 |
| `GET` | `/api/admin/cicc/schedule` | 588 |
| `PUT` | `/api/admin/cicc/schedule` | 588 |
| `GET` | `/api/admin/cicc/status` | 586 |
| `POST` | `/api/admin/cicc/trigger` | 587 |
| `DELETE` | `/api/admin/cookies/{kind}` | 367 |
| `GET` | `/api/admin/dashboard` | 356 |
| `GET` | `/api/admin/error-logs` | 358 |
| `GET` | `/api/admin/feishu-documents` | 438 |
| `POST` | `/api/admin/feishu-documents` | 438 |
| `PUT` | `/api/admin/feishu-documents/config` | 442 |
| `GET` | `/api/admin/feishu-documents/oauth/callback` | 450 |
| `POST` | `/api/admin/feishu-documents/oauth/start` | 446 |
| `POST` | `/api/admin/feishu-documents/preview` | 454 |
| `DELETE` | `/api/admin/feishu-documents/{id}` | 462 |
| `PATCH` | `/api/admin/feishu-documents/{id}` | 462 |
| `POST` | `/api/admin/feishu-documents/{id}/sync` | 458 |
| `GET` | `/api/admin/ima-arm` | 583 |
| `GET` | `/api/admin/ima-collector` | 489 |
| `PUT` | `/api/admin/ima-collector` | 489 |
| `GET` | `/api/admin/ima-collector/cicc-categories` | 592 |
| `PUT` | `/api/admin/ima-collector/cicc-categories` | 592 |
| `POST` | `/api/admin/ima-collector/discover` | 493 |
| `PUT` | `/api/admin/ima-collector/groups/{group_id}/acl` | 502 |
| `GET` | `/api/admin/ima-collector/groups/{group_id}/folders` | 498 |
| `POST` | `/api/admin/ima-collector/sync` | 497 |
| `GET` | `/api/admin/ima-credentials` | 418 |
| `POST` | `/api/admin/ima-credentials` | 418 |
| `GET` | `/api/admin/ima-local-libraries` | 422 |
| `POST` | `/api/admin/ima-local-libraries` | 422 |
| `POST` | `/api/admin/ima-local-libraries/scan` | 426 |
| `PUT` | `/api/admin/ima-local-libraries/{slug}` | 434 |
| `PUT` | `/api/admin/ima-local-libraries/{slug}/enabled` | 430 |
| `DELETE` | `/api/admin/imgbed` | 351 |
| `GET` | `/api/admin/imgbed` | 351 |
| `PUT` | `/api/admin/imgbed` | 351 |
| `GET` | `/api/admin/inactive-users-policy` | 406 |
| `PUT` | `/api/admin/inactive-users-policy` | 406 |
| `GET` | `/api/admin/kol-requests` | 342 |
| `POST` | `/api/admin/kol-requests/{request_id}/approve` | 343 |
| `POST` | `/api/admin/kol-requests/{request_id}/reject` | 347 |
| `GET` | `/api/admin/kols` | 598 |
| `POST` | `/api/admin/kols/batch` | 599 |
| `GET` | `/api/admin/logs` | 357 |
| `GET` | `/api/admin/news/articles` | 582 |
| `DELETE` | `/api/admin/news/articles/{article_id}` | 578 |
| `POST` | `/api/admin/news/feeds/validate` | 560 |
| `DELETE` | `/api/admin/news/feeds/{feed_id}` | 573 |
| `PATCH` | `/api/admin/news/feeds/{feed_id}` | 573 |
| `POST` | `/api/admin/news/feeds/{feed_id}/archive` | 561 |
| `POST` | `/api/admin/news/feeds/{feed_id}/refresh` | 569 |
| `POST` | `/api/admin/news/feeds/{feed_id}/restore` | 565 |
| `POST` | `/api/admin/news/refresh` | 577 |
| `GET` | `/api/admin/news/settings` | 532 |
| `PATCH` | `/api/admin/news/settings` | 532 |
| `GET` | `/api/admin/news/sources` | 536 |
| `POST` | `/api/admin/news/sources` | 536 |
| `DELETE` | `/api/admin/news/sources/{source_id}` | 556 |
| `PATCH` | `/api/admin/news/sources/{source_id}` | 556 |
| `POST` | `/api/admin/news/sources/{source_id}/archive` | 544 |
| `POST` | `/api/admin/news/sources/{source_id}/feeds` | 540 |
| `POST` | `/api/admin/news/sources/{source_id}/refresh` | 552 |
| `POST` | `/api/admin/news/sources/{source_id}/restore` | 548 |
| `GET` | `/api/admin/plaza-sources` | 410 |
| `PUT` | `/api/admin/plaza-sources` | 410 |
| `GET` | `/api/admin/polling-config` | 360 |
| `PUT` | `/api/admin/polling-config` | 360 |
| `GET` | `/api/admin/proxies` | 466 |
| `POST` | `/api/admin/proxies` | 466 |
| `DELETE` | `/api/admin/proxies/{id}` | 471 |
| `POST` | `/api/admin/proxies/{id}/test` | 470 |
| `GET` | `/api/admin/proxy-pools` | 472 |
| `POST` | `/api/admin/proxy-pools` | 472 |
| `DELETE` | `/api/admin/proxy-pools/{id}` | 484 |
| `POST` | `/api/admin/proxy-pools/{id}/extract` | 480 |
| `POST` | `/api/admin/proxy-pools/{id}/import` | 476 |
| `GET` | `/api/admin/proxy-routes` | 485 |
| `PUT` | `/api/admin/proxy-routes` | 485 |
| `POST` | `/api/admin/register-code-batches/{batch_id}/revoke-unused` | 397 |
| `GET` | `/api/admin/register-codes` | 385 |
| `POST` | `/api/admin/register-codes` | 385 |
| `POST` | `/api/admin/register-codes/batch` | 389 |
| `POST` | `/api/admin/register-codes/{code}/revoke` | 393 |
| `GET` | `/api/admin/system-logs` | 359 |
| `POST` | `/api/admin/test-push` | 405 |
| `GET` | `/api/admin/turnstile` | 381 |
| `PUT` | `/api/admin/turnstile` | 381 |
| `POST` | `/api/admin/twitter-cookie` | 365 |
| `POST` | `/api/admin/users/batch` | 404 |
| `PUT` | `/api/admin/users/{user_id}/ima-kb` | 403 |
| `POST` | `/api/admin/weibo-qr/start` | 584 |
| `GET` | `/api/admin/weibo-qr/status` | 585 |
| `POST` | `/api/admin/xueqiu-cookie` | 364 |
| `POST` | `/api/admin/zsxq-cache/purge` | 368 |
| `POST` | `/api/admin/zsxq-cookie` | 366 |
| `POST` | `/api/auth/login` | 285 |
| `POST` | `/api/auth/logout` | 288 |
| `POST` | `/api/auth/register` | 286 |
| `GET` | `/api/auth/turnstile` | 284 |
| `POST` | `/api/auth/wechat` | 287 |
| `GET` | `/api/catalog` | 316 |
| `GET` | `/api/categories` | 318 |
| `POST` | `/api/categories` | 318 |
| `DELETE` | `/api/categories/{category_id}` | 319 |
| `PUT` | `/api/categories/{category_id}` | 319 |
| `GET` | `/api/ima-documents` | 507 |
| `GET` | `/api/ima-documents/catalog` | 506 |
| `DELETE` | `/api/ima-documents/groups/{group_id}/subscribe` | 414 |
| `POST` | `/api/ima-documents/groups/{group_id}/subscribe` | 414 |
| `GET` | `/api/ima-documents/tickers/{code}` | 509 |
| `GET` | `/api/ima-documents/timeline/all` | 508 |
| `GET` | `/api/ima-documents/{media_id}` | 520 |
| `GET` | `/api/ima-documents/{media_id}/assets/{asset_id}` | 514 |
| `GET` | `/api/ima-documents/{media_id}/pdf` | 518 |
| `GET` | `/api/ima-documents/{media_id}/text` | 519 |
| `POST` | `/api/ima-documents/{media_id}/translate` | 510 |
| `GET` | `/api/img-proxy` | 607 |
| `POST` | `/api/kol-requests` | 340 |
| `GET` | `/api/kols` | 596 |
| `POST` | `/api/kols` | 596 |
| `POST` | `/api/kols/batch` | 597 |
| `DELETE` | `/api/kols/{kol_id}` | 600 |
| `GET` | `/api/kols/{kol_id}` | 600 |
| `PUT` | `/api/kols/{kol_id}` | 600 |
| `GET` | `/api/kols/{kol_id}/holdings` | 605 |
| `GET` | `/api/kols/{kol_id}/nav` | 606 |
| `GET` | `/api/kols/{kol_id}/posts` | 604 |
| `GET` | `/api/live/wscn` | 608 |
| `GET` | `/api/market/indices` | 609 |
| `GET` | `/api/me` | 309 |
| `PUT` | `/api/me` | 309 |
| `POST` | `/api/me/bind-code` | 308 |
| `DELETE` | `/api/me/feishu-personal` | 307 |
| `POST` | `/api/me/feishu-personal/register` | 291 |
| `GET` | `/api/me/feishu-personal/register/{session_id}` | 303 |
| `POST` | `/api/me/feishu-personal/register/{session_id}/cancel` | 299 |
| `POST` | `/api/me/feishu-personal/register/{session_id}/refresh-code` | 295 |
| `POST` | `/api/me/llm-models` | 289 |
| `POST` | `/api/me/llm-test` | 290 |
| `POST` | `/api/me/password` | 310 |
| `DELETE` | `/api/me/webpush` | 311 |
| `POST` | `/api/me/webpush` | 311 |
| `GET` | `/api/media/zsxq-file/{file_id}` | 529 |
| `GET` | `/api/my/feed` | 315 |
| `GET` | `/api/my/kol-requests` | 341 |
| `GET` | `/api/my/subscriptions` | 339 |
| `GET` | `/api/news` | 523 |
| `GET` | `/api/news/magazine` | 527 |
| `POST` | `/api/news/read-all` | 525 |
| `POST` | `/api/news/read-all/undo` | 526 |
| `POST` | `/api/news/seen` | 524 |
| `GET` | `/api/news/sources` | 522 |
| `GET` | `/api/news/{article_id}` | 531 |
| `GET` | `/api/news/{article_id}/images/{index}` | 528 |
| `POST` | `/api/news/{article_id}/read` | 530 |
| `GET` | `/api/posts` | 323 |
| `GET` | `/api/push-logs` | 324 |
| `GET` | `/api/recommendations` | 317 |
| `GET` | `/api/stats` | 355 |
| `POST` | `/api/subscriptions` | 328 |
| `DELETE` | `/api/subscriptions/{kol_id}` | 329 |
| `PUT` | `/api/subscriptions/{kol_id}` | 329 |
| `PUT` | `/api/subscriptions/{kol_id}/favorite` | 333 |
| `PUT` | `/api/subscriptions/{kol_id}/hide-images` | 335 |
| `PUT` | `/api/subscriptions/{kol_id}/secondary` | 334 |
| `GET` | `/api/tags` | 325 |
| `PUT` | `/api/tags` | 325 |
| `POST` | `/api/tags/backfill` | 327 |
| `POST` | `/api/tags/maintain` | 326 |
| `GET` | `/api/users` | 401 |
| `DELETE` | `/api/users/{user_id}` | 402 |
| `PUT` | `/api/users/{user_id}` | 402 |
| `GET` | `/api/version` | 283 |
| `POST` | `/api/xincai/ingest` | 521 |
| `GET` | `/healthz` | 282 |
