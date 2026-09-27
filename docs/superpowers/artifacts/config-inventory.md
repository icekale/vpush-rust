# Configuration Inventory

Generated during Task 1 from the production `api.py` and Rust `src/**/*.rs` source scan. Values are intentionally excluded. This is a name inventory, not a secret export or deployment configuration.

## Production Python names discovered

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

## Rust source names discovered

- `ALERTS_ENABLED`
- `FEISHU_ARCHIVE_ROOT`

## Limitations

- The static source scan finds direct environment access only; settings loaded from SQLite, compose interpolation, shell scripts, or framework settings require a separate runtime inventory.
- No values, tokens, private keys, or production `.env` content is included.
- Before deployment, each production variable must be classified as Rust-mapped, intentionally Python-only, or a blocking missing capability.
