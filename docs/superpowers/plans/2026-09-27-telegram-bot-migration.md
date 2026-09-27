# Telegram Bot Complete Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Python Telegram shared bot and user-owned bots to ARM Rust without losing existing bindings, secrets, commands, media, digests, or retries.

**Architecture:** Keep `Db` responsible for identity, bind codes, subscriptions and retry persistence. Introduce a Telegram-only adapter for API requests, command processing and rendering; reuse the existing per-user push selection. Convert legacy Fernet secrets on an isolated copy of the current ARM database, not by replacing that database with a stale DMIT snapshot. Production inbound polling starts only after one-owner checks and staged tests.

**Tech Stack:** Rust 1.95, Axum/Tokio, SQLite/sqlx, ureq, serde_json, AES-GCM/base64 already present; Python 3 `sqlite3` and `cryptography` on ARM for offline conversion.

**Reference:** `docs/superpowers/specs/2026-09-27-telegram-rust-migration-design.md`. Python behavior: DMIT `/opt/vpush/src/app/{telegram_bot.py,bot_core.py,channels.py,notifiers/telegram.py,notifiers/telegram_rich.py}`. Do not copy secrets to tests, repo, logs, or terminal output.

---

### Task 1: Encrypted token boundary

**Files:** Modify `src/push.rs:536-555`, `src/main.rs:5164-5221`, `src/db.rs:3080-3095`; test in `src/push.rs` and `src/main.rs` existing test modules.

- [ ] Add failing tests: `telegram_token_for("enc1:gAAAA...")` must not return a shared token, and a valid `enc2:` AES-GCM token round-trips using a test-only 32-byte base64 key. Existing bare plaintext tokens remain readable while converting. A failed decrypt must cause an error, not fallback.
- [ ] Run `cargo test --bin vpush telegram_token`; expect failure before implementation.
- [ ] Add one `telegram_secret(stored: &str, key: &str) -> Result<String, String>` at the read boundary. `enc2:` calls `feishu_personal::open_app_secret(key, &stored[5..])`; `enc1:` yields an explicit migration-required error; bare plaintext is accepted for legacy support. Empty value means use shared `TELEGRAM_BOT_TOKEN`; nonempty invalid values never do.
- [ ] At `apply_profile`, encrypt newly entered custom tokens with `feishu_personal::seal(key, value)` and store `enc2:` plus ciphertext. Reject writes when the credential key is missing. For uniqueness, compare decrypted values across the small user set or add a deterministic SHA-256 fingerprint column; do not compare randomized ciphertext with `other_user_has`.
- [ ] Re-run focused tests, `cargo clippy --all-targets --all-features -- -D warnings`, commit only this task.

### Task 2: Safe offline legacy-secret conversion

**Files:** Create `tools/migrate_telegram_secrets.py`; test `tools/test_migrate_telegram_secrets.py` using a temporary SQLite DB and ephemeral Fernet key. Do not use live credentials in tests.

- [ ] Write a failing test inserting one Fernet-wrapped `enc1:` token and one bare legacy token, then verify dry-run counts, AES-GCM round-trip, and second-run idempotency. Wrong key must leave the DB byte-for-byte unchanged.
- [ ] Run `python3 -m unittest tools/test_migrate_telegram_secrets.py -v`; expect failure.
- [ ] Implement CLI requiring an explicit path to an **offline copy** and credential key via protected file descriptor or environment (never positional arguments). Preflight every `enc1:` token with `Fernet(key).decrypt`, calculate all replacements in memory, then execute one `BEGIN IMMEDIATE` transaction. Produce `enc2:` + `base64.urlsafe_b64encode(nonce + AESGCM(decoded_key).encrypt(nonce, value.encode(), None)).decode()`; 12 random nonce bytes. Keep existing `enc2:` and empty values. Never print token/key/ciphertext, only row counts and pass/fail.
- [ ] Compare counts and SQLite `PRAGMA quick_check` on a separately backed-up ARM copy before considering production. Original DMIT DB is never overwritten; live ARM DB is never replaced by the DMIT snapshot.
- [ ] Run test, `git diff --check`, commit.

### Task 3: Telegram API transport and routing

**Files:** Create `src/telegram.rs`; modify `src/main.rs` module declaration, `src/push.rs:30-84,130-148,381-425,536-654`; tests in `src/telegram.rs`.

- [ ] Test fake Telegram HTTP responses for `ok:false`, 429 `parameters.retry_after`, transport failure, and token-bearing error text. Assert no token in emitted error.
- [ ] Implement `send(token, method, payload)` with existing ureq timeouts; `sendMessage` accepts JSON with `chat_id`, `text`, `parse_mode: "HTML"`, `disable_web_page_preview: true`. Guard invalid/missing token or chat as an error whenever Telegram was selected; never mark a skipped send as success.
- [ ] Route `send_user_text`, `deliver` and `retry_due_live` through one custom/shared token resolution boundary. Preserve `channels`, DND, disabled-user, and existing `(channel, user_id, post_id)` retry key. Record per-user send success/failure without response bodies or token.
- [ ] Test that custom beats shared, wrong-key encrypted custom token never sends via shared, and failure is queued with the intended user ID. Run focused and all tests; commit.

### Task 4: Persistent inbound identity and cursor

**Files:** Modify `src/db.rs` schema/migrations and tests; create `src/telegram_bot.rs` for update decoding.

- [ ] Test an in-memory DB: Telegram private chat identity creates/looks up one user; group chat never creates one; two pollers cannot both claim the same update; process restart does not replay committed update; bind to an existing web user does not leave the old provisional account linked.
- [ ] Implement `Db::user_by_telegram_chat_id(&str)`, transaction-safe first-contact creation mirroring Python display-name fallback, a persisted `telegram_update_offset` setting, and atomic update claim/commit. Use existing `consume_bind_code` for single-use codes. Do not use web session tokens for Telegram identity.
- [ ] Run DB/adapter tests, commit.

### Task 5: Shared bot command and callback contract

**Files:** Extend `src/telegram_bot.rs`, `src/db.rs` only for missing server-enforced query or binding; tests in `src/telegram_bot.rs`.

- [ ] Use captured *synthetic* updates (no real user data) to test Python-compatible `/start`, `/help`, `/list` 20/page, `/search` 10 results, `/sub` (`post|reply|both`), `/unsub`, `/mysubs`, `/bind`, `/start bind_<code>`, and pasted code. Use `Db::catalog`, `kol_for`, `subscribe`, `unsubscribe`, `set_subscription_type`, `my_subscriptions`, `consume_bind_code`; server-side visibility and idempotency stay in Db.
- [ ] Test `list:`, `sub:`, `unsub:`, `mysubs:type:`, `mysubs:unsub:`, help, secondary/undo and malformed callbacks. Only private-chat messages may mutate state; callback actor is looked up by chat ID. Add bind-attempt throttling so guesses cannot consume unlimited codes.
- [ ] Implement parser and response serializer with `sendMessage`, `editMessageText`, `answerCallbackQuery`. Escape HTML and bound message sizes; invalid identifiers return a user message instead of panicking. Run focused tests and commit.

### Task 6: Requests and admin approvals

**Files:** Extend `src/telegram_bot.rs`, use `src/db.rs:5818-6030` and existing profile/avatar resolution from HTTP admin approval routes in `src/main.rs`; tests in `src/telegram_bot.rs`.

- [ ] Test `/ask`, category selection, `approve:`, `apcat:`, `reject:` with one admin and one non-admin. Repeated callback must not repeat approval/subscription. Assert requester notification, avatar/profile resolution and actor audit are present as in Python `kol_requests.py`.
- [ ] Reuse Db request/status methods, authorize `User.is_admin` *before* any mutation, and consolidate HTTP/bot approval effects in the shared service so both produce the same outcome. Never trust callback `chat_id`, category, or requester identity alone.
- [ ] Run tests, commit.

### Task 7: One-owner long polling

**Files:** Extend `src/telegram_bot.rs`; modify `src/main.rs:214-270` startup and shutdown; tests with a fake API server.

- [ ] Test `getUpdates` with `offset` and `timeout=30`: only accepted private-chat updates process; processed updates commit offset; failed processing is retryable; service shutdown cancels polling. Starting without `TELEGRAM_BOT_TOKEN` or with `TELEGRAM_BOT_INBOUND=0` does not call API.
- [ ] Spawn exactly one worker guarded by explicit `TELEGRAM_BOT_INBOUND` and a single-process ownership guard; redact token in request/error logs. Self-hosted discovery `resolve_telegram_bot` must not poll the shared token while this worker runs.
- [ ] Run focused tests and commit. Do **not** set `TELEGRAM_BOT_INBOUND=1` on production yet.

### Task 8: Python-equivalent text and rich messages

**Files:** Extend `src/telegram.rs` with rendering; modify `src/push.rs::deliver` and retry query; tests in `src/telegram.rs`.

- [ ] Golden tests for HTML escaping, reply blockquote, favorite/keyword/translation badges, category and tags, `detail.files`, published time, Xueqiu combination trades, images (max 9) and rich-message fallback to HTML text.
- [ ] Load full post metadata in initial delivery **and retry**; pass the same per-user favorite/keyword context. Select rich vs HTML using existing `config_telegram_rich_messages`; when rich fails, try HTML and preserve original post identity in retry logs.
- [ ] Compare synthetic rendering with Python `notifiers/telegram.py` and `telegram_rich.py`; run tests and commit.

### Task 9: Media, digest, daily, DND and transport failure

**Files:** Extend `src/telegram.rs`, `src/push.rs` and `src/maintenance.rs:235-255`; tests in their existing test modules.

- [ ] Test photo/media-group/video routing, URL fallback to upload, image/video counts, and media download guard; test rich failure fallback, bounded 429 retry and global rate limit (15 messages/sec). URL fetches must reuse `url_guard::public_resolver` and disable redirects.
- [ ] Test digest/daily/DND summary text and buttons, ten-item limit and overflow; wire existing scheduled delivery through Telegram-specific renderer without changing other channels.
- [ ] Keep ambiguous network failures in retry rather than sending the same item through a second bot. Run full tests/clippy/fmt and commit.

### Task 10: Canary and production cutover gate

**Files:** Modify `compose.yaml` only for protected Telegram env-file reference/toggle after code/tests pass; write non-secret verification evidence under `docs/superpowers/artifacts/`.

- [ ] Run `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo fmt --check`; capture test counts, no tokens.
- [ ] Deploy image to isolated ARM canary with separate DB copy, separate test Telegram bot, outbound disabled except test recipients. Verify all commands, bindings, buttons, images, plain/rich, custom/shared routing, retries and reboot cursor. Compare encrypted token counts to current ARM DB.
- [ ] Back up current ARM DB; verify Python shared bot listener is stopped and no other `getUpdates` consumer uses production token. Supply protected token/key to ARM without shell-argument/history exposure; preflight decrypt all legacy secrets and `PRAGMA quick_check`. Enable only after a named test account successfully receives a controlled notification.
- [ ] Verify no duplicate consumers, no token leakage, user-visible parity, per-user push logs and retry queue. On failure **stop Rust poller/outbound before starting Python**, reconcile bindings created during gray period rather than overwriting ARM DB.

**Feishu is a separate approved stage**: `docs/superpowers/specs/2026-09-27-feishu-bot-rust-migration-design.md`. Do not turn on Feishu from this plan.
