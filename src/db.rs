use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, Connection, Row, SqlitePool};

#[derive(Debug)]
pub struct KeywordDigest {
    pub user_id: i64,
    pub text: String,
    pub articles: Vec<i64>,
    pub docs: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct NewsEntry {
    pub external_id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub url: String,
    pub author: String,
    pub published_at: String,
}

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub token_version: i64,
    pub created_at: String,
    pub telegram_chat_id: String,
    pub telegram_bot_token: String,
    pub feishu_open_id: String,
    pub feishu_chat_id: String,
    pub wecom_webhook: String,
    pub bark_key: String,
    pub notify_enabled: bool,
    pub daily_report: bool,
    pub translate_twitter: bool,
    pub push_channels: String,
    pub dnd_start: String,
    pub dnd_end: String,
    pub dnd_allow_favorite: bool,
    pub keywords: String,
    pub keywords_match_reports: bool,
    pub keywords_match_news: bool,
    pub news_font_size: String,
    pub llm_api_base: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_api_format: String,
    #[allow(dead_code)]
    pub wechat_openid: String,
}

#[derive(Debug, Clone)]
pub struct WebPushSub {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone)]
pub struct FeishuSession {
    pub session_id: String,
    pub user_id: i64,
    pub device_code_ciphertext: String,
    pub registration_base_url: String,
    pub verification_uri: String,
    pub candidate_app_id: String,
    pub candidate_app_secret_ciphertext: String,
    pub candidate_tenant_brand: String,
    pub expected_open_id: String,
    pub bind_code_hash: String,
    pub bind_code_expires_at: Option<i64>,
    pub session_expires_at: i64,
    pub poll_interval: i64,
    pub status: String,
    pub last_error: String,
}

#[derive(Debug, Clone)]
pub struct FeishuBot {
    pub status: String,
    pub app_id: String,
}

pub struct FeishuRoute {
    pub app_id: String,
    pub app_secret_ciphertext: String,
    pub chat_id: String,
    pub tenant_brand: String,
}

fn feishu_session_from_row(row: sqlx::sqlite::SqliteRow) -> FeishuSession {
    FeishuSession {
        session_id: row.get("session_id"),
        user_id: row.get("user_id"),
        device_code_ciphertext: row.get("device_code_ciphertext"),
        registration_base_url: row.get("registration_base_url"),
        verification_uri: row.get("verification_uri"),
        candidate_app_id: row.get("candidate_app_id"),
        candidate_app_secret_ciphertext: row.get("candidate_app_secret_ciphertext"),
        candidate_tenant_brand: row.get("candidate_tenant_brand"),
        expected_open_id: row.get("expected_open_id"),
        bind_code_hash: row.get("bind_code_hash"),
        bind_code_expires_at: row.get("bind_code_expires_at"),
        session_expires_at: row.get("session_expires_at"),
        poll_interval: row.get("poll_interval"),
        status: row.get("status"),
        last_error: row.get("last_error"),
    }
}

#[derive(Debug, Clone)]
pub struct PushTarget {
    pub notify_enabled: bool,
    pub push_channels: String,
    pub dnd_start: String,
    pub dnd_end: String,
    pub dnd_allow_favorite: bool,
    pub favorite: bool,
    pub wecom_webhook: String,
    pub bark_key: String,
    pub user_id: i64,
    pub telegram_chat_id: String,
    pub telegram_bot_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KolRequestEffect {
    pub request_id: i64,
    pub kol_id: Option<i64>,
    pub applicant_user_id: i64,
    pub applicant_chat_id: String,
    pub applicant_name: String,
    pub platform: String,
    pub external_id: String,
    pub name: String,
    pub category_id: Option<i64>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_poll_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    owner TEXT NOT NULL DEFAULT '',
    lease_until INTEGER NOT NULL DEFAULT 0,
    offset INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO telegram_poll_state (id, owner, lease_until, offset) VALUES (1, '', 0, 0);
CREATE TABLE IF NOT EXISTS hosted_images (
    source_url TEXT PRIMARY KEY,
    hosted_url TEXT NOT NULL DEFAULT '',
    content_hash TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT NOT NULL DEFAULT '',
    last_attempt_at TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL DEFAULT '',
    is_admin INTEGER NOT NULL DEFAULT 0,
    token_version INTEGER NOT NULL DEFAULT 0,
    last_login_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    telegram_chat_id TEXT NOT NULL DEFAULT '',
    telegram_bot_token TEXT NOT NULL DEFAULT '',
    feishu_open_id TEXT NOT NULL DEFAULT '',
    feishu_chat_id TEXT NOT NULL DEFAULT '',
    wecom_webhook TEXT NOT NULL DEFAULT '',
    bark_key TEXT NOT NULL DEFAULT '',
    notify_enabled INTEGER NOT NULL DEFAULT 1,
    daily_report INTEGER NOT NULL DEFAULT 0,
    translate_twitter INTEGER NOT NULL DEFAULT 1,
    push_channels TEXT NOT NULL DEFAULT '',
    dnd_start TEXT NOT NULL DEFAULT '',
    dnd_end TEXT NOT NULL DEFAULT '',
    dnd_allow_favorite INTEGER NOT NULL DEFAULT 0,
    keywords TEXT NOT NULL DEFAULT '[]',
    keywords_match_reports INTEGER NOT NULL DEFAULT 0,
    keywords_match_reports_since TEXT NOT NULL DEFAULT '',
    keywords_match_news INTEGER NOT NULL DEFAULT 0,
    keywords_match_news_since TEXT NOT NULL DEFAULT '',
    news_font_size TEXT NOT NULL DEFAULT '',
    llm_api_base TEXT NOT NULL DEFAULT '',
    llm_api_key TEXT NOT NULL DEFAULT '',
    llm_model TEXT NOT NULL DEFAULT '',
    llm_api_format TEXT NOT NULL DEFAULT 'chat',
    wechat_openid TEXT NOT NULL DEFAULT '',
    telegram_provisional INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_wechat_openid ON users(wechat_openid) WHERE wechat_openid != '';
CREATE TABLE IF NOT EXISTS bind_codes (
    code TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS bind_quota (
    key TEXT PRIMARY KEY,
    period_start INTEGER NOT NULL,
    count INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS feishu_personal_bots (
    user_id INTEGER PRIMARY KEY,
    app_id TEXT NOT NULL UNIQUE,
    app_secret_ciphertext TEXT NOT NULL,
    open_id TEXT NOT NULL DEFAULT '',
    chat_id TEXT NOT NULL DEFAULT '',
    tenant_brand TEXT NOT NULL DEFAULT 'feishu',
    status TEXT NOT NULL,
    last_error TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS feishu_registration_sessions (
    session_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    device_code_ciphertext TEXT NOT NULL,
    registration_base_url TEXT NOT NULL,
    verification_uri TEXT NOT NULL,
    candidate_app_id TEXT NOT NULL DEFAULT '',
    candidate_app_secret_ciphertext TEXT NOT NULL DEFAULT '',
    candidate_tenant_brand TEXT NOT NULL DEFAULT 'feishu',
    expected_open_id TEXT NOT NULL DEFAULT '',
    bind_code_hash TEXT NOT NULL DEFAULT '',
    bind_code_expires_at INTEGER,
    session_expires_at INTEGER NOT NULL,
    poll_interval INTEGER NOT NULL,
    status TEXT NOT NULL,
    last_error TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS register_codes (
    code TEXT PRIMARY KEY,
    note TEXT NOT NULL DEFAULT '',
    used_by INTEGER,
    used_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    batch_id TEXT NOT NULL DEFAULT '',
    expires_at TEXT,
    revoked_at TEXT,
    created_by INTEGER
);
CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE
);
CREATE TABLE IF NOT EXISTS kols (
    id INTEGER PRIMARY KEY,
    platform TEXT NOT NULL,
    name TEXT NOT NULL,
    external_id TEXT NOT NULL,
    avatar_url TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    is_private INTEGER NOT NULL DEFAULT 0,
    original_only INTEGER NOT NULL DEFAULT 0,
    secondary INTEGER NOT NULL DEFAULT 0,
    priority INTEGER NOT NULL DEFAULT 0,
    recommend_weight INTEGER NOT NULL DEFAULT 0,
    category_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (platform, external_id)
);
CREATE TABLE IF NOT EXISTS kol_acl (
    kol_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    PRIMARY KEY (kol_id, user_id)
);
CREATE TABLE IF NOT EXISTS subscriptions (
    user_id INTEGER NOT NULL,
    kol_id INTEGER NOT NULL,
    type TEXT NOT NULL DEFAULT 'post',
    favorite INTEGER NOT NULL DEFAULT 0,
    secondary INTEGER NOT NULL DEFAULT 0,
    hide_images INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, kol_id)
);
CREATE TABLE IF NOT EXISTS kol_requests (
    id INTEGER PRIMARY KEY,
    platform TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    external_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    category_id INTEGER,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    handled_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS uq_kol_requests_pending
    ON kol_requests(platform, external_id) WHERE status = 'pending';
CREATE TABLE IF NOT EXISTS posts (
    id INTEGER PRIMARY KEY,
    platform TEXT NOT NULL,
    kol_id INTEGER NOT NULL,
    external_id TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    title_src TEXT NOT NULL DEFAULT '',
    content_src TEXT NOT NULL DEFAULT '',
    post_type TEXT NOT NULL DEFAULT '',
    images TEXT NOT NULL DEFAULT '[]',
    tags TEXT NOT NULL DEFAULT '[]',
    url TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',
    published_at TEXT NOT NULL DEFAULT '',
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (platform, external_id)
);
CREATE INDEX IF NOT EXISTS idx_posts_feed ON posts (kol_id, published_at, id);
CREATE TABLE IF NOT EXISTS push_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id INTEGER NOT NULL,
    channel TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER
);
CREATE TABLE IF NOT EXISTS push_retries (
    channel TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    platform TEXT NOT NULL,
    external_id TEXT NOT NULL,
    post_id INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_at INTEGER NOT NULL,
    PRIMARY KEY (channel, user_id, platform, external_id)
);
CREATE TABLE IF NOT EXISTS admin_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER,
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS error_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    level TEXT NOT NULL,
    logger TEXT NOT NULL DEFAULT '',
    message TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS platform_runs (
    platform TEXT PRIMARY KEY,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_error TEXT NOT NULL DEFAULT '',
    last_success_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS source_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    platform TEXT NOT NULL,
    status TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    ok_count INTEGER NOT NULL DEFAULT 0,
    fail_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS cube_snapshots (
    kol_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (kol_id, kind)
);
CREATE TABLE IF NOT EXISTS feishu_document_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_key_hash TEXT UNIQUE NOT NULL,
    group_id TEXT UNIQUE NOT NULL,
    media_id TEXT UNIQUE NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    canonical_url TEXT NOT NULL DEFAULT '',
    revision_id TEXT NOT NULL DEFAULT '',
    timeline_path TEXT NOT NULL DEFAULT '',
    asset_root TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    last_success_at TEXT NOT NULL DEFAULT '',
    deleted_at TEXT
);
CREATE TABLE IF NOT EXISTS feishu_oauth_sessions (
    state_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    code_verifier TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS feishu_oauth_credentials (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL DEFAULT '',
    expires_at INTEGER NOT NULL DEFAULT 0,
    refresh_expires_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS news_sources (
    id INTEGER PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    group_name TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1,
    kind TEXT NOT NULL DEFAULT 'feed',
    internal INTEGER NOT NULL DEFAULT 0,
    archived_at TEXT,
    last_success_at TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS news_feeds (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    archived_at TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_success_at TEXT NOT NULL DEFAULT '',
    last_error_detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS news_articles (
    id INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL,
    feed_id INTEGER,
    external_id TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL DEFAULT '',
    author TEXT NOT NULL DEFAULT '',
    published_at TEXT NOT NULL DEFAULT '',
    issue_key TEXT NOT NULL DEFAULT '',
    issue_label TEXT NOT NULL DEFAULT '',
    issue_title TEXT NOT NULL DEFAULT '',
    issue_cover TEXT NOT NULL DEFAULT '',
    section TEXT NOT NULL DEFAULT '',
    toc_order INTEGER NOT NULL DEFAULT 0,
    images TEXT NOT NULL DEFAULT '[]',
    fetched_at TEXT NOT NULL DEFAULT '',
    UNIQUE (source_id, external_id)
);
CREATE TABLE IF NOT EXISTS news_keyword_notified (
    user_id INTEGER NOT NULL,
    article_id INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, article_id)
);
CREATE TABLE IF NOT EXISTS knowledge_keyword_notified (
    user_id INTEGER NOT NULL,
    group_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, group_id, media_id)
);
CREATE TABLE IF NOT EXISTS news_reads (
    user_id INTEGER NOT NULL,
    article_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, article_id)
);
CREATE TABLE IF NOT EXISTS news_seen (
    user_id INTEGER PRIMARY KEY,
    seen_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS user_news_sources (
    user_id INTEGER NOT NULL,
    source_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, source_id)
);
CREATE TABLE IF NOT EXISTS ima_kb_acl (
    group_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    PRIMARY KEY (group_id, user_id)
);
CREATE TABLE IF NOT EXISTS ima_kb_subscriptions (
    user_id INTEGER NOT NULL,
    group_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, group_id)
);
CREATE INDEX IF NOT EXISTS idx_ima_kb_acl_user ON ima_kb_acl(user_id);
CREATE INDEX IF NOT EXISTS idx_ima_kb_sub_group ON ima_kb_subscriptions(group_id);
CREATE TABLE IF NOT EXISTS proxy_pools (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL DEFAULT 'static',
    extract_url TEXT NOT NULL DEFAULT '',
    protocol TEXT NOT NULL DEFAULT 'http',
    expire_seconds INTEGER NOT NULL DEFAULT 0,
    refresh_interval_seconds INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_extract_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS proxies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pool_id INTEGER NOT NULL,
    protocol TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    password TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT 'manual',
    status TEXT NOT NULL DEFAULT 'unknown',
    fail_count INTEGER NOT NULL DEFAULT 0,
    last_ok_at INTEGER,
    last_fail_at INTEGER,
    last_error TEXT NOT NULL DEFAULT '',
    expires_at INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (pool_id, protocol, host, port, username)
);
CREATE INDEX IF NOT EXISTS idx_proxies_pool ON proxies(pool_id);
CREATE TABLE IF NOT EXISTS webpush_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    endpoint TEXT NOT NULL UNIQUE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    user_agent TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_webpush_user ON webpush_subscriptions(user_id);
CREATE TABLE IF NOT EXISTS android_devices (
    installation_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    token TEXT NOT NULL,
    provider TEXT NOT NULL,
    device_model TEXT NOT NULL DEFAULT '',
    app_version TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_android_devices_user ON android_devices(user_id);
CREATE TABLE IF NOT EXISTS ima_document_index (
    group_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    day TEXT NOT NULL DEFAULT '',
    sort_date TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    group_name TEXT NOT NULL DEFAULT '',
    abstract TEXT NOT NULL DEFAULT '',
    size INTEGER NOT NULL DEFAULT 0,
    chars INTEGER NOT NULL DEFAULT 0,
    pdf_path TEXT NOT NULL DEFAULT '',
    txt_path TEXT NOT NULL DEFAULT '',
    downloaded_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (group_id, media_id)
);
CREATE INDEX IF NOT EXISTS idx_ima_doc_latest ON ima_document_index(sort_date DESC, name DESC, group_id ASC, media_id ASC);
CREATE INDEX IF NOT EXISTS idx_ima_doc_group_latest ON ima_document_index(group_id, sort_date DESC, name DESC, media_id ASC);
CREATE TABLE IF NOT EXISTS report_extractions (
    group_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    txt_hash TEXT NOT NULL DEFAULT '',
    report_kind TEXT NOT NULL DEFAULT '',
    rating TEXT NOT NULL DEFAULT '',
    target_price TEXT NOT NULL DEFAULT '',
    thesis TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'ok',
    extracted_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (group_id, media_id)
);
CREATE TABLE IF NOT EXISTS report_extraction_tickers (
    group_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    code TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    stance TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (group_id, media_id, code)
);
CREATE TABLE IF NOT EXISTS ima_ticker_digests (
    kind TEXT NOT NULL DEFAULT 'ticker',
    code TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    signature TEXT NOT NULL DEFAULT '',
    source_count INTEGER NOT NULL DEFAULT 0,
    digest TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    updated_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (kind, code)
);
CREATE TABLE IF NOT EXISTS ima_abstract_translations (
    group_id TEXT NOT NULL,
    media_id TEXT NOT NULL,
    src_hash TEXT NOT NULL DEFAULT '',
    abstract_zh TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (group_id, media_id)
);
";

#[allow(dead_code)]
impl Db {
    pub async fn open(path: &Path) -> Result<Self, sqlx::Error> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
            }
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5))
            .log_slow_statements(
                log::LevelFilter::Warn,
                std::time::Duration::from_millis(500),
            );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        ensure_user_columns(&pool).await?;
        ensure_users_autoincrement(&pool).await?;
        ensure_kol_columns(&pool).await?;
        ensure_news_article_columns(&pool).await?;
        ensure_register_code_columns(&pool).await?;
        ensure_feishu_columns(&pool).await?;
        ensure_news_admin_columns(&pool).await?;
        ensure_hot_indexes(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn telegram_poll_acquire(
        &self,
        owner: &str,
        lease_secs: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query(
            "INSERT OR IGNORE INTO telegram_poll_state (id, owner, lease_until, offset) VALUES (1, '', 0, 0)",
        )
        .execute(&self.pool)
        .await?;
        let result = sqlx::query(
            "UPDATE telegram_poll_state
             SET owner = ?, lease_until = unixepoch() + ?
             WHERE id = 1 AND (lease_until <= unixepoch() OR owner = ?)",
        )
        .bind(owner)
        .bind(lease_secs)
        .bind(owner)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Ok(None);
        }
        sqlx::query_scalar("SELECT offset FROM telegram_poll_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await
            .map(Some)
    }

    pub async fn telegram_poll_is_owner(&self, owner: &str) -> Result<bool, sqlx::Error> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM telegram_poll_state
             WHERE id = 1 AND owner = ? AND lease_until > unixepoch()",
        )
        .bind(owner)
        .fetch_one(&self.pool)
        .await?;
        Ok(count == 1)
    }
    pub async fn telegram_poll_heartbeat(
        &self,
        owner: &str,
        lease_secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE telegram_poll_state
             SET lease_until = unixepoch() + ?
             WHERE id = 1 AND owner = ? AND lease_until > unixepoch()",
        )
        .bind(lease_secs)
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn telegram_poll_save_offset(
        &self,
        owner: &str,
        offset: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE telegram_poll_state
             SET offset = ?
             WHERE id = 1 AND owner = ? AND lease_until > unixepoch() AND offset <= ?",
        )
        .bind(offset)
        .bind(owner)
        .bind(offset)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn telegram_poll_release(&self, owner: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE telegram_poll_state SET owner = '', lease_until = 0
             WHERE id = 1 AND owner = ?",
        )
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn telegram_poll_offset(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT offset FROM telegram_poll_state WHERE id = 1")
            .fetch_one(&self.pool)
            .await
    }

    pub async fn active_feishu_sources(&self) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, group_id, media_id, title, canonical_url, revision_id, last_success_at, timeline_path \
             FROM feishu_document_sources WHERE enabled = 1 AND deleted_at IS NULL AND timeline_path != '' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "group_id": row.get::<String, _>("group_id"),
                    "media_id": row.get::<String, _>("media_id"),
                    "title": row.get::<String, _>("title"),
                    "canonical_url": row.get::<String, _>("canonical_url"),
                    "revision_id": row.get::<String, _>("revision_id"),
                    "last_success_at": row.get::<String, _>("last_success_at"),
                    "timeline_path": row.get::<String, _>("timeline_path"),
                })
            })
            .collect())
    }

    pub async fn insert_feishu_source(
        &self,
        group_id: &str,
        media_id: &str,
        title: &str,
        timeline_path: &str,
        asset_root: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO feishu_document_sources (source_key_hash, group_id, media_id, title, timeline_path, asset_root) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(group_id)
        .bind(group_id)
        .bind(media_id)
        .bind(title)
        .bind(timeline_path)
        .bind(asset_root)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn feishu_asset_root(
        &self,
        media_id: &str,
        group: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT group_id, asset_root FROM feishu_document_sources \
             WHERE media_id = ? AND enabled = 1 AND deleted_at IS NULL AND (? = '' OR group_id = ?) LIMIT 1",
        )
        .bind(media_id)
        .bind(group)
        .bind(group)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| (row.get("group_id"), row.get("asset_root"))))
    }

    pub async fn feishu_document(
        &self,
        media_id: &str,
        group: &str,
    ) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT media_id, group_id, title, canonical_url, last_success_at FROM feishu_document_sources \
             WHERE media_id = ? AND enabled = 1 AND deleted_at IS NULL AND (? = '' OR group_id = ?) LIMIT 1",
        )
        .bind(media_id)
        .bind(group)
        .bind(group)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| {
            json!({
                "media_id": row.get::<String, _>("media_id"),
                "group_id": row.get::<String, _>("group_id"),
                "title": row.get::<String, _>("title"),
                "canonical_url": row.get::<String, _>("canonical_url"),
                "last_success_at": row.get::<String, _>("last_success_at"),
            })
        }))
    }

    pub async fn feishu_group_exists(&self, group_id: &str) -> Result<bool, sqlx::Error> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM feishu_document_sources WHERE group_id = ? AND enabled = 1 AND deleted_at IS NULL",
        )
        .bind(group_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    pub async fn ima_group_known(&self, group_id: &str) -> Result<bool, sqlx::Error> {
        if self.feishu_group_exists(group_id).await? {
            return Ok(true);
        }
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ima_document_index WHERE group_id = ?")
                .bind(group_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(n > 0)
    }

    pub async fn record_ima_listing(
        &self,
        group_id: &str,
        group_name: &str,
        file: &crate::ima_client::File,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO ima_document_index (group_id, media_id, day, sort_date, name, group_name, abstract, size)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(group_id, media_id) DO UPDATE SET
               day = excluded.day, sort_date = excluded.sort_date, name = excluded.name,
               group_name = excluded.group_name, abstract = excluded.abstract, size = excluded.size",
        )
        .bind(group_id)
        .bind(&file.media_id)
        .bind(&file.day)
        .bind(&file.sort_date)
        .bind(&file.name)
        .bind(group_name)
        .bind(&file.text)
        .bind(file.size)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_ima_pdf(
        &self,
        group_id: &str,
        media_id: &str,
        pdf_path: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE ima_document_index SET pdf_path = ?, downloaded_at = datetime('now') WHERE group_id = ? AND media_id = ?")
            .bind(pdf_path)
            .bind(group_id)
            .bind(media_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn insert_ima_document(
        &self,
        group_id: &str,
        media_id: &str,
        name: &str,
        day: &str,
        pdf_path: &str,
        txt_path: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO ima_document_index (group_id, media_id, day, sort_date, name, group_name, pdf_path, txt_path, downloaded_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'))",
        )
        .bind(group_id)
        .bind(media_id)
        .bind(day)
        .bind(day)
        .bind(name)
        .bind(group_id)
        .bind(pdf_path)
        .bind(txt_path)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn ima_document_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM ima_document_index")
            .fetch_one(&self.pool)
            .await
    }

    pub async fn ima_downloads_between(
        &self,
        group_id: &str,
        started: i64,
        finished: i64,
    ) -> Result<i64, sqlx::Error> {
        if group_id.is_empty() || finished < started {
            return Ok(0);
        }
        let start = crate::arm::iso_utc(started.saturating_sub(2).max(0));
        let end = crate::arm::iso_utc(finished.saturating_add(2));
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ima_document_index WHERE group_id = ? AND downloaded_at >= ? AND downloaded_at <= ?",
        )
        .bind(group_id)
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    pub async fn ima_latest_batch(&self, group_id: &str) -> Result<(String, i64), sqlx::Error> {
        let stamp: Option<String> = sqlx::query_scalar(
            "SELECT downloaded_at FROM ima_document_index WHERE group_id = ? AND downloaded_at != '' ORDER BY downloaded_at DESC LIMIT 1",
        )
        .bind(group_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(stamp) = stamp else {
            return Ok((String::new(), 0));
        };
        if stamp.len() < 19 {
            return Ok((stamp, 1));
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ima_document_index WHERE group_id = ? AND downloaded_at LIKE ?",
        )
        .bind(group_id)
        .bind(format!("{}%", &stamp[..19]))
        .fetch_one(&self.pool)
        .await?;
        Ok((stamp, n))
    }

    pub async fn ima_group_summaries(&self) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT group_id, COUNT(*) AS document_count, MAX(sort_date) AS latest_day, \
                    MAX(media_id) AS latest_media_id, MAX(group_name) AS group_name \
             FROM ima_document_index GROUP BY group_id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "group_id": row.get::<String, _>("group_id"),
                    "document_count": row.get::<i64, _>("document_count"),
                    "latest_day": row.get::<Option<String>, _>("latest_day").unwrap_or_default(),
                    "latest_media_id": row.get::<Option<String>, _>("latest_media_id").unwrap_or_default(),
                    "group_name": row.get::<Option<String>, _>("group_name").unwrap_or_default(),
                })
            })
            .collect())
    }
    pub async fn ima_index(&self) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT group_id, media_id, day, sort_date, name, group_name, abstract, size, chars, pdf_path, txt_path, downloaded_at FROM ima_document_index",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(ima_index_row).collect())
    }

    pub async fn ima_documents_for_media(
        &self,
        media_id: &str,
        group: &str,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT group_id, media_id, day, sort_date, name, group_name, abstract, size, chars, pdf_path, txt_path, downloaded_at
             FROM ima_document_index WHERE media_id = ? AND (? = '' OR group_id = ?)",
        )
        .bind(media_id)
        .bind(group)
        .bind(group)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(ima_index_row).collect())
    }

    pub async fn ima_ticker_page(
        &self,
        raw_code: &str,
        readable: &[String],
        limit: i64,
    ) -> Result<Value, sqlx::Error> {
        let code = self.ima_ticker_code(raw_code).await?;
        if code.is_empty() || readable.is_empty() {
            return Ok(json!({"code": code, "name": "", "digest": {}, "items": [], "count": 0}));
        }
        let limit = limit.clamp(1, 200);
        let placeholders = std::iter::repeat_n("?", readable.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT t.group_id AS group_id, t.media_id AS media_id, t.name AS ticker_name, d.name AS name, d.sort_date AS sort_date, d.day AS day, \
             re.rating AS rating, re.target_price AS target_price, re.thesis AS thesis \
             FROM report_extraction_tickers t \
             JOIN ima_document_index d ON d.group_id = t.group_id AND d.media_id = t.media_id \
             LEFT JOIN report_extractions re ON re.group_id = t.group_id AND re.media_id = t.media_id \
             WHERE t.code = ? AND t.group_id IN ({placeholders}) \
             ORDER BY (d.sort_date = '') ASC, d.sort_date DESC, d.media_id DESC LIMIT ?"
        );
        let mut query = sqlx::query(&sql).bind(&code);
        for group in readable {
            query = query.bind(group);
        }
        let rows = query.bind(limit).fetch_all(&self.pool).await?;
        let items: Vec<Value> = rows.iter().map(|row| {
            json!({
                "group_id": row.get::<String, _>("group_id"),
                "media_id": row.get::<String, _>("media_id"),
                "name": row.get::<String, _>("name"),
                "sort_date": row.get::<String, _>("sort_date"),
                "day": row.get::<String, _>("day"),
                "extraction": {
                    "rating": row.get::<Option<String>, _>("rating").unwrap_or_default(),
                    "target_price": row.get::<Option<String>, _>("target_price").unwrap_or_default(),
                    "thesis": row.get::<Option<String>, _>("thesis").unwrap_or_default(),
                }
            })
        }).collect();
        let ticker_name = rows
            .first()
            .map(|row| row.get::<String, _>("ticker_name"))
            .unwrap_or_default();
        let digest = self.ima_ticker_digest(&code, readable).await?;
        let name = digest["name"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or(&ticker_name)
            .to_string();
        let count = items.len();
        Ok(json!({"code": code, "name": name, "digest": digest, "items": items, "count": count}))
    }

    async fn ima_ticker_code(&self, raw: &str) -> Result<String, sqlx::Error> {
        let raw = raw.trim().chars().take(64).collect::<String>();
        if raw.is_empty() || raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(raw);
        }
        let found = sqlx::query_scalar::<_, String>(
            "SELECT code FROM report_extraction_tickers WHERE name = ? GROUP BY code ORDER BY COUNT(*) DESC LIMIT 1",
        )
        .bind(&raw)
        .fetch_optional(&self.pool)
        .await?;
        Ok(found.unwrap_or_else(|| raw.to_ascii_uppercase()))
    }

    async fn ima_ticker_digest(
        &self,
        code: &str,
        readable: &[String],
    ) -> Result<Value, sqlx::Error> {
        let row = sqlx::query("SELECT name, source_count, digest, updated_at, status FROM ima_ticker_digests WHERE kind = 'ticker' AND code = ?")
            .bind(code)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(json!({})) };
        let sources = sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT group_id FROM report_extraction_tickers WHERE code = ?",
        )
        .bind(code)
        .fetch_all(&self.pool)
        .await?;
        let configured =
            sqlx::query_scalar::<_, String>("SELECT DISTINCT group_id FROM ima_document_index")
                .fetch_all(&self.pool)
                .await?;
        let visible = sources
            .iter()
            .filter(|group| configured.iter().any(|item| item == *group))
            .collect::<Vec<_>>();
        if visible.is_empty()
            || visible
                .iter()
                .any(|group| !readable.iter().any(|item| item == *group))
        {
            return Ok(json!({}));
        }
        let raw = row.get::<String, _>("digest");
        let mut view = serde_json::from_str::<Value>(&raw)
            .unwrap_or_else(|_| json!({"consensus": raw.chars().take(1200).collect::<String>()}));
        if !view.is_object() {
            view = json!({"consensus": raw.chars().take(1200).collect::<String>()});
        }
        view["name"] = json!(row.get::<String, _>("name"));
        view["source_count"] = json!(row.get::<i64, _>("source_count"));
        view["updated_at"] = json!(row.get::<String, _>("updated_at"));
        view["status"] = json!(row.get::<String, _>("status"));
        Ok(view)
    }

    pub async fn ima_abstract_translation(
        &self,
        group_id: &str,
        media_id: &str,
    ) -> Result<(String, String), sqlx::Error> {
        let row = sqlx::query("SELECT src_hash, abstract_zh FROM ima_abstract_translations WHERE group_id = ? AND media_id = ?")
            .bind(group_id)
            .bind(media_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row
            .map(|row| (row.get("src_hash"), row.get("abstract_zh")))
            .unwrap_or_default())
    }

    pub async fn save_ima_abstract_translation(
        &self,
        group_id: &str,
        media_id: &str,
        src_hash: &str,
        text: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO ima_abstract_translations (group_id, media_id, src_hash, abstract_zh) VALUES (?, ?, ?, ?) \
             ON CONFLICT(group_id, media_id) DO UPDATE SET src_hash = excluded.src_hash, abstract_zh = excluded.abstract_zh",
        )
        .bind(group_id)
        .bind(media_id)
        .bind(src_hash)
        .bind(text)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn ima_access(
        &self,
        user_id: i64,
    ) -> Result<(HashSet<String>, HashSet<String>), sqlx::Error> {
        let acl =
            sqlx::query_scalar::<_, String>("SELECT group_id FROM ima_kb_acl WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?;
        let subscribed = sqlx::query_scalar::<_, String>(
            "SELECT group_id FROM ima_kb_subscriptions WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok((acl.into_iter().collect(), subscribed.into_iter().collect()))
    }

    pub async fn ima_kb_subscribe(&self, user_id: i64, group_id: &str) -> Result<(), sqlx::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        sqlx::query("INSERT OR IGNORE INTO ima_kb_subscriptions (user_id, group_id, created_at) VALUES (?, ?, ?)")
            .bind(user_id)
            .bind(group_id)
            .bind(now)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn ima_kb_unsubscribe(
        &self,
        user_id: i64,
        group_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM ima_kb_subscriptions WHERE user_id = ? AND group_id = ?")
            .bind(user_id)
            .bind(group_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn ima_kb_acl_usernames(&self, group_id: &str) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT u.username FROM ima_kb_acl a JOIN users u ON u.id = a.user_id WHERE a.group_id = ? ORDER BY u.username",
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn set_ima_kb_acl(
        &self,
        group_id: &str,
        user_ids: &[i64],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM ima_kb_acl WHERE group_id = ?")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for uid in user_ids {
            sqlx::query("INSERT OR IGNORE INTO ima_kb_acl (group_id, user_id) VALUES (?, ?)")
                .bind(group_id)
                .bind(uid)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO ima_kb_subscriptions (user_id, group_id, created_at) VALUES (?, ?, ?)")
                .bind(uid)
                .bind(group_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        let existing = sqlx::query_scalar::<_, i64>(
            "SELECT user_id FROM ima_kb_subscriptions WHERE group_id = ?",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await?;
        for uid in existing {
            if !user_ids.contains(&uid) {
                sqlx::query("DELETE FROM ima_kb_subscriptions WHERE group_id = ? AND user_id = ?")
                    .bind(group_id)
                    .bind(uid)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn ima_kb_group_ids_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT group_id FROM ima_kb_acl WHERE user_id = ? ORDER BY group_id")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn ima_kb_subscribed_group_ids_for_user(
        &self,
        user_id: i64,
    ) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT group_id FROM ima_kb_subscriptions WHERE user_id = ? ORDER BY group_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn ima_kb_acl_map(&self) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
        let rows = sqlx::query("SELECT user_id, group_id FROM ima_kb_acl ORDER BY group_id")
            .fetch_all(&self.pool)
            .await?;
        let mut mapped = HashMap::new();
        for row in rows {
            mapped
                .entry(row.get::<i64, _>("user_id"))
                .or_insert_with(Vec::new)
                .push(row.get("group_id"));
        }
        Ok(mapped)
    }

    async fn ima_kb_sub_map(&self) -> Result<HashMap<i64, Vec<String>>, sqlx::Error> {
        let rows =
            sqlx::query("SELECT user_id, group_id FROM ima_kb_subscriptions ORDER BY group_id")
                .fetch_all(&self.pool)
                .await?;
        let mut mapped = HashMap::new();
        for row in rows {
            mapped
                .entry(row.get::<i64, _>("user_id"))
                .or_insert_with(Vec::new)
                .push(row.get("group_id"));
        }
        Ok(mapped)
    }

    pub async fn set_ima_kb_acl_for_user(
        &self,
        user_id: i64,
        group_ids: &[String],
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let existing =
            sqlx::query_scalar::<_, String>("SELECT group_id FROM ima_kb_acl WHERE user_id = ?")
                .bind(user_id)
                .fetch_all(&mut *tx)
                .await?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for group_id in group_ids {
            if existing.iter().any(|item| item == group_id) {
                continue;
            }
            sqlx::query("INSERT OR IGNORE INTO ima_kb_acl (group_id, user_id) VALUES (?, ?)")
                .bind(group_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT OR IGNORE INTO ima_kb_subscriptions (user_id, group_id, created_at) VALUES (?, ?, ?)")
                .bind(user_id)
                .bind(group_id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        for group_id in &existing {
            if group_ids.iter().any(|item| item == group_id) {
                continue;
            }
            sqlx::query("DELETE FROM ima_kb_acl WHERE group_id = ? AND user_id = ?")
                .bind(group_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM ima_kb_subscriptions WHERE group_id = ? AND user_id = ?")
                .bind(group_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn setting(&self, key: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn hosted_image_counts(&self) -> Result<(i64, i64, i64), sqlx::Error> {
        let rows = sqlx::query("SELECT status, COUNT(*) AS n FROM hosted_images GROUP BY status")
            .fetch_all(&self.pool)
            .await?;
        let mut ready = 0;
        let mut pending = 0;
        let mut failed = 0;
        for row in rows {
            let n: i64 = row.get("n");
            match row.get::<String, _>("status").as_str() {
                "ready" => ready = n,
                "pending" => pending = n,
                "failed" => failed = n,
                _ => {}
            }
        }
        Ok((ready, pending, failed))
    }

    pub async fn note_hosted_image(
        &self,
        source_url: &str,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO hosted_images (source_url, status) VALUES (?, ?)")
            .bind(source_url)
            .bind(status)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_cookie(&self, key: &str, value: &str) -> Result<(), sqlx::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.set_setting(key, value).await?;
        self.set_setting(&format!("{key}_updated_at"), &now.to_string())
            .await
    }

    pub async fn clear_cookie(&self, kind: &str) -> Result<(), CatalogError> {
        let key = match kind {
            "xueqiu" => "xueqiu_cookie",
            "weibo" => "weibo_cookie",
            "twitter" => "twitter_cookie",
            "ima" => "ima_cookie",
            "zsxq" => "zsxq_cookie",
            _ => return Err(CatalogError::Bad("未知 Cookie 源")),
        };
        self.set_setting(key, "").await?;
        self.set_setting(&format!("{key}_updated_at"), "").await?;
        Ok(())
    }

    pub async fn polling_config(&self) -> Result<Value, sqlx::Error> {
        let settings = self.settings().await?;
        Ok(polling_json(&settings))
    }

    pub async fn update_polling(
        &self,
        body: &serde_json::Map<String, Value>,
    ) -> Result<Value, CatalogError> {
        for (name, key, _default, lo, hi) in POLL_FIELDS {
            let Some(value) = body.get(*name) else {
                continue;
            };
            let Some(n) = value.as_i64() else {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            };
            if n < *lo || n > *hi {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            }
            self.set_setting(key, &n.to_string()).await?;
        }
        for (name, key, lo, hi) in ZSXQ_INTS {
            let Some(value) = body.get(*name) else {
                continue;
            };
            let Some(n) = value.as_i64() else {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            };
            if n < *lo || n > *hi {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            }
            self.set_setting(key, &n.to_string()).await?;
        }
        for (name, key, lo, hi) in ZSXQ_FLOATS {
            let Some(value) = body.get(*name) else {
                continue;
            };
            let Some(n) = value.as_f64() else {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            };
            if n < *lo || n > *hi {
                return Err(CatalogError::Invalid(format!("{name} 需在 {lo}-{hi} 之间")));
            }
            self.set_setting(key, &n.to_string()).await?;
        }
        for (name, key) in [
            (
                "translate_twitter_content",
                "config_translate_twitter_content",
            ),
            ("telegram_rich_messages", "config_telegram_rich_messages"),
            ("zsxq_prefetch_files", "zsxq_prefetch_files"),
            ("zsxq_fetch_comments", "zsxq_fetch_comments"),
            ("zsxq_app_channel", "zsxq_app_channel"),
        ] {
            let Some(value) = body.get(name) else {
                continue;
            };
            let Some(on) = value.as_bool() else {
                return Err(CatalogError::Invalid(format!("{name} 需为布尔值")));
            };
            self.set_setting(key, if on { "1" } else { "0" }).await?;
        }
        if let Some(value) = body.get("zsxq_app_device") {
            let dev = value.as_str().unwrap_or("").trim();
            if dev.is_empty() || dev.chars().count() > 64 {
                return Err(CatalogError::Bad("zsxq_app_device 需 1-64 字符"));
            }
            self.set_setting("zsxq_app_device", dev).await?;
        }
        self.polling_config().await.map_err(CatalogError::Db)
    }

    pub async fn admin_stats(&self) -> Result<Value, sqlx::Error> {
        let settings = self.settings().await?;
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        let posts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts")
            .fetch_one(&self.pool)
            .await?;
        let kols: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kols")
            .fetch_one(&self.pool)
            .await?;
        let enabled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kols WHERE enabled = 1")
            .fetch_one(&self.pool)
            .await?;
        let priority: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kols WHERE priority = 1")
            .fetch_one(&self.pool)
            .await?;
        let secondary: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kols WHERE secondary = 1")
            .fetch_one(&self.pool)
            .await?;
        let active: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT kol_id) FROM subscriptions")
            .fetch_one(&self.pool)
            .await?;
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM kol_requests WHERE status = 'pending'")
                .fetch_one(&self.pool)
                .await?;
        let counts = sqlx::query("SELECT platform, SUM(enabled) AS n FROM kols GROUP BY platform")
            .fetch_all(&self.pool)
            .await?;
        let mut by_platform = HashMap::new();
        for row in counts {
            by_platform.insert(row.get::<String, _>("platform"), row.get::<i64, _>("n"));
        }
        let plaza = plaza_rows(&by_platform, &settings);
        let zsxq_cache = crate::zsxq_file::cache_stats(self).await;
        Ok(json!({
            "users": users,
            "posts": posts,
            "kols": kols,
            "enabled_kols": enabled,
            "active_kols": active,
            "priority_kols": priority,
            "secondary_kols": secondary,
            "pending_kol_requests": pending,
            "keepalive_interval_seconds": int_setting(&settings, "config_cookie_keepalive_interval_seconds", 0),
            "polling_interval_seconds": int_setting(&settings, "config_interval_seconds", 180),
            "last_poll_at": Value::Null,
            "last_poll_error": "",
            "retry_pending": 0,
            "sources": [],
            "kol_health": [],
            "recent_source_events": [],
            "alerts": {},
            "x_channels": {},
            "imgbed": crate::imgbed::status(self).await?,
            "plaza_sources": plaza,
            "polling_config": polling_json(&settings),
            "xueqiu_cookie": cookie_status(&settings, "xueqiu_cookie", None),
            "weibo_cookie": cookie_status(&settings, "weibo_cookie", env_nonempty("WEIBO_COOKIE")),
            "twitter_cookie": cookie_status(&settings, "twitter_cookie", env_nonempty("TWITTER_COOKIE")),
            "zsxq_cookie": cookie_status(&settings, "zsxq_cookie", env_nonempty("ZSXQ_COOKIE").or_else(|| env_nonempty("ZSXQ_ACCESS_TOKEN"))),
            "zsxq_cache": zsxq_cache,
            "ima_credentials": crate::ima_admin::credentials(self).await?,
            "ima_collector": {"config": {"groups": []}},
        }))
    }

    async fn settings(&self) -> Result<HashMap<String, String>, sqlx::Error> {
        let rows = sqlx::query("SELECT key, value FROM settings")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get("key"), row.get("value")))
            .collect())
    }

    pub async fn plaza_sources(&self) -> Result<Vec<Value>, sqlx::Error> {
        Ok(plaza_rows(
            &self.enabled_counts().await?,
            &self.settings().await?,
        ))
    }

    pub async fn set_plaza_visibility(
        &self,
        updates: &serde_json::Map<String, Value>,
    ) -> Result<Vec<Value>, CatalogError> {
        if updates.is_empty() {
            return Err(CatalogError::Bad("没有要更新的平台"));
        }
        let mut modes = plaza_modes(self.setting("plaza_source_visibility").await?.as_deref());
        for (platform, mode) in updates {
            if !PLAZA_PLATFORMS.contains(&platform.as_str()) {
                return Err(CatalogError::Invalid(format!("不支持的平台: {platform}")));
            }
            let Some(mode) = mode.as_str() else {
                return Err(CatalogError::Bad("显示方式须为自动、显示或隐藏"));
            };
            if !matches!(mode, "auto" | "show" | "hide") {
                return Err(CatalogError::Bad("显示方式须为自动、显示或隐藏"));
            }
            modes.insert(platform.clone(), mode.to_string());
        }
        let saved = serde_json::to_string(&modes).unwrap_or_else(|_| "{}".into());
        self.set_setting("plaza_source_visibility", &saved).await?;
        self.plaza_sources().await.map_err(CatalogError::Db)
    }

    pub async fn subscribed_platforms(&self, user_id: i64) -> Result<Vec<String>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT DISTINCT k.platform FROM subscriptions s
             JOIN kols k ON k.id = s.kol_id WHERE s.user_id = ?",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get("platform")).collect())
    }

    pub async fn news_flag(&self, key: &str, default: bool) -> Result<bool, sqlx::Error> {
        Ok(self
            .setting(key)
            .await?
            .map(|value| value == "1")
            .unwrap_or(default))
    }

    pub async fn set_news_flag(&self, key: &str, on: bool) -> Result<(), sqlx::Error> {
        self.set_setting(key, if on { "1" } else { "0" }).await
    }

    pub async fn add_news_source(&self, name: &str, group_name: &str) -> Result<i64, CatalogError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(CatalogError::Bad("媒体名称需 1-60 字"));
        }
        let id = sqlx::query(
            "INSERT INTO news_sources (slug, name, group_name) VALUES ('pending', ?, ?)",
        )
        .bind(name)
        .bind(group_name.trim())
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        sqlx::query("UPDATE news_sources SET slug = ? WHERE id = ?")
            .bind(format!("s{id}"))
            .bind(id)
            .execute(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO user_news_sources (user_id, source_id)
             SELECT DISTINCT user_id, ? FROM user_news_sources",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn set_user_news_sources(
        &self,
        user_id: i64,
        ids: &[i64],
    ) -> Result<(), CatalogError> {
        let mut unique = Vec::new();
        for id in ids {
            if !unique.contains(id) {
                unique.push(*id);
            }
        }
        if !unique.is_empty() {
            let mut found = Vec::new();
            for id in &unique {
                let ok: Option<i64> = sqlx::query_scalar(
                    "SELECT id FROM news_sources WHERE id = ? AND archived_at IS NULL",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
                if ok.is_none() {
                    return Err(CatalogError::Bad("来源不存在或已归档"));
                }
                found.push(*id);
            }
            let _ = found;
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM user_news_sources WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        for id in unique {
            sqlx::query("INSERT INTO user_news_sources (user_id, source_id) VALUES (?, ?)")
                .bind(user_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn add_news_feed(
        &self,
        source_id: i64,
        name: &str,
        url: &str,
    ) -> Result<i64, CatalogError> {
        let found: Option<i64> =
            sqlx::query_scalar("SELECT id FROM news_sources WHERE id = ? AND archived_at IS NULL")
                .bind(source_id)
                .fetch_optional(&self.pool)
                .await?;
        if found.is_none() {
            return Err(CatalogError::Missing("媒体不存在"));
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(CatalogError::Bad("Feed 名称需 1-80 字"));
        }
        let url = url.trim();
        if url.is_empty() || url.len() > 2048 {
            return Err(CatalogError::Bad("Feed URL 无效"));
        }
        let duplicate: Option<i64> = sqlx::query_scalar("SELECT id FROM news_feeds WHERE url = ?")
            .bind(url)
            .fetch_optional(&self.pool)
            .await?;
        if duplicate.is_some() {
            return Err(CatalogError::Bad("Feed URL 已存在"));
        }
        Ok(
            sqlx::query("INSERT INTO news_feeds (source_id, name, url) VALUES (?, ?, ?)")
                .bind(source_id)
                .bind(name)
                .bind(url)
                .execute(&self.pool)
                .await?
                .last_insert_rowid(),
        )
    }

    pub async fn news_feeds(&self) -> Result<Vec<(i64, i64, String, String)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT f.id, f.source_id, f.name, f.url FROM news_feeds f
             JOIN news_sources s ON s.id = f.source_id
             WHERE f.enabled = 1 AND f.archived_at IS NULL AND f.url != ''
               AND s.enabled = 1 AND s.archived_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.get("id"),
                    row.get("source_id"),
                    row.get("name"),
                    row.get("url"),
                )
            })
            .collect())
    }

    pub async fn save_news_entries(
        &self,
        source_id: i64,
        feed_id: i64,
        entries: &[NewsEntry],
    ) -> Result<usize, sqlx::Error> {
        let mut added = 0;
        for entry in entries {
            let result = sqlx::query(
                "INSERT INTO news_articles
                    (source_id, feed_id, external_id, title, summary, content, url, author, published_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(source_id, external_id) DO NOTHING",
            )
            .bind(source_id)
            .bind(feed_id)
            .bind(&entry.external_id)
            .bind(&entry.title)
            .bind(&entry.summary)
            .bind(&entry.content)
            .bind(&entry.url)
            .bind(&entry.author)
            .bind(&entry.published_at)
            .execute(&self.pool)
            .await?;
            added += result.rows_affected() as usize;
        }
        sqlx::query("UPDATE news_sources SET last_success_at = datetime('now') WHERE id = ?")
            .bind(source_id)
            .execute(&self.pool)
            .await?;
        Ok(added)
    }

    pub async fn save_xincai(
        &self,
        group: &str,
        items: &[XincaiRow],
    ) -> Result<serde_json::Map<String, Value>, CatalogError> {
        let mut sources = serde_json::Map::new();
        let mut saved = 0;
        let mut seen: Vec<(String, i64, i64)> = Vec::new();
        for item in items {
            let (source_id, feed_id) = if let Some((_, source_id, feed_id)) =
                seen.iter().find(|(key, _, _)| key == &item.key)
            {
                (*source_id, *feed_id)
            } else {
                let source_id = self
                    .xincai_source(&item.slug, &item.name, group, &item.kind, &item.platform)
                    .await?;
                let feed_id = self.xincai_feed(source_id).await?;
                seen.push((item.key.clone(), source_id, feed_id));
                (source_id, feed_id)
            };
            sqlx::query(
                "INSERT INTO news_articles
                    (source_id, feed_id, external_id, title, summary, content, url, author, published_at,
                     issue_key, issue_label, issue_title, issue_cover, section, toc_order, images, topics,
                     fetched_at, content_hash)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(source_id, external_id) DO UPDATE SET
                    feed_id = excluded.feed_id, title = excluded.title, summary = excluded.summary,
                    content = excluded.content, url = excluded.url, author = excluded.author,
                    published_at = excluded.published_at, issue_key = excluded.issue_key,
                    issue_label = excluded.issue_label, issue_title = excluded.issue_title,
                    issue_cover = excluded.issue_cover, section = excluded.section,
                    toc_order = excluded.toc_order, images = excluded.images,
                    topics = CASE WHEN excluded.topics = '[]' THEN news_articles.topics ELSE excluded.topics END,
                    fetched_at = excluded.fetched_at,
                    content_hash = CASE WHEN excluded.content_hash = '' THEN news_articles.content_hash ELSE excluded.content_hash END",
            )
            .bind(source_id)
            .bind(feed_id)
            .bind(&item.external_id)
            .bind(&item.title)
            .bind(&item.summary)
            .bind(&item.content)
            .bind(&item.url)
            .bind(&item.author)
            .bind(&item.published_at)
            .bind(&item.issue_key)
            .bind(&item.issue_label)
            .bind(&item.issue_title)
            .bind(&item.issue_cover)
            .bind(&item.section)
            .bind(item.toc_order)
            .bind(&item.images)
            .bind(&item.topics)
            .bind(&item.fetched_at)
            .bind(&item.content_hash)
            .execute(&self.pool)
            .await?;
            saved += 1;
            sources.insert(item.name.clone(), json!(source_id));
        }
        for (_, source_id, _) in &seen {
            sqlx::query("UPDATE news_sources SET last_success_at = datetime('now') WHERE id = ?")
                .bind(source_id)
                .execute(&self.pool)
                .await?;
        }
        let _ = saved;
        Ok(sources)
    }

    async fn xincai_source(
        &self,
        slug: &str,
        name: &str,
        group: &str,
        kind: &str,
        platform: &str,
    ) -> Result<i64, CatalogError> {
        let kind = if kind == "magazine" {
            "magazine"
        } else {
            "feed"
        };
        let group = group.trim();
        if let Some(row) =
            sqlx::query("SELECT id, kind, group_name FROM news_sources WHERE slug = ?")
                .bind(slug)
                .fetch_optional(&self.pool)
                .await?
        {
            self.touch_xincai_source(
                row.get("id"),
                row.get("kind"),
                row.get("group_name"),
                kind,
                group,
                platform,
            )
            .await?;
            return Ok(row.get("id"));
        }
        if let Some(row) = sqlx::query(
            "SELECT id, slug, kind, group_name FROM news_sources WHERE name = ? COLLATE NOCASE",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        {
            let existing: String = row.get("slug");
            if !existing.starts_with("xincai-") {
                return Err(CatalogError::Invalid(format!(
                    "媒体名称已被公开源占用：{name}"
                )));
            }
            let id: i64 = row.get("id");
            if existing != slug {
                sqlx::query("UPDATE news_sources SET slug = ? WHERE id = ?")
                    .bind(slug)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
            }
            self.touch_xincai_source(
                id,
                row.get("kind"),
                row.get("group_name"),
                kind,
                group,
                platform,
            )
            .await?;
            return Ok(id);
        }
        let id = sqlx::query("INSERT INTO news_sources (slug, name, group_name, kind, internal, platform) VALUES (?, ?, ?, ?, 1, ?)")
            .bind(slug)
            .bind(name)
            .bind(group)
            .bind(kind)
            .bind(platform)
            .execute(&self.pool)
            .await?
            .last_insert_rowid();
        sqlx::query(
            "INSERT OR IGNORE INTO user_news_sources (user_id, source_id) SELECT id, ? FROM users",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    async fn touch_xincai_source(
        &self,
        id: i64,
        current_kind: String,
        current_group: String,
        kind: &str,
        group: &str,
        platform: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE news_sources SET internal = 1 WHERE id = ? AND internal = 0")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if !platform.is_empty() {
            sqlx::query(
                "UPDATE news_sources SET platform = ? WHERE id = ? AND COALESCE(platform, '') = ''",
            )
            .bind(platform)
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        if kind == "magazine" && current_kind != "magazine" {
            sqlx::query("UPDATE news_sources SET kind = 'magazine' WHERE id = ?")
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if !group.is_empty() && (current_group.is_empty() || current_group == "心裁") {
            sqlx::query("UPDATE news_sources SET group_name = ? WHERE id = ?")
                .bind(group)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn xincai_feed(&self, source_id: i64) -> Result<i64, sqlx::Error> {
        if let Some(row) =
            sqlx::query("SELECT id FROM news_feeds WHERE source_id = ? AND name = '推送'")
                .bind(source_id)
                .fetch_optional(&self.pool)
                .await?
        {
            return Ok(row.get("id"));
        }
        Ok(sqlx::query(
            "INSERT INTO news_feeds (source_id, name, url, normalized_url, enabled) VALUES (?, '推送', '', '', 0)",
        )
        .bind(source_id)
        .execute(&self.pool)
        .await?
        .last_insert_rowid())
    }

    pub async fn admin_news_sources(&self) -> Result<Vec<Value>, sqlx::Error> {
        self.admin_news_sources_all(false).await
    }

    pub async fn admin_news_sources_all(
        &self,
        include_archived: bool,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let sources = sqlx::query(
            "SELECT id, slug, name, group_name, enabled, kind, internal, archived_at, last_success_at,
                    (SELECT COUNT(*) FROM news_articles a WHERE a.source_id = news_sources.id) AS article_count
             FROM news_sources
             WHERE (? = 1 OR archived_at IS NULL)
             ORDER BY id",
        )
        .bind(i64::from(include_archived))
        .fetch_all(&self.pool)
        .await?;
        let feeds = sqlx::query(
            "SELECT id, source_id, name, url, enabled, archived_at, consecutive_failures, last_success_at, last_error_detail
             FROM news_feeds
             WHERE (? = 1 OR archived_at IS NULL)
             ORDER BY id",
        )
        .bind(i64::from(include_archived))
        .fetch_all(&self.pool)
        .await?;
        Ok(sources
            .into_iter()
            .map(|row| news_source_json(&row, &feeds))
            .collect())
    }

    pub async fn admin_news_source(&self, id: i64) -> Result<Option<Value>, sqlx::Error> {
        Ok(self
            .admin_news_sources_all(true)
            .await?
            .into_iter()
            .find(|row| row["id"].as_i64() == Some(id)))
    }

    pub async fn update_news_source(
        &self,
        id: i64,
        name: Option<&str>,
        group_name: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Value, CatalogError> {
        if self.admin_news_source(id).await?.is_none() {
            return Err(CatalogError::Missing("媒体不存在"));
        }
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 60 {
                return Err(CatalogError::Bad("媒体名称需 1-60 字"));
            }
            let duplicate: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM news_sources WHERE name = ? COLLATE NOCASE AND id != ?",
            )
            .bind(name)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
            if duplicate.is_some() {
                return Err(CatalogError::Bad("媒体名称已存在"));
            }
            sqlx::query("UPDATE news_sources SET name = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(group) = group_name {
            let group: String = group.trim().chars().take(40).collect();
            sqlx::query("UPDATE news_sources SET group_name = ? WHERE id = ?")
                .bind(group)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(on) = enabled {
            sqlx::query("UPDATE news_sources SET enabled = ? WHERE id = ?")
                .bind(i64::from(on))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        self.admin_news_source(id)
            .await?
            .ok_or(CatalogError::Missing("媒体不存在"))
    }

    pub async fn set_news_source_archived(
        &self,
        id: i64,
        archived: bool,
    ) -> Result<(), CatalogError> {
        let changed = sqlx::query("UPDATE news_sources SET archived_at = CASE WHEN ? = 1 THEN datetime('now') ELSE NULL END WHERE id = ?")
            .bind(i64::from(archived)).bind(id).execute(&self.pool).await?.rows_affected();
        if changed == 0 {
            Err(CatalogError::Missing("媒体不存在"))
        } else {
            Ok(())
        }
    }

    pub async fn delete_news_source(&self, id: i64) -> Result<bool, sqlx::Error> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM news_sources WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_none() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM news_reads WHERE article_id IN (SELECT id FROM news_articles WHERE source_id = ?)").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM news_articles WHERE source_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM user_news_sources WHERE source_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM news_feeds WHERE source_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM news_sources WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn news_feed_target(
        &self,
        id: i64,
    ) -> Result<Option<(i64, String, bool, bool)>, sqlx::Error> {
        let row =
            sqlx::query("SELECT source_id, url, enabled, archived_at FROM news_feeds WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|row| {
            let archived: Option<String> = row.get("archived_at");
            (
                row.get("source_id"),
                row.get("url"),
                row.get::<i64, _>("enabled") != 0,
                archived.is_some(),
            )
        }))
    }

    pub async fn enabled_feed_ids(&self, source_id: Option<i64>) -> Result<Vec<i64>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT f.id FROM news_feeds f
             JOIN news_sources s ON s.id = f.source_id
             WHERE f.enabled = 1 AND f.archived_at IS NULL AND f.url != ''
               AND s.enabled = 1 AND s.archived_at IS NULL
               AND (? IS NULL OR f.source_id = ?)
             ORDER BY f.id",
        )
        .bind(source_id)
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get("id")).collect())
    }

    pub async fn update_news_feed(
        &self,
        id: i64,
        name: Option<&str>,
        url: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<Value, CatalogError> {
        let current = sqlx::query("SELECT source_id, url FROM news_feeds WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(current) = current else {
            return Err(CatalogError::Missing("Feed 不存在"));
        };
        let source_id: i64 = current.get("source_id");
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 80 {
                return Err(CatalogError::Bad("Feed 名称需 1-80 字"));
            }
            let duplicate: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM news_feeds WHERE source_id = ? AND name = ? AND id != ?",
            )
            .bind(source_id)
            .bind(name)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
            if duplicate.is_some() {
                return Err(CatalogError::Bad("该媒体下的 Feed 名称已存在"));
            }
            sqlx::query("UPDATE news_feeds SET name = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(url) = url {
            let url = url.trim();
            if url.is_empty() || url.len() > 2048 {
                return Err(CatalogError::Bad("Feed URL 无效"));
            }
            let duplicate: Option<i64> =
                sqlx::query_scalar("SELECT id FROM news_feeds WHERE url = ? AND id != ?")
                    .bind(url)
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?;
            if duplicate.is_some() {
                return Err(CatalogError::Bad("Feed URL 已存在"));
            }
            let current_url: String = current.get("url");
            if current_url != url {
                sqlx::query("UPDATE news_feeds SET url = ?, consecutive_failures = 0, last_error_detail = '', last_success_at = '' WHERE id = ?")
                    .bind(url).bind(id).execute(&self.pool).await?;
            }
        }
        if let Some(on) = enabled {
            sqlx::query("UPDATE news_feeds SET enabled = ? WHERE id = ?")
                .bind(i64::from(on))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        self.feed_value(id)
            .await?
            .ok_or(CatalogError::Missing("Feed 不存在"))
    }

    pub async fn set_news_feed_archived(
        &self,
        id: i64,
        archived: bool,
    ) -> Result<(), CatalogError> {
        let changed = sqlx::query("UPDATE news_feeds SET archived_at = CASE WHEN ? = 1 THEN datetime('now') ELSE NULL END WHERE id = ?")
            .bind(i64::from(archived)).bind(id).execute(&self.pool).await?.rows_affected();
        if changed == 0 {
            Err(CatalogError::Missing("Feed 不存在"))
        } else {
            Ok(())
        }
    }

    pub async fn delete_news_feed(&self, id: i64) -> Result<bool, sqlx::Error> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM news_feeds WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_none() {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM news_reads WHERE article_id IN (SELECT id FROM news_articles WHERE feed_id = ?)").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM news_articles WHERE feed_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM news_feeds WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn delete_news_article(&self, id: i64) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM news_reads WHERE article_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query("DELETE FROM news_articles WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(deleted > 0)
    }

    pub async fn note_feed_success(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE news_feeds SET consecutive_failures = 0, last_error_detail = '', last_success_at = datetime('now') WHERE id = ?")
            .bind(id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn note_feed_failure(&self, id: i64, detail: &str) -> Result<(), sqlx::Error> {
        let detail: String = detail.chars().take(300).collect();
        sqlx::query("UPDATE news_feeds SET consecutive_failures = consecutive_failures + 1, last_error_detail = ? WHERE id = ?")
            .bind(detail).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    async fn feed_value(&self, id: i64) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT id, source_id, name, url, enabled, archived_at, consecutive_failures, last_success_at, last_error_detail FROM news_feeds WHERE id = ?",
        )
        .bind(id).fetch_optional(&self.pool).await?;
        Ok(row.as_ref().map(news_feed_json))
    }

    pub async fn user_news_sources(&self, user_id: i64) -> Result<Value, sqlx::Error> {
        let sources = self.admin_news_sources().await?;
        let mut items = Vec::new();
        let mut unread_total = 0;
        for source in sources {
            if source["enabled"] != true {
                continue;
            }
            let id = source["id"].as_i64().unwrap_or(0);
            let chosen = self.news_source_chosen(user_id, id).await?;
            let unread = if chosen {
                self.news_unread(user_id, Some(id)).await?
            } else {
                0
            };
            unread_total += unread;
            items.push(json!({
                "id": id,
                "slug": source["slug"],
                "name": source["name"],
                "enabled": true,
                "selected": chosen,
                "status": "ok",
                "last_success_at": source["last_success_at"],
                "group_name": source["group_name"],
                "kind": source["kind"],
                "unread_count": unread,
            }));
        }
        Ok(json!({
            "items": items,
            "collection_enabled": self.news_flag("news_enabled", false).await?,
            "unread_count": unread_total,
        }))
    }

    pub async fn list_news(
        &self,
        user_id: i64,
        source_id: i64,
        q: &str,
        unread: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Value, sqlx::Error> {
        let like = like_pattern(q);
        let rows = sqlx::query(
            "SELECT a.id, a.source_id, s.name AS source_name, COALESCE(s.platform, '') AS source_platform, f.name AS feed_name,
                    a.title, a.summary, a.url, a.author, a.published_at, a.topics,
                    CASE WHEN a.images NOT IN ('', '[]') THEN 1 ELSE 0 END AS has_image,
                    CASE WHEN r.user_id IS NOT NULL OR (COALESCE(sn.seen_at, '') != '' AND a.published_at != '' AND a.published_at <= sn.seen_at) THEN 1 ELSE 0 END AS is_read
             FROM news_articles a
             JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 AND s.archived_at IS NULL
             LEFT JOIN news_feeds f ON f.id = a.feed_id
             LEFT JOIN news_reads r ON r.article_id = a.id AND r.user_id = ?
             LEFT JOIN news_seen sn ON sn.user_id = ?
             WHERE (? = 0 OR a.source_id = ?)
               AND (
                 NOT EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ?)
                 OR EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ? AND u.source_id = a.source_id)
               )
               AND (? = '' OR a.title LIKE ? ESCAPE '!' OR a.summary LIKE ? ESCAPE '!')
               AND (? = 0 OR NOT (r.user_id IS NOT NULL OR (COALESCE(sn.seen_at, '') != '' AND a.published_at != '' AND a.published_at <= sn.seen_at)))
             ORDER BY a.published_at DESC, a.id DESC
             LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(source_id)
        .bind(source_id)
        .bind(user_id)
        .bind(user_id)
        .bind(q)
        .bind(&like)
        .bind(&like)
        .bind(i64::from(unread))
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let has_more = rows.len() as i64 > limit;
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| news_item(&row, false))
            .collect::<Vec<_>>();
        Ok(json!({
            "items": items,
            "offset": offset,
            "next_offset": offset + items.len() as i64,
            "has_more": has_more,
        }))
    }

    pub async fn news_article(
        &self,
        user_id: i64,
        article_id: i64,
    ) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT a.id, a.source_id, s.name AS source_name, COALESCE(s.platform, '') AS source_platform, f.name AS feed_name,
                    a.title, a.summary, a.content, a.url, a.author, a.published_at, a.topics,
                    CASE WHEN a.images NOT IN ('', '[]') THEN 1 ELSE 0 END AS has_image,
                    CASE WHEN r.user_id IS NOT NULL OR (COALESCE(sn.seen_at, '') != '' AND a.published_at != '' AND a.published_at <= sn.seen_at) THEN 1 ELSE 0 END AS is_read
             FROM news_articles a
             JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 AND s.archived_at IS NULL
             LEFT JOIN news_feeds f ON f.id = a.feed_id
             LEFT JOIN news_reads r ON r.article_id = a.id AND r.user_id = ?
             LEFT JOIN news_seen sn ON sn.user_id = ?
             WHERE a.id = ?
               AND (
                 NOT EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ?)
                 OR EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ? AND u.source_id = a.source_id)
               )",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(article_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(None) };
        let source_id: i64 = row.get("source_id");
        let published: String = row.get("published_at");
        let mut article = news_item(&row, true);
        article["prev_id"] = json!(
            self.news_neighbor(source_id, &published, article_id, true)
                .await?
        );
        article["next_id"] = json!(
            self.news_neighbor(source_id, &published, article_id, false)
                .await?
        );
        Ok(Some(article))
    }

    pub async fn magazine(&self, user_id: i64, source_id: i64) -> Result<Value, CatalogError> {
        let source =
            sqlx::query("SELECT kind, enabled, archived_at FROM news_sources WHERE id = ?")
                .bind(source_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(source) = source else {
            return Err(CatalogError::Bad("新闻来源不存在或已归档"));
        };
        let archived: Option<String> = source.get("archived_at");
        if source.get::<i64, _>("enabled") == 0 || archived.is_some() {
            return Err(CatalogError::Bad("新闻来源不存在或已归档"));
        }
        if source.get::<String, _>("kind") != "magazine" {
            return Err(CatalogError::Bad("这个来源不是周刊"));
        }
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ?)
             OR EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ? AND u.source_id = ?)",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        if visible.is_none() {
            return Err(CatalogError::Bad("新闻来源不存在或已归档"));
        }
        let rows = sqlx::query(
            "SELECT a.id, a.title, a.author, a.issue_key, a.issue_label, a.issue_title, a.issue_cover,
                    a.section, a.published_at, a.toc_order,
                    CASE WHEN r.user_id IS NOT NULL OR (COALESCE(sn.seen_at, '') != '' AND a.published_at != '' AND a.published_at <= sn.seen_at) THEN 1 ELSE 0 END AS is_read
             FROM news_articles a
             JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 AND s.archived_at IS NULL
             LEFT JOIN news_reads r ON r.article_id = a.id AND r.user_id = ?
             LEFT JOIN news_seen sn ON sn.user_id = ?
             WHERE a.source_id = ? AND a.issue_key != ''
             ORDER BY a.published_at DESC, a.toc_order ASC, a.id ASC",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(source_id)
        .fetch_all(&self.pool)
        .await?;
        let mut order = Vec::new();
        let mut issues: Vec<(String, Value)> = Vec::new();
        for row in rows {
            let key: String = row.get("issue_key");
            if !order.contains(&key) {
                order.push(key.clone());
                issues.push((
                    key.clone(),
                    json!({
                        "issue_key": key,
                        "label": row.get::<String, _>("issue_label"),
                        "title": row.get::<String, _>("issue_title"),
                        "cover": row.get::<String, _>("issue_cover"),
                        "published_at": row.get::<String, _>("published_at"),
                        "articles": [],
                    }),
                ));
            }
            let issue = &mut issues
                .iter_mut()
                .find(|(found, _)| found == &key)
                .expect("issue")
                .1;
            let label: String = row.get("issue_label");
            if issue["label"].as_str().unwrap_or("").is_empty() && !label.is_empty() {
                issue["label"] = json!(label);
            }
            if issue["label"].as_str().unwrap_or("").is_empty() {
                issue["label"] = json!(key);
            }
            let title: String = row.get("issue_title");
            if issue["title"].as_str().unwrap_or("").is_empty() && !title.is_empty() {
                issue["title"] = json!(title);
            }
            let cover: String = row.get("issue_cover");
            if issue["cover"].as_str().unwrap_or("").is_empty() && !cover.is_empty() {
                issue["cover"] = json!(cover);
            }
            let section: String = row.get("section");
            issue["articles"]
                .as_array_mut()
                .expect("articles")
                .push(json!({
                    "id": row.get::<i64, _>("id"),
                    "title": row.get::<String, _>("title"),
                    "author": row.get::<String, _>("author"),
                    "section": if section.is_empty() { "正文" } else { &section },
                    "is_read": row.get::<i64, _>("is_read") != 0,
                }));
        }
        Ok(
            json!({"source_id": source_id, "issues": issues.into_iter().map(|(_, issue)| issue).collect::<Vec<_>>()}),
        )
    }

    pub async fn news_image_url(
        &self,
        user_id: i64,
        article_id: i64,
        index: i64,
    ) -> Result<String, CatalogError> {
        if index < 0 {
            return Err(CatalogError::Missing("图片不存在"));
        }
        let images: Option<String> = sqlx::query_scalar(
            "SELECT a.images FROM news_articles a
             JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 AND s.archived_at IS NULL
             WHERE a.id = ?
               AND (
                 NOT EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ?)
                 OR EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ? AND u.source_id = a.source_id)
               )",
        )
        .bind(article_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(images) = images else {
            return Err(CatalogError::Missing("图片不存在"));
        };
        let list: Vec<String> = serde_json::from_str(&images).unwrap_or_default();
        list.get(index as usize)
            .filter(|url| !url.is_empty())
            .cloned()
            .ok_or(CatalogError::Missing("图片不存在"))
    }

    async fn news_neighbor(
        &self,
        source_id: i64,
        published: &str,
        article_id: i64,
        newer: bool,
    ) -> Result<Option<i64>, sqlx::Error> {
        let sql = if newer {
            "SELECT id FROM news_articles WHERE source_id = ? AND (published_at > ? OR (published_at = ? AND id > ?)) ORDER BY published_at, id LIMIT 1"
        } else {
            "SELECT id FROM news_articles WHERE source_id = ? AND (published_at < ? OR (published_at = ? AND id < ?)) ORDER BY published_at DESC, id DESC LIMIT 1"
        };
        sqlx::query_scalar(sql)
            .bind(source_id)
            .bind(published)
            .bind(published)
            .bind(article_id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn mark_news_read(&self, user_id: i64, article_id: i64) -> Result<bool, sqlx::Error> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT a.id FROM news_articles a JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 WHERE a.id = ?",
        )
        .bind(article_id)
        .fetch_optional(&self.pool)
        .await?;
        if found.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO news_reads (user_id, article_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(article_id)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn news_seen(&self, user_id: i64) -> Result<String, sqlx::Error> {
        Ok(
            sqlx::query_scalar("SELECT seen_at FROM news_seen WHERE user_id = ?")
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_default(),
        )
    }

    pub async fn set_news_seen(&self, user_id: i64, seen_at: &str) -> Result<String, sqlx::Error> {
        let previous = self.news_seen(user_id).await?;
        sqlx::query(
            "INSERT INTO news_seen (user_id, seen_at) VALUES (?, ?)
             ON CONFLICT(user_id) DO UPDATE SET seen_at = excluded.seen_at",
        )
        .bind(user_id)
        .bind(seen_at)
        .execute(&self.pool)
        .await?;
        Ok(previous)
    }

    pub async fn mark_news_seen_now(&self, user_id: i64) -> Result<(String, String), sqlx::Error> {
        let now: String = sqlx::query_scalar("SELECT datetime('now')")
            .fetch_one(&self.pool)
            .await?;
        let previous = self.set_news_seen(user_id, &now).await?;
        Ok((now, previous))
    }

    async fn news_unread(&self, user_id: i64, source_id: Option<i64>) -> Result<i64, sqlx::Error> {
        let source_id = source_id.unwrap_or(0);
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM news_articles a
             JOIN news_sources s ON s.id = a.source_id AND s.enabled = 1 AND s.archived_at IS NULL
             LEFT JOIN news_reads r ON r.article_id = a.id AND r.user_id = ?
             LEFT JOIN news_seen sn ON sn.user_id = ?
             WHERE (? = 0 OR a.source_id = ?)
               AND (
                 NOT EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ?)
                 OR EXISTS (SELECT 1 FROM user_news_sources u WHERE u.user_id = ? AND u.source_id = a.source_id)
               )
               AND NOT (r.user_id IS NOT NULL OR (COALESCE(sn.seen_at, '') != '' AND a.published_at != '' AND a.published_at <= sn.seen_at))",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(source_id)
        .bind(source_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn news_source_chosen(&self, user_id: i64, source_id: i64) -> Result<bool, sqlx::Error> {
        let any: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_news_sources WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?;
        if any == 0 {
            return Ok(true);
        }
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM user_news_sources WHERE user_id = ? AND source_id = ?",
        )
        .bind(user_id)
        .bind(source_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.is_some())
    }

    async fn enabled_counts(&self) -> Result<HashMap<String, i64>, sqlx::Error> {
        let rows = sqlx::query("SELECT platform, SUM(enabled) AS n FROM kols GROUP BY platform")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get("platform"), row.get("n")))
            .collect())
    }

    pub async fn user_by_username(&self, username: &str) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(user_from_row))
    }

    pub async fn user_by_telegram_chat_id(
        &self,
        identity: &str,
    ) -> Result<Option<User>, sqlx::Error> {
        if identity.trim().is_empty() {
            return Ok(None);
        }
        let rows = sqlx::query("SELECT * FROM users WHERE telegram_chat_id = ? LIMIT 2")
            .bind(identity.trim())
            .fetch_all(&self.pool)
            .await?;
        Ok((rows.len() == 1).then(|| user_from_row(rows.into_iter().next().unwrap())))
    }

    // The caller must pass true only for a private Telegram chat.
    pub async fn get_or_create_telegram_user(
        &self,
        identity: &str,
        display_name: &str,
        is_private: bool,
    ) -> Result<Option<User>, CatalogError> {
        if !is_private {
            return Ok(None);
        }
        let identity = identity.trim();
        if identity.is_empty() {
            return Err(CatalogError::Bad("绑定身份不能为空"));
        }
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query("SELECT * FROM users WHERE telegram_chat_id = ? LIMIT 2")
            .bind(identity)
            .fetch_all(&mut *tx)
            .await?;
        if existing.len() > 1 {
            return Err(CatalogError::Bad("Telegram 身份重复"));
        }
        if let Some(row) = existing.into_iter().next() {
            return Ok(Some(user_from_row(row)));
        }
        let preferred: String = display_name.trim().chars().take(30).collect();
        let fallback: String = format!("tg_{identity}").chars().take(30).collect();
        for candidate in [preferred.as_str(), fallback.as_str()] {
            if candidate.is_empty() {
                continue;
            }
            let result = sqlx::query("INSERT OR IGNORE INTO users (username, telegram_chat_id, telegram_provisional) VALUES (?, ?, 1)")
                .bind(candidate).bind(identity).execute(&mut *tx).await?;
            if result.rows_affected() != 0 {
                let row = sqlx::query("SELECT * FROM users WHERE telegram_chat_id = ?")
                    .bind(identity)
                    .fetch_one(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(Some(user_from_row(row)));
            }
        }
        // A stable numeric suffix handles collisions with both display names and tg_<id>.
        for n in 1..=100 {
            let suffix = format!("_{n}");
            let base: String = fallback.chars().take(30 - suffix.len()).collect();
            let name = format!("{base}{suffix}");
            let result = sqlx::query("INSERT OR IGNORE INTO users (username, telegram_chat_id, telegram_provisional) VALUES (?, ?, 1)")
                .bind(name).bind(identity).execute(&mut *tx).await?;
            if result.rows_affected() != 0 {
                let row = sqlx::query("SELECT * FROM users WHERE telegram_chat_id = ?")
                    .bind(identity)
                    .fetch_one(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(Some(user_from_row(row)));
            }
        }
        Err(CatalogError::Bad("无法分配 Telegram 用户名"))
    }

    pub async fn user_by_feishu_open_id(&self, open_id: &str) -> Result<Option<User>, sqlx::Error> {
        let row =
            sqlx::query("SELECT * FROM users WHERE feishu_open_id = ? AND feishu_open_id != ''")
                .bind(open_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(user_from_row))
    }

    pub async fn upsert_feishu_identity(
        &self,
        open_id: &str,
        chat_id: &str,
        display_name: &str,
    ) -> Result<User, CatalogError> {
        let open_id = feishu_identity(open_id)?;
        let chat_id = feishu_identity(chat_id)?;
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query("SELECT * FROM users WHERE feishu_open_id = ? LIMIT 2")
            .bind(open_id)
            .fetch_all(&mut *tx)
            .await?;
        if existing.len() > 1 {
            return Err(CatalogError::Bad("飞书身份重复"));
        }
        if let Some(row) = existing.into_iter().next() {
            let user = user_from_row(row);
            if user.feishu_chat_id != chat_id {
                sqlx::query("UPDATE users SET feishu_chat_id = ? WHERE id = ?")
                    .bind(chat_id)
                    .bind(user.id)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            return self
                .user_by_id(user.id)
                .await?
                .ok_or(CatalogError::Bad("飞书用户不存在"));
        }
        let preferred: String = display_name.trim().chars().take(30).collect();
        let fallback: String = format!("fs_{open_id}").chars().take(30).collect();
        for candidate in [preferred.as_str(), fallback.as_str()] {
            if candidate.is_empty() {
                continue;
            }
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO users (username, feishu_open_id, feishu_chat_id) VALUES (?, ?, ?)",
            )
            .bind(candidate)
            .bind(open_id)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() != 0 {
                let row = sqlx::query("SELECT * FROM users WHERE feishu_open_id = ?")
                    .bind(open_id)
                    .fetch_one(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(user_from_row(row));
            }
        }
        Err(CatalogError::Bad("无法分配飞书用户名"))
    }

    pub async fn release_feishu_placeholder(&self, open_id: &str) -> Result<bool, sqlx::Error> {
        let removed = sqlx::query(
            "DELETE FROM users WHERE feishu_open_id = ? AND password_hash = '' AND is_admin = 0
             AND telegram_chat_id = '' AND wechat_openid = ''
             AND NOT EXISTS (SELECT 1 FROM subscriptions WHERE user_id = users.id)
             AND NOT EXISTS (SELECT 1 FROM feishu_personal_bots WHERE user_id = users.id)",
        )
        .bind(open_id)
        .execute(&self.pool)
        .await?;
        Ok(removed.rows_affected() != 0)
    }

    pub async fn user_by_openid(&self, openid: &str) -> Result<Option<User>, sqlx::Error> {
        let row =
            sqlx::query("SELECT * FROM users WHERE wechat_openid = ? AND wechat_openid != ''")
                .bind(openid)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(user_from_row))
    }

    pub async fn register_wechat(
        &self,
        code: &str,
        username: &str,
        openid: &str,
    ) -> Result<i64, RegisterError> {
        let code = code.trim().to_ascii_uppercase();
        let openid = openid.trim();
        if openid.is_empty() {
            return Err(RegisterError::Rejected("微信登录态无效"));
        }
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE wechat_openid = ?")
            .bind(openid)
            .fetch_optional(&mut *tx)
            .await?;
        if existing.is_some() {
            return Err(RegisterError::Rejected("微信账号已存在"));
        }
        let used = sqlx::query(
            "UPDATE register_codes SET used_at = datetime('now')
             WHERE code = ? AND used_by IS NULL AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > datetime('now'))",
        )
        .bind(&code)
        .execute(&mut *tx)
        .await?;
        if used.rows_affected() == 0 {
            let row = sqlx::query("SELECT used_by, revoked_at FROM register_codes WHERE code = ?")
                .bind(&code)
                .fetch_optional(&mut *tx)
                .await?;
            let msg = match row {
                None => "邀请码无效或已被使用",
                Some(row) if row.get::<Option<i64>, _>("used_by").is_some() => {
                    "邀请码无效或已被使用"
                }
                Some(row) if row.get::<Option<String>, _>("revoked_at").is_some() => {
                    "邀请码已作废，请向管理员索取新的"
                }
                Some(_) => "邀请码已过期，请向管理员索取新的",
            };
            return Err(RegisterError::Rejected(msg));
        }
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_some() {
            return Err(RegisterError::Rejected("用户名已存在"));
        }
        let inserted = sqlx::query("INSERT INTO users (username, password_hash, is_admin, wechat_openid) VALUES (?, '', 0, ?)")
            .bind(username)
            .bind(openid)
            .execute(&mut *tx)
            .await;
        let id = match inserted {
            Ok(res) => res.last_insert_rowid(),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err(RegisterError::Rejected("微信账号已存在"))
            }
            Err(err) => return Err(err.into()),
        };
        sqlx::query("UPDATE register_codes SET used_by = ? WHERE code = ?")
            .bind(id)
            .bind(&code)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn user_by_id(&self, id: i64) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(user_from_row))
    }

    pub async fn ensure_admin(&self, password_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin)
             SELECT 'admin', ?, 1
             WHERE NOT EXISTS (SELECT 1 FROM users WHERE username = 'admin' COLLATE NOCASE)",
        )
        .bind(password_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn touch_login(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET last_login_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_password(&self, id: i64, password_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE users SET password_hash = ?, token_version = token_version + 1 WHERE id = ?",
        )
        .bind(password_hash)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_user_text(
        &self,
        id: i64,
        column: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        let sql = match column {
            "telegram_chat_id" => "UPDATE users SET telegram_chat_id = ? WHERE id = ?",
            "telegram_bot_token" => "UPDATE users SET telegram_bot_token = ? WHERE id = ?",
            "feishu_open_id" => "UPDATE users SET feishu_open_id = ? WHERE id = ?",
            "feishu_chat_id" => "UPDATE users SET feishu_chat_id = ? WHERE id = ?",
            "wecom_webhook" => "UPDATE users SET wecom_webhook = ? WHERE id = ?",
            "bark_key" => "UPDATE users SET bark_key = ? WHERE id = ?",
            "push_channels" => "UPDATE users SET push_channels = ? WHERE id = ?",
            "dnd_start" => "UPDATE users SET dnd_start = ? WHERE id = ?",
            "dnd_end" => "UPDATE users SET dnd_end = ? WHERE id = ?",
            "keywords" => "UPDATE users SET keywords = ? WHERE id = ?",
            "news_font_size" => "UPDATE users SET news_font_size = ? WHERE id = ?",
            "llm_api_base" => "UPDATE users SET llm_api_base = ? WHERE id = ?",
            "llm_api_key" => "UPDATE users SET llm_api_key = ? WHERE id = ?",
            "llm_model" => "UPDATE users SET llm_model = ? WHERE id = ?",
            "llm_api_format" => "UPDATE users SET llm_api_format = ? WHERE id = ?",
            other => panic!("unknown user text column {other}"),
        };
        sqlx::query(sql)
            .bind(value)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_user_flag(
        &self,
        id: i64,
        column: &str,
        value: bool,
    ) -> Result<(), sqlx::Error> {
        let sql = match column {
            "notify_enabled" => "UPDATE users SET notify_enabled = ? WHERE id = ?",
            "daily_report" => "UPDATE users SET daily_report = ? WHERE id = ?",
            "translate_twitter" => "UPDATE users SET translate_twitter = ? WHERE id = ?",
            "dnd_allow_favorite" => "UPDATE users SET dnd_allow_favorite = ? WHERE id = ?",
            "keywords_match_reports" => "UPDATE users SET keywords_match_reports = ? WHERE id = ?",
            "keywords_match_news" => "UPDATE users SET keywords_match_news = ? WHERE id = ?",
            other => panic!("unknown user flag {other}"),
        };
        sqlx::query(sql)
            .bind(i64::from(value))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn issue_bind_code(
        &self,
        user_id: i64,
        now: i64,
    ) -> Result<(String, i64), CatalogError> {
        const TTL: i64 = 600;
        const LIMIT: i64 = 3;
        let window = now.div_euclid(TTL) * TTL;
        let key = format!("issue:{user_id}");
        let mut tx = self.pool.begin().await?;
        let quota: Option<(i64, i64)> =
            sqlx::query_as("SELECT period_start, count FROM bind_quota WHERE key = ?")
                .bind(&key)
                .fetch_optional(&mut *tx)
                .await?;
        let count = match quota {
            Some((start, count)) if start == window => count,
            _ => 0,
        };
        if count >= LIMIT {
            return Err(CatalogError::Limited("绑定码生成过于频繁，请稍后再试"));
        }
        sqlx::query(
            "INSERT INTO bind_quota (key, period_start, count) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET period_start = excluded.period_start, count = excluded.count",
        )
        .bind(&key)
        .bind(window)
        .bind(count + 1)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM bind_codes WHERE expires_at < ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let mut issued = None;
        for _ in 0..8 {
            let code = bind_code_text()?;
            let digest = bind_code_digest(&code);
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO bind_codes (code, user_id, expires_at) VALUES (?, ?, ?)",
            )
            .bind(&digest)
            .bind(user_id)
            .bind(now + TTL)
            .execute(&mut *tx)
            .await?;
            if inserted.rows_affected() == 1 {
                issued = Some(code);
                break;
            }
        }
        let Some(code) = issued else {
            return Err(CatalogError::Bad("绑定码生成失败，请重试"));
        };
        tx.commit().await?;
        Ok((code, TTL))
    }

    pub async fn consume_bind_code(
        &self,
        code: &str,
        channel: &str,
        identity: &str,
        now: i64,
    ) -> Result<Option<i64>, CatalogError> {
        let column = match channel {
            "telegram_chat_id" | "feishu_open_id" => channel,
            _ => return Err(CatalogError::Bad("不支持的绑定渠道")),
        };
        let identity = identity.trim();
        if identity.is_empty() {
            return Err(CatalogError::Bad("绑定身份不能为空"));
        }
        let digest = bind_code_digest(code);
        if digest.is_empty() {
            return Ok(None);
        }
        let mut tx = self.pool.begin().await?;
        let user_id: Option<i64> = sqlx::query_scalar(
            "DELETE FROM bind_codes WHERE code = ? AND expires_at >= ? RETURNING user_id",
        )
        .bind(&digest)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(user_id) = user_id else {
            tx.commit().await?;
            return Ok(None);
        };
        let owner: Option<i64> = match column {
            "telegram_chat_id" => {
                sqlx::query_scalar(
                    "SELECT id FROM users WHERE telegram_chat_id = ? AND id != ? LIMIT 1",
                )
                .bind(identity)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?
            }
            _ => {
                sqlx::query_scalar(
                    "SELECT id FROM users WHERE feishu_open_id = ? AND id != ? LIMIT 1",
                )
                .bind(identity)
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?
            }
        };
        if let Some(owner) = owner {
            if column != "telegram_chat_id" {
                return Err(CatalogError::Bad("该渠道已绑定其他账号"));
            }
            let owner_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM users WHERE telegram_chat_id = ? AND id != ?",
            )
            .bind(identity)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            if owner_count != 1 {
                return Err(CatalogError::Bad("该渠道已绑定其他账号"));
            }
            // Only explicitly marked bot accounts with default personal settings
            // may be absorbed; credentials and other bindings remain disqualifying.
            let eligible: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM users WHERE id = ? AND telegram_chat_id = ?
                 AND telegram_provisional = 1
                 AND password_hash = '' AND is_admin = 0 AND wechat_openid = ''
                 AND feishu_open_id = '' AND feishu_chat_id = '' AND telegram_bot_token = ''
                 AND wecom_webhook = '' AND bark_key = '' AND llm_api_key = ''
                 AND token_version = 0 AND last_login_at IS NULL
                 AND notify_enabled = 1 AND daily_report = 0 AND translate_twitter = 1
                 AND push_channels = '' AND dnd_start = '' AND dnd_end = ''
                 AND dnd_allow_favorite = 0 AND keywords = '[]'
                 AND keywords_match_reports = 0 AND keywords_match_reports_since = ''
                 AND keywords_match_news = 0 AND keywords_match_news_since = ''
                 AND news_font_size = '' AND llm_api_base = '' AND llm_model = ''
                 AND llm_api_format = 'chat'
                 AND NOT EXISTS (SELECT 1 FROM webpush_subscriptions WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM android_devices WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM feishu_personal_bots WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM feishu_registration_sessions WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM feishu_oauth_sessions WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM kol_acl WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM kol_requests WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM register_codes WHERE used_by = users.id OR created_by = users.id)
                 AND NOT EXISTS (SELECT 1 FROM ima_kb_acl WHERE user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM ima_kb_subscriptions WHERE user_id = users.id)"
            ).bind(owner).bind(identity).fetch_optional(&mut *tx).await?;
            if eligible.is_none() {
                return Err(CatalogError::Bad("该渠道已绑定其他账号"));
            }
            sqlx::query(
                "INSERT INTO subscriptions (user_id, kol_id, type, favorite, secondary, hide_images)
                 SELECT ?, kol_id, type, favorite, secondary, hide_images FROM subscriptions WHERE user_id = ?
                 ON CONFLICT(user_id, kol_id) DO UPDATE SET
                   type = CASE WHEN subscriptions.type = excluded.type THEN subscriptions.type ELSE 'both' END,
                   favorite = MAX(subscriptions.favorite, excluded.favorite),
                   secondary = MAX(subscriptions.secondary, excluded.secondary),
                   hide_images = MAX(subscriptions.hide_images, excluded.hide_images)"
            ).bind(user_id).bind(owner).execute(&mut *tx).await?;
            for table in [
                "user_news_sources",
                "news_reads",
                "news_keyword_notified",
                "knowledge_keyword_notified",
            ] {
                let columns = match table {
                    "user_news_sources" => "user_id, source_id",
                    "news_reads" => "user_id, article_id",
                    "news_keyword_notified" => "user_id, article_id, created_at",
                    _ => "user_id, group_id, media_id, created_at",
                };
                let values = match table {
                    "user_news_sources" => "?, source_id",
                    "news_reads" => "?, article_id",
                    "news_keyword_notified" => "?, article_id, created_at",
                    _ => "?, group_id, media_id, created_at",
                };
                sqlx::query(&format!("INSERT OR IGNORE INTO {table} ({columns}) SELECT {values} FROM {table} WHERE user_id = ?"))
                    .bind(user_id).bind(owner).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO news_seen (user_id, seen_at) SELECT ?, seen_at FROM news_seen WHERE user_id = ? ON CONFLICT(user_id) DO UPDATE SET seen_at = MAX(news_seen.seen_at, excluded.seen_at)")
                .bind(user_id).bind(owner).execute(&mut *tx).await?;
            sqlx::query("UPDATE users SET telegram_chat_id = ?, news_last_seen_at = MAX(news_last_seen_at, (SELECT news_last_seen_at FROM users WHERE id = ?)) WHERE id = ?")
                .bind(identity).bind(owner).bind(user_id).execute(&mut *tx).await?;
            sqlx::query(
                "INSERT INTO push_retries (channel, user_id, platform, external_id, post_id, attempts, next_at)
                 SELECT channel, ?, platform, external_id, post_id, attempts, next_at
                 FROM push_retries WHERE user_id = ?
                 ON CONFLICT(channel, user_id, platform, external_id) DO UPDATE SET
                   attempts = MAX(push_retries.attempts, excluded.attempts),
                   next_at = MIN(push_retries.next_at, excluded.next_at)"
            ).bind(user_id).bind(owner).execute(&mut *tx).await?;
            sqlx::query("UPDATE push_logs SET user_id = ? WHERE user_id = ?")
                .bind(user_id)
                .bind(owner)
                .execute(&mut *tx)
                .await?;
            for table in [
                "subscriptions",
                "user_news_sources",
                "news_reads",
                "news_seen",
                "news_keyword_notified",
                "knowledge_keyword_notified",
                "push_retries",
                "bind_codes",
                "bind_quota",
            ] {
                if table == "bind_quota" {
                    sqlx::query("DELETE FROM bind_quota WHERE key = ?")
                        .bind(format!("issue:{owner}"))
                        .execute(&mut *tx)
                        .await?;
                } else {
                    sqlx::query(&format!("DELETE FROM {table} WHERE user_id = ?"))
                        .bind(owner)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            sqlx::query("DELETE FROM users WHERE id = ?")
                .bind(owner)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(Some(user_id));
        }
        let sql = match column {
            "telegram_chat_id" => "UPDATE users SET telegram_chat_id = ? WHERE id = ?",
            _ => "UPDATE users SET feishu_open_id = ? WHERE id = ?",
        };
        sqlx::query(sql)
            .bind(identity)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(user_id))
    }

    pub async fn cancel_feishu_sessions(&self, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE feishu_registration_sessions SET status = 'cancelled'
             WHERE user_id = ? AND status NOT IN ('expired', 'cancelled')",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_feishu_session(
        &self,
        session_id: &str,
        user_id: i64,
        device_code: &str,
        base_url: &str,
        verification_uri: &str,
        expires_at: i64,
        interval: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO feishu_registration_sessions
             (session_id, user_id, device_code_ciphertext, registration_base_url, verification_uri, session_expires_at, poll_interval, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(device_code)
        .bind(base_url)
        .bind(verification_uri)
        .bind(expires_at)
        .bind(interval)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn feishu_session(
        &self,
        session_id: &str,
    ) -> Result<Option<FeishuSession>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT session_id, user_id, device_code_ciphertext, registration_base_url, verification_uri,
                    candidate_app_id, candidate_app_secret_ciphertext, candidate_tenant_brand, expected_open_id,
                    bind_code_hash, bind_code_expires_at, session_expires_at, poll_interval, status, last_error
             FROM feishu_registration_sessions WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(feishu_session_from_row))
    }

    pub async fn set_feishu_status(
        &self,
        session_id: &str,
        status: &str,
        last_error: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE feishu_registration_sessions SET status = ?, last_error = ? WHERE session_id = ?")
            .bind(status)
            .bind(last_error)
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_feishu_credentials(
        &self,
        session_id: &str,
        app_id: &str,
        secret: &str,
        brand: &str,
        open_id: &str,
        base_url: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE feishu_registration_sessions
             SET status = 'credentials_created', candidate_app_id = ?, candidate_app_secret_ciphertext = ?,
                 candidate_tenant_brand = ?, expected_open_id = ?, registration_base_url = ?, last_error = ''
             WHERE session_id = ?",
        )
        .bind(app_id)
        .bind(secret)
        .bind(brand)
        .bind(open_id)
        .bind(base_url)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn save_feishu_bind_code(
        &self,
        session_id: &str,
        hash: &str,
        expires_at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE feishu_registration_sessions
             SET status = 'awaiting_bind', bind_code_hash = ?, bind_code_expires_at = ?
             WHERE session_id = ?",
        )
        .bind(hash)
        .bind(expires_at)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_feishu_bind_code(
        &self,
        session_id: &str,
        status: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE feishu_registration_sessions
             SET status = ?, bind_code_hash = '', bind_code_expires_at = NULL
             WHERE session_id = ?",
        )
        .bind(status)
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn feishu_sessions_by_status(
        &self,
        status: &str,
    ) -> Result<Vec<FeishuSession>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM feishu_registration_sessions WHERE status = ? ORDER BY session_id",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(feishu_session_from_row).collect())
    }

    pub async fn feishu_bot(&self, user_id: i64) -> Result<Option<FeishuBot>, sqlx::Error> {
        let row = sqlx::query("SELECT status, app_id FROM feishu_personal_bots WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| FeishuBot {
            status: row.get("status"),
            app_id: row.get("app_id"),
        }))
    }

    pub async fn save_feishu_bot(
        &self,
        user_id: i64,
        app_id: &str,
        secret: &str,
        brand: &str,
        open_id: &str,
        chat_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO feishu_personal_bots
                (user_id, app_id, app_secret_ciphertext, tenant_brand, status, open_id, chat_id)
             VALUES (?, ?, ?, ?, 'active', ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET
                app_id = excluded.app_id,
                app_secret_ciphertext = excluded.app_secret_ciphertext,
                tenant_brand = excluded.tenant_brand,
                status = 'active',
                open_id = excluded.open_id,
                chat_id = excluded.chat_id,
                last_error = ''",
        )
        .bind(user_id)
        .bind(app_id)
        .bind(secret)
        .bind(brand)
        .bind(open_id)
        .bind(chat_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn active_feishu_route(
        &self,
        user_id: i64,
    ) -> Result<Option<FeishuRoute>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT app_id, app_secret_ciphertext, chat_id, tenant_brand
             FROM feishu_personal_bots
             WHERE user_id = ? AND status = 'active' AND chat_id != ''",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| FeishuRoute {
            app_id: row.get("app_id"),
            app_secret_ciphertext: row.get("app_secret_ciphertext"),
            chat_id: row.get("chat_id"),
            tenant_brand: row.get("tenant_brand"),
        }))
    }

    pub async fn degrade_feishu_bot(&self, user_id: i64, error: &str) -> Result<(), sqlx::Error> {
        let error: String = error.chars().take(300).collect();
        sqlx::query(
            "UPDATE feishu_personal_bots
             SET status = 'degraded', last_error = ?
             WHERE user_id = ? AND status = 'active'",
        )
        .bind(error)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_feishu_bot(&self, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM feishu_personal_bots WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn other_user_has(&self, column: &str, value: &str, id: i64) -> Result<bool, String> {
        if column == "telegram_bot_token" {
            let rows = sqlx::query("SELECT telegram_bot_token FROM users WHERE id != ?")
                .bind(id)
                .fetch_all(&self.pool)
                .await
                .map_err(|_| "读取 Telegram 配置失败".to_string())?;
            let key = crate::feishu_personal::credential_key().unwrap_or_default();
            for row in rows {
                let stored: String = row.get("telegram_bot_token");
                let token = crate::push::telegram_secret(&stored, &key)?;
                if token == value {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        let sql = match column {
            "telegram_chat_id" => {
                "SELECT id FROM users WHERE telegram_chat_id = ? AND id != ? LIMIT 1"
            }
            _ => return Ok(false),
        };
        Ok(sqlx::query(sql)
            .bind(value)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| "读取 Telegram 配置失败".to_string())?
            .is_some())
    }

    pub async fn push_targets(&self, kol_id: i64) -> Result<Vec<PushTarget>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT s.user_id, u.notify_enabled, u.push_channels, u.dnd_start, u.dnd_end,
                    u.dnd_allow_favorite, u.wecom_webhook, u.bark_key,
                    u.telegram_chat_id, u.telegram_bot_token, s.favorite
             FROM subscriptions s
             JOIN users u ON u.id = s.user_id
             WHERE s.kol_id = ?",
        )
        .bind(kol_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| PushTarget {
                user_id: row.get("user_id"),
                notify_enabled: row.get::<i64, _>("notify_enabled") != 0,
                push_channels: row.get("push_channels"),
                dnd_start: row.get("dnd_start"),
                dnd_end: row.get("dnd_end"),
                dnd_allow_favorite: row.get::<i64, _>("dnd_allow_favorite") != 0,
                favorite: row.get::<i64, _>("favorite") != 0,
                wecom_webhook: row.get("wecom_webhook"),
                bark_key: row.get("bark_key"),
                telegram_chat_id: row.get("telegram_chat_id"),
                telegram_bot_token: row.get("telegram_bot_token"),
            })
            .collect())
    }

    pub async fn android_device_count(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM android_devices WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn upsert_android_device(
        &self,
        installation_id: &str,
        user_id: i64,
        token: &str,
        provider: &str,
        device_model: &str,
        app_version: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO android_devices (installation_id, user_id, token, provider, device_model, app_version)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(installation_id) DO UPDATE SET
                user_id = excluded.user_id, token = excluded.token,
                provider = excluded.provider, device_model = excluded.device_model,
                app_version = excluded.app_version, updated_at = datetime('now')",
        )
        .bind(installation_id)
        .bind(user_id)
        .bind(token)
        .bind(provider)
        .bind(device_model)
        .bind(app_version)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_android_device(
        &self,
        installation_id: &str,
        user_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM android_devices WHERE installation_id = ? AND user_id = ?")
            .bind(installation_id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn webpush_count(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM webpush_subscriptions WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn webpush_subs(&self, user_id: i64) -> Result<Vec<WebPushSub>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT endpoint, p256dh, auth FROM webpush_subscriptions WHERE user_id = ? ORDER BY id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| WebPushSub {
                endpoint: row.get("endpoint"),
                p256dh: row.get("p256dh"),
                auth: row.get("auth"),
            })
            .collect())
    }

    pub async fn upsert_webpush(
        &self,
        user_id: i64,
        endpoint: &str,
        p256dh: &str,
        auth: &str,
        user_agent: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO webpush_subscriptions (user_id, endpoint, p256dh, auth, user_agent)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(endpoint) DO UPDATE SET
                user_id = excluded.user_id, p256dh = excluded.p256dh,
                auth = excluded.auth, user_agent = excluded.user_agent",
        )
        .bind(user_id)
        .bind(endpoint)
        .bind(p256dh)
        .bind(auth)
        .bind(user_agent)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_webpush_endpoint(&self, endpoint: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM webpush_subscriptions WHERE endpoint = ?")
            .bind(endpoint)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_webpush_user(&self, user_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM webpush_subscriptions WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn bump_token_version(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET token_version = token_version + 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_admin_users(&self) -> Result<Vec<Value>, sqlx::Error> {
        let (after, purge) = self.inactive_days().await?;
        let rows = sqlx::query(
            "SELECT u.id, u.username, u.is_admin, u.created_at, u.notify_enabled, u.daily_report,
                    u.dnd_start, u.push_channels, u.last_login_at,
                    u.telegram_chat_id, u.feishu_open_id, u.feishu_chat_id,
                    u.wecom_webhook, u.bark_key, u.telegram_bot_token,
                    u.password_hash != '' AS has_password,
                    COALESCE((SELECT rc.code FROM register_codes rc WHERE rc.used_by = u.id LIMIT 1), '') AS register_code,
                    (SELECT COUNT(*) FROM subscriptions s WHERE s.user_id = u.id) AS subscription_count,
                    (SELECT COUNT(*) FROM webpush_subscriptions w WHERE w.user_id = u.id) AS webpush_count
             FROM users u ORDER BY u.id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let acl = self.ima_kb_acl_map().await?;
        let subscribed = self.ima_kb_sub_map().await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let mut value = admin_user_json(&row, after, purge);
                let id = value["id"].as_i64().unwrap_or(0);
                value["ima_kb_groups"] = json!(acl.get(&id).cloned().unwrap_or_default());
                value["ima_kb_subscribed"] =
                    json!(subscribed.get(&id).cloned().unwrap_or_default());
                value
            })
            .collect())
    }

    pub async fn inactive_policy(
        &self,
        preview_after: Option<i64>,
        preview_purge: Option<i64>,
    ) -> Result<Value, CatalogError> {
        for value in [preview_after, preview_purge].into_iter().flatten() {
            if !(0..=3650).contains(&value) {
                return Err(CatalogError::Bad("天数须在 0–3650"));
            }
        }
        let (after, purge) = self.inactive_days().await?;
        let marked = self.inactive_count(preview_after.unwrap_or(after)).await?;
        let doomed = self
            .inactive_count(preview_after.unwrap_or(after) + preview_purge.unwrap_or(purge))
            .await?;
        let doomed = if preview_after.unwrap_or(after) <= 0 || preview_purge.unwrap_or(purge) <= 0 {
            0
        } else {
            doomed
        };
        let customized = self
            .setting("inactive_policy_customized")
            .await?
            .is_some_and(|v| v == "1");
        Ok(json!({
            "inactive_after_days": after,
            "inactive_purge_after_days": purge,
            "customized": customized,
            "marked_count": marked,
            "purge_count": doomed,
        }))
    }

    pub async fn set_inactive_policy(&self, after: i64, purge: i64) -> Result<Value, CatalogError> {
        if !(0..=3650).contains(&after) || !(0..=3650).contains(&purge) {
            return Err(CatalogError::Bad("天数须在 0–3650"));
        }
        self.set_setting("inactive_after_days", &after.to_string())
            .await?;
        self.set_setting("inactive_purge_after_days", &purge.to_string())
            .await?;
        self.set_setting("inactive_policy_customized", "1").await?;
        self.inactive_policy(None, None).await
    }

    pub async fn update_admin_user(
        &self,
        actor: i64,
        id: i64,
        username: Option<&str>,
        password_hash: Option<&str>,
        is_admin: Option<bool>,
    ) -> Result<(), CatalogError> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_none() {
            return Err(CatalogError::Missing("用户不存在"));
        }
        if is_admin == Some(false) && actor == id {
            return Err(CatalogError::Bad("不能取消自己的管理员权限"));
        }
        if let Some(on) = is_admin {
            sqlx::query("UPDATE users SET is_admin = ? WHERE id = ?")
                .bind(i64::from(on))
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(name) = username {
            let taken: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM users WHERE username = ? COLLATE NOCASE AND id != ?",
            )
            .bind(name)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
            if taken.is_some() {
                return Err(CatalogError::Bad("用户名已存在"));
            }
            sqlx::query("UPDATE users SET username = ? WHERE id = ?")
                .bind(name)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        if let Some(hash) = password_hash {
            self.set_password(id, hash).await?;
        }
        Ok(())
    }

    pub async fn delete_user(&self, actor: i64, id: i64) -> Result<(), CatalogError> {
        let row = sqlx::query("SELECT is_admin FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Err(CatalogError::Missing("用户不存在"));
        };
        if actor == id {
            return Err(CatalogError::Bad("不能删除自己的账号"));
        }
        if row.get::<i64, _>("is_admin") != 0 {
            return Err(CatalogError::Bad("不能删除管理员"));
        }
        let mut tx = self.pool.begin().await?;
        let tables = tables_with_column(&mut tx, "user_id").await?;
        for table in tables {
            sqlx::query(&format!("DELETE FROM \"{table}\" WHERE user_id = ?"))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn set_notify_many(&self, ids: &[i64], on: bool) -> Result<i64, sqlx::Error> {
        let mut changed = 0;
        for id in ids {
            changed += sqlx::query("UPDATE users SET notify_enabled = ? WHERE id = ?")
                .bind(i64::from(on))
                .bind(id)
                .execute(&self.pool)
                .await?
                .rows_affected();
        }
        Ok(changed as i64)
    }

    async fn inactive_days(&self) -> Result<(i64, i64), sqlx::Error> {
        let after = self
            .setting("inactive_after_days")
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(90);
        let purge = self
            .setting("inactive_purge_after_days")
            .await?
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        Ok((after, purge))
    }

    async fn inactive_count(&self, days: i64) -> Result<i64, sqlx::Error> {
        if days <= 0 {
            return Ok(0);
        }
        sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM users WHERE is_admin = 0 AND last_login_at IS NULL
             AND created_at <= datetime('now', '-{days} days')
             AND telegram_chat_id = '' AND feishu_open_id = '' AND feishu_chat_id = ''
             AND wecom_webhook = '' AND bark_key = ''
             AND NOT EXISTS (SELECT 1 FROM subscriptions s WHERE s.user_id = users.id)
             AND NOT EXISTS (SELECT 1 FROM webpush_subscriptions w WHERE w.user_id = users.id)"
        ))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn backfill_new_news_sources(&self) -> Result<i64, sqlx::Error> {
        if self.setting("news_select_new_sources_v1").await?.as_deref() == Some("1") {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let added = sqlx::query(
            "INSERT OR IGNORE INTO user_news_sources (user_id, source_id)
             SELECT u.id, s.id FROM users u JOIN news_sources s
             WHERE s.archived_at IS NULL AND s.enabled = 1 AND s.internal = 1",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected() as i64;
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('news_select_new_sources_v1', '1')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(added)
    }

    pub async fn purge_inactive_if_due(&self) -> Result<i64, sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let last = self
            .setting("inactive_users_last_purge_at")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        if last > 0 && now.saturating_sub(last) < 24 * 3600 {
            return Ok(0);
        }
        let (after, extra) = self.inactive_days().await?;
        let total = after.saturating_add(extra);
        let ids: Vec<i64> = if after <= 0 || extra <= 0 {
            Vec::new()
        } else {
            sqlx::query_scalar(&format!(
                "SELECT id FROM users WHERE is_admin = 0 AND last_login_at IS NULL
                 AND created_at <= datetime('now', '-{total} days')
                 AND telegram_chat_id = '' AND feishu_open_id = '' AND feishu_chat_id = ''
                 AND wecom_webhook = '' AND bark_key = ''
                 AND NOT EXISTS (SELECT 1 FROM feishu_personal_bots b WHERE b.user_id = users.id AND b.status = 'active' AND b.chat_id != '')
                 AND NOT EXISTS (SELECT 1 FROM webpush_subscriptions w WHERE w.user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM push_logs p WHERE p.user_id = users.id)
                 AND NOT EXISTS (SELECT 1 FROM subscriptions s WHERE s.user_id = users.id)"
            ))
            .fetch_all(&self.pool)
            .await?
        };
        let mut removed = 0;
        for id in ids {
            let name: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or_default();
            self.add_admin_log(0, "purge_inactive_user", &id.to_string(), &name)
                .await?;
            match self.delete_user(0, id).await {
                Ok(()) => removed += 1,
                Err(CatalogError::Db(err)) => return Err(err),
                Err(_) => {}
            }
        }
        self.set_setting("inactive_users_last_purge_at", &now.to_string())
            .await?;
        Ok(removed)
    }

    pub async fn prune_retention_if_due(&self) -> Result<(i64, i64, i64, i64), sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let last = self
            .setting("maintenance_last_cleanup")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0);
        if last > 0 && now.saturating_sub(last) < 6 * 3600 {
            return Ok((0, 0, 0, 0));
        }
        let posts_days = self
            .setting("stats_posts_retention_days")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(30)
            .max(0);
        let log_days = self
            .setting("stats_push_logs_retention_days")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(90)
            .max(0);
        let posts = if posts_days == 0 {
            0
        } else {
            sqlx::query(&format!("DELETE FROM push_logs WHERE post_id IN (SELECT id FROM posts WHERE fetched_at < datetime('now', '-{posts_days} days'))")).execute(&self.pool).await?;
            sqlx::query(&format!("DELETE FROM news_reads WHERE article_id IN (SELECT id FROM news_articles WHERE published_at != '' AND published_at < datetime('now', '-{posts_days} days'))")).execute(&self.pool).await?;
            let news = sqlx::query(&format!("DELETE FROM news_articles WHERE published_at != '' AND published_at < datetime('now', '-{posts_days} days')")).execute(&self.pool).await?.rows_affected();
            let posts = sqlx::query(&format!(
                "DELETE FROM posts WHERE fetched_at < datetime('now', '-{posts_days} days')"
            ))
            .execute(&self.pool)
            .await?
            .rows_affected();
            self.set_setting("maintenance_last_cleanup", &now.to_string())
                .await?;
            let logs = if log_days == 0 {
                0
            } else {
                sqlx::query(&format!(
                    "DELETE FROM push_logs WHERE created_at < datetime('now', '-{log_days} days')"
                ))
                .execute(&self.pool)
                .await?
                .rows_affected()
            };
            let admin = sqlx::query(
                "DELETE FROM admin_logs WHERE created_at < datetime('now', '-180 days')",
            )
            .execute(&self.pool)
            .await?
            .rows_affected();
            sqlx::query("DELETE FROM source_events WHERE created_at < datetime('now', '-7 days')")
                .execute(&self.pool)
                .await?;
            return Ok((posts as i64, news as i64, logs as i64, admin as i64));
        };
        let logs = if log_days == 0 {
            0
        } else {
            sqlx::query(&format!(
                "DELETE FROM push_logs WHERE created_at < datetime('now', '-{log_days} days')"
            ))
            .execute(&self.pool)
            .await?
            .rows_affected() as i64
        };
        let admin =
            sqlx::query("DELETE FROM admin_logs WHERE created_at < datetime('now', '-180 days')")
                .execute(&self.pool)
                .await?
                .rows_affected() as i64;
        sqlx::query("DELETE FROM source_events WHERE created_at < datetime('now', '-7 days')")
            .execute(&self.pool)
            .await?;
        self.set_setting("maintenance_last_cleanup", &now.to_string())
            .await?;
        Ok((posts, 0, logs, admin))
    }

    pub async fn register(
        &self,
        code: &str,
        username: &str,
        password_hash: &str,
    ) -> Result<i64, RegisterError> {
        let code = code.trim().to_ascii_uppercase();
        let mut tx = self.pool.begin().await?;
        let used = sqlx::query(
            "UPDATE register_codes SET used_at = datetime('now')
             WHERE code = ? AND used_by IS NULL AND revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > datetime('now'))",
        )
        .bind(&code)
        .execute(&mut *tx)
        .await?;
        if used.rows_affected() == 0 {
            let row = sqlx::query("SELECT used_by, revoked_at FROM register_codes WHERE code = ?")
                .bind(&code)
                .fetch_optional(&mut *tx)
                .await?;
            let msg = match row {
                None => "邀请码无效或已被使用",
                Some(row) if row.get::<Option<i64>, _>("used_by").is_some() => {
                    "邀请码无效或已被使用"
                }
                Some(row) if row.get::<Option<String>, _>("revoked_at").is_some() => {
                    "邀请码已作废，请向管理员索取新的"
                }
                Some(_) => "邀请码已过期，请向管理员索取新的",
            };
            return Err(RegisterError::Rejected(msg));
        }
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_some() {
            return Err(RegisterError::Rejected("用户名已存在"));
        }
        let inserted =
            sqlx::query("INSERT INTO users (username, password_hash, is_admin) VALUES (?, ?, 0)")
                .bind(username)
                .bind(password_hash)
                .execute(&mut *tx)
                .await;
        let id = match inserted {
            Ok(res) => res.last_insert_rowid(),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                return Err(RegisterError::Rejected("用户名已存在"));
            }
            Err(err) => return Err(err.into()),
        };
        sqlx::query("UPDATE register_codes SET used_by = ? WHERE code = ?")
            .bind(id)
            .bind(&code)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn generate_register_codes(
        &self,
        count: i64,
        note: &str,
        expires_in_days: Option<i64>,
        created_by: i64,
    ) -> Result<Value, CatalogError> {
        let count = count.clamp(1, 100);
        let note = note.trim();
        if note.chars().count() > 40 {
            return Err(CatalogError::Bad("备注最长40字"));
        }
        if let Some(days) = expires_in_days {
            if !matches!(days, 1 | 7 | 30) {
                return Err(CatalogError::Bad("有效期需为 1、7、30 天或永不过期"));
            }
        }
        let existing: Vec<String> = sqlx::query_scalar("SELECT code FROM register_codes")
            .fetch_all(&self.pool)
            .await?;
        let mut seen: std::collections::HashSet<String> = existing.into_iter().collect();
        let mut codes = Vec::new();
        let mut tries = 0;
        while codes.len() < count as usize {
            tries += 1;
            if tries > count as usize * 20 {
                return Err(CatalogError::Bad("无法生成邀请码"));
            }
            let code = random_invite_code()?;
            if seen.insert(code.clone()) {
                codes.push(code);
            }
        }
        let batch_id = random_hex(8)?;
        let expires_at: Option<String> = if let Some(days) = expires_in_days {
            sqlx::query_scalar("SELECT datetime('now', ?)")
                .bind(format!("+{days} days"))
                .fetch_one(&self.pool)
                .await?
        } else {
            None
        };
        let mut tx = self.pool.begin().await?;
        for code in &codes {
            sqlx::query(
                "INSERT INTO register_codes (code, note, batch_id, expires_at, created_by, created_at)
                 VALUES (?, ?, ?, ?, ?, datetime('now'))",
            )
            .bind(code)
            .bind(note)
            .bind(&batch_id)
            .bind(&expires_at)
            .bind(created_by)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(json!({
            "codes": codes,
            "count": codes.len(),
            "batch_id": batch_id,
            "expires_at": expires_at.unwrap_or_default(),
            "note": note,
        }))
    }

    pub async fn list_register_codes(&self) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT rc.code, rc.note, rc.used_by, rc.used_at, rc.created_at, rc.batch_id,
                    rc.expires_at, rc.revoked_at, rc.created_by,
                    u.username AS used_by_name, c.username AS created_by_name
             FROM register_codes rc
             LEFT JOIN users u ON u.id = rc.used_by
             LEFT JOIN users c ON c.id = rc.created_by
             ORDER BY rc.created_at DESC, rc.code DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "code": row.get::<String, _>("code"),
                    "note": row.get::<String, _>("note"),
                    "used_by": row.get::<Option<i64>, _>("used_by"),
                    "used_at": row.get::<Option<String>, _>("used_at"),
                    "created_at": row.get::<String, _>("created_at"),
                    "batch_id": row.get::<String, _>("batch_id"),
                    "expires_at": row.get::<Option<String>, _>("expires_at"),
                    "revoked_at": row.get::<Option<String>, _>("revoked_at"),
                    "created_by": row.get::<Option<i64>, _>("created_by"),
                    "used_by_name": row.get::<Option<String>, _>("used_by_name"),
                    "created_by_name": row.get::<Option<String>, _>("created_by_name"),
                })
            })
            .collect())
    }

    pub async fn update_register_code_note(
        &self,
        code: &str,
        note: &str,
    ) -> Result<Value, CatalogError> {
        let code = code.trim().to_ascii_uppercase();
        let changed = sqlx::query("UPDATE register_codes SET note = ? WHERE code = ?")
            .bind(note)
            .bind(&code)
            .execute(&self.pool)
            .await?;
        if changed.rows_affected() == 0 {
            return Err(CatalogError::Missing("注册码不存在"));
        }
        self.list_register_codes()
            .await?
            .into_iter()
            .find(|row| row["code"] == code)
            .ok_or_else(|| CatalogError::Missing("注册码不存在"))
    }
    pub async fn revoke_register_code(&self, code: &str) -> Result<(), CatalogError> {
        let code = code.trim().to_ascii_uppercase();
        if code.is_empty() || code.len() > 64 {
            return Err(CatalogError::Missing("注册码不存在"));
        }
        let changed = sqlx::query(
            "UPDATE register_codes SET revoked_at = datetime('now')
             WHERE code = ? AND used_by IS NULL AND revoked_at IS NULL",
        )
        .bind(&code)
        .execute(&self.pool)
        .await?;
        if changed.rows_affected() > 0 {
            return Ok(());
        }
        let row = sqlx::query("SELECT used_by, revoked_at FROM register_codes WHERE code = ?")
            .bind(&code)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Err(CatalogError::Missing("注册码不存在")),
            Some(row) if row.get::<Option<i64>, _>("used_by").is_some() => {
                Err(CatalogError::Bad("该注册码已被使用，不能删除"))
            }
            Some(row) if row.get::<Option<String>, _>("revoked_at").is_some() => {
                Err(CatalogError::Bad("该注册码已作废"))
            }
            Some(_) => Err(CatalogError::Bad("该注册码已作废")),
        }
    }

    pub async fn revoke_unused_batch(&self, batch_id: &str) -> Result<i64, CatalogError> {
        let batch_id = batch_id.trim();
        if batch_id.is_empty() || batch_id.len() > 64 {
            return Err(CatalogError::Bad("批次不存在"));
        }
        let changed = sqlx::query(
            "UPDATE register_codes SET revoked_at = datetime('now')
             WHERE batch_id = ? AND used_by IS NULL AND revoked_at IS NULL",
        )
        .bind(batch_id)
        .execute(&self.pool)
        .await?;
        Ok(changed.rows_affected() as i64)
    }

    pub async fn register_codes_batch(
        &self,
        action: &str,
        codes: &[String],
    ) -> Result<(i64, i64), CatalogError> {
        let codes: Vec<String> = codes
            .iter()
            .map(|code| code.trim().to_ascii_uppercase())
            .filter(|code| !code.is_empty() && code.len() <= 64)
            .take(500)
            .collect();
        if codes.is_empty() {
            return Err(CatalogError::Bad("请先选择注册码"));
        }
        let count = match action {
            "revoke" => {
                let mut n = 0;
                for code in &codes {
                    if self.revoke_register_code(code).await.is_ok() {
                        n += 1;
                    }
                }
                if n == 0 {
                    return Err(CatalogError::Bad("没有可作废的注册码"));
                }
                n
            }
            "delete" => {
                let mut sql = sqlx::QueryBuilder::new("DELETE FROM register_codes WHERE code IN (");
                let mut separated = sql.separated(", ");
                for code in &codes {
                    separated.push_bind(code);
                }
                separated.push_unseparated(") AND (used_by IS NOT NULL OR revoked_at IS NOT NULL OR (expires_at IS NOT NULL AND expires_at <= datetime('now')))");
                let changed = sql.build().execute(&self.pool).await?;
                let n = changed.rows_affected() as i64;
                if n == 0 {
                    return Err(CatalogError::Bad("没有可删除的注册码"));
                }
                n
            }
            other => return Err(CatalogError::Invalid(format!("不支持的操作: {other}"))),
        };
        Ok((count, codes.len() as i64 - count))
    }

    pub async fn subscription_count(&self, user_id: i64) -> Result<i64, sqlx::Error> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions WHERE user_id = ?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

    pub async fn categories(&self) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT c.id, c.name,
                    (SELECT COUNT(*) FROM kols k WHERE k.category_id = c.id AND k.enabled = 1) AS kol_count
             FROM categories c ORDER BY c.name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "name": row.get::<String, _>("name"),
                    "kol_count": row.get::<i64, _>("kol_count"),
                })
            })
            .collect())
    }

    pub async fn add_category(&self, name: &str) -> Result<i64, CatalogError> {
        let name = clean_category_name(name)?;
        match sqlx::query("INSERT INTO categories (name) VALUES (?)")
            .bind(name)
            .execute(&self.pool)
            .await
        {
            Ok(res) => Ok(res.last_insert_rowid()),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                Err(CatalogError::Bad("分类已存在"))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn category(&self, id: i64) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT c.id, c.name,
                    (SELECT COUNT(*) FROM kols k WHERE k.category_id = c.id AND k.enabled = 1) AS kol_count
             FROM categories c WHERE c.id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(|row| {
            json!({
                "id": row.get::<i64, _>("id"),
                "name": row.get::<String, _>("name"),
                "kol_count": row.get::<i64, _>("kol_count"),
            })
        }))
    }

    pub async fn rename_category(&self, id: i64, name: &str) -> Result<Value, CatalogError> {
        if self.category(id).await?.is_none() {
            return Err(CatalogError::Missing("分类不存在"));
        }
        let name = clean_category_name(name)?;
        match sqlx::query("UPDATE categories SET name = ? WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(&self.pool)
            .await
        {
            Ok(_) => Ok(self.category(id).await?.expect("category")),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                Err(CatalogError::Bad("分类已存在"))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn delete_category(&self, id: i64) -> Result<(), CatalogError> {
        if self.category(id).await?.is_none() {
            return Err(CatalogError::Missing("分类不存在"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE kols SET category_id = NULL WHERE category_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM categories WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_posts(
        &self,
        limit: i64,
        offset: i64,
        platform: &str,
        kol_id: Option<i64>,
        q: &str,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let mut sql = sqlx::QueryBuilder::new(
            "SELECT p.id, p.platform, p.kol_id, p.external_id, p.title, p.content,
                    p.title_src, p.content_src, p.post_type, p.images, p.tags, p.url, p.detail,
                    p.published_at, p.fetched_at, k.name AS kol_name, k.avatar_url,
                    k.external_id AS kol_external_id, k.category_id,
                    COALESCE(c.name, '') AS category_name, 0 AS favorite, 0 AS hide_images
             FROM posts p
             JOIN kols k ON k.id = p.kol_id
             LEFT JOIN categories c ON c.id = k.category_id
             WHERE 1 = 1",
        );
        if !platform.is_empty() {
            sql.push(" AND p.platform = ");
            sql.push_bind(platform);
        }
        if let Some(kol_id) = kol_id {
            sql.push(" AND p.kol_id = ");
            sql.push_bind(kol_id);
        }
        let q = q.trim();
        if !q.is_empty() {
            let like = like_pattern(q);
            sql.push(" AND (p.title LIKE ");
            sql.push_bind(like.clone());
            sql.push(" ESCAPE '!' OR p.content LIKE ");
            sql.push_bind(like);
            sql.push(" ESCAPE '!')");
        }
        sql.push(" ORDER BY p.id DESC LIMIT ");
        sql.push_bind(limit.clamp(1, 500));
        sql.push(" OFFSET ");
        sql.push_bind(offset.max(0));
        let rows = sql.build().fetch_all(&self.pool).await?;
        let mut posts: Vec<Value> = rows.iter().map(post_json).collect();
        crate::imgbed::rewrite_posts(self, &mut posts).await?;
        Ok(posts)
    }

    pub async fn add_push_log(
        &self,
        post_id: i64,
        channel: &str,
        status: &str,
        error: &str,
        user_id: Option<i64>,
    ) -> Result<i64, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO push_logs (post_id, channel, status, error, user_id) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(post_id)
        .bind(channel)
        .bind(status)
        .bind(error)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(res.last_insert_rowid())
    }

    pub async fn list_push_logs(
        &self,
        limit: i64,
        user_id: Option<i64>,
        channel: &str,
        status: &str,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let mut sql = sqlx::QueryBuilder::new(
            "SELECT l.id, l.post_id, l.channel, l.status, l.error, l.created_at, l.user_id,
                    p.title, k.name AS kol_name, u.username AS user_name
             FROM push_logs l
             JOIN posts p ON p.id = l.post_id
             JOIN kols k ON k.id = p.kol_id
             LEFT JOIN users u ON u.id = l.user_id
             WHERE 1 = 1",
        );
        if let Some(user_id) = user_id {
            sql.push(" AND l.user_id = ");
            sql.push_bind(user_id);
        }
        if !channel.is_empty() {
            sql.push(" AND l.channel = ");
            sql.push_bind(channel);
        }
        if !status.is_empty() {
            sql.push(" AND l.status = ");
            sql.push_bind(status);
        }
        sql.push(" ORDER BY l.id DESC LIMIT ");
        sql.push_bind(limit.clamp(1, 500));
        let rows = sql.build().fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "post_id": row.get::<i64, _>("post_id"),
                    "channel": row.get::<String, _>("channel"),
                    "status": row.get::<String, _>("status"),
                    "error": row.get::<String, _>("error"),
                    "created_at": row.get::<String, _>("created_at"),
                    "user_id": row.get::<Option<i64>, _>("user_id"),
                    "title": row.get::<String, _>("title"),
                    "kol_name": row.get::<String, _>("kol_name"),
                    "user_name": row.get::<Option<String>, _>("user_name"),
                })
            })
            .collect())
    }

    pub async fn dashboard(&self) -> Result<Value, sqlx::Error> {
        let users_total = self.scalar("SELECT COUNT(*) FROM users").await?;
        let admins = self
            .scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1")
            .await?;
        let bound = self
            .scalar(
                "SELECT COUNT(*) FROM users WHERE telegram_chat_id != '' OR feishu_open_id != ''
             OR feishu_chat_id != '' OR wecom_webhook != '' OR bark_key != ''
             OR EXISTS (SELECT 1 FROM webpush_subscriptions w WHERE w.user_id = users.id)",
            )
            .await?;
        let new_7d = self
            .scalar("SELECT COUNT(*) FROM users WHERE created_at >= datetime('now', '-7 days')")
            .await?;
        let subs = self.scalar("SELECT COUNT(*) FROM subscriptions").await?;
        let favorite = self
            .scalar("SELECT COUNT(*) FROM subscriptions WHERE favorite = 1")
            .await?;
        let posts_total = self.scalar("SELECT COUNT(*) FROM posts").await?;
        let posts_today = self
            .scalar("SELECT COUNT(*) FROM posts WHERE fetched_at >= datetime('now', '-24 hours')")
            .await?;
        let posts_7d = self
            .scalar("SELECT COUNT(*) FROM posts WHERE fetched_at >= datetime('now', '-7 days')")
            .await?;
        let push_ok = self.scalar("SELECT COUNT(*) FROM push_logs WHERE status = 'success' AND created_at >= datetime('now', '-7 days')").await?;
        let push_total = self
            .scalar("SELECT COUNT(*) FROM push_logs WHERE created_at >= datetime('now', '-7 days')")
            .await?;
        let push_today = self
            .scalar(
                "SELECT COUNT(*) FROM push_logs WHERE created_at >= datetime('now', '-24 hours')",
            )
            .await?;
        let platforms = sqlx::query(
            "SELECT platform, COUNT(*) AS c FROM posts GROUP BY platform ORDER BY c DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut by_platform = serde_json::Map::new();
        for row in platforms {
            by_platform.insert(row.get("platform"), json!(row.get::<i64, _>("c")));
        }
        let channels = sqlx::query(
            "SELECT channel, COUNT(*) AS c, COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) AS ok
             FROM push_logs WHERE created_at >= datetime('now', '-7 days') GROUP BY channel ORDER BY c DESC",
        ).fetch_all(&self.pool).await?;
        let mut by_channel = serde_json::Map::new();
        for row in channels {
            by_channel.insert(
                row.get("channel"),
                json!({"total": row.get::<i64, _>("c"), "ok": row.get::<i64, _>("ok")}),
            );
        }
        let trend_rows = sqlx::query(
            "SELECT strftime('%Y-%m-%d', created_at, 'localtime') AS d, COUNT(*) AS c,
                    COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END), 0) AS ok
             FROM push_logs WHERE created_at >= datetime('now', '-13 days') GROUP BY d ORDER BY d",
        )
        .fetch_all(&self.pool)
        .await?;
        let trend: Vec<Value> = trend_rows
            .iter()
            .map(|row| {
                json!({
                    "date": row.get::<String, _>("d"),
                    "pushed": row.get::<i64, _>("c"),
                    "ok": row.get::<i64, _>("ok"),
                })
            })
            .collect();
        let rate = if push_total == 0 {
            Value::Null
        } else {
            json!(((push_ok as f64 / push_total as f64) * 1000.0).round() / 10.0)
        };
        let avg = if users_total == 0 {
            0.0
        } else {
            (subs as f64 / users_total as f64 * 10.0).round() / 10.0
        };
        Ok(json!({
            "users": {"total": users_total, "admins": admins, "bound": bound, "new_7d": new_7d},
            "subscriptions": {"total": subs, "favorite": favorite, "avg_per_user": avg},
            "posts": {"total": posts_total, "today": posts_today, "last_7d": posts_7d, "by_platform": by_platform},
            "pushes": {
                "total_7d": push_total,
                "ok_7d": push_ok,
                "fail_7d": push_total - push_ok,
                "success_rate": rate,
                "today": push_today,
                "by_channel": by_channel,
                "trend_14d": trend,
            },
            "sources_fail_24h": {},
        }))
    }

    async fn scalar(&self, sql: &str) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await
    }

    pub async fn add_admin_log(
        &self,
        user_id: i64,
        action: &str,
        target: &str,
        detail: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO admin_logs (user_id, action, target, detail) VALUES (?, ?, ?, ?)")
            .bind(user_id)
            .bind(action)
            .bind(target)
            .bind(detail)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_admin_logs(&self, limit: i64) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT l.id, l.user_id, l.action, l.target, l.detail, l.created_at, u.username
             FROM admin_logs l LEFT JOIN users u ON u.id = l.user_id
             ORDER BY l.id DESC LIMIT ?",
        )
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "user_id": row.get::<Option<i64>, _>("user_id"),
                    "action": row.get::<String, _>("action"),
                    "target": row.get::<String, _>("target"),
                    "detail": row.get::<String, _>("detail"),
                    "created_at": row.get::<String, _>("created_at"),
                    "username": row.get::<Option<String>, _>("username"),
                })
            })
            .collect())
    }

    pub async fn note_platform(
        &self,
        platform: &str,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let platform = platform.trim();
        if platform.is_empty() {
            return Ok(());
        }
        if let Some(error) = error {
            sqlx::query(
                "INSERT INTO platform_runs (platform, consecutive_failures, last_error) VALUES (?, 1, ?)
                 ON CONFLICT(platform) DO UPDATE SET consecutive_failures = consecutive_failures + 1, last_error = excluded.last_error",
            )
            .bind(platform)
            .bind(error)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO platform_runs (platform, consecutive_failures, last_error, last_success_at) VALUES (?, 0, '', datetime('now'))
                 ON CONFLICT(platform) DO UPDATE SET consecutive_failures = 0, last_error = '', last_success_at = datetime('now')",
            )
            .bind(platform)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn health_alerts(&self) -> Result<Vec<String>, sqlx::Error> {
        let today: String = sqlx::query_scalar("SELECT date('now')")
            .fetch_one(&self.pool)
            .await?;
        let mut alerts = Vec::new();
        let feeds = sqlx::query(
            "SELECT id, name, consecutive_failures, last_error_detail FROM news_feeds
             WHERE enabled = 1 AND archived_at IS NULL AND consecutive_failures >= 3",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in feeds {
            let id: i64 = row.get("id");
            let key = format!("health_alert:feed:{id}");
            if self.setting(&key).await?.as_deref() == Some(today.as_str()) {
                continue;
            }
            let name: String = row.get("name");
            let count: i64 = row.get("consecutive_failures");
            let detail: String = row.get("last_error_detail");
            let message = format!("资讯源「{name}」已连续失败 {count} 次：{detail}");
            self.add_admin_log(0, "platform_health", &key, &message)
                .await?;
            self.set_setting(&key, &today).await?;
            alerts.push(message);
        }
        let runs = sqlx::query(
            "SELECT platform, consecutive_failures, last_error FROM platform_runs WHERE consecutive_failures >= 3",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in runs {
            let platform: String = row.get("platform");
            let key = format!("health_alert:platform:{platform}");
            if self.setting(&key).await?.as_deref() == Some(today.as_str()) {
                continue;
            }
            let count: i64 = row.get("consecutive_failures");
            let detail: String = row.get("last_error");
            let message = format!("平台 {platform} 已连续失败 {count} 次：{detail}");
            self.add_admin_log(0, "platform_health", &key, &message)
                .await?;
            self.set_setting(&key, &today).await?;
            alerts.push(message);
        }
        Ok(alerts)
    }

    pub async fn take_daily_reports(&self) -> Result<Vec<(i64, String)>, sqlx::Error> {
        let hour = self
            .setting("config_daily_report_hour")
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(8)
            .clamp(0, 23);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if (((now + 8 * 3600) % 86400 / 3600) as i64) < hour {
            return Ok(Vec::new());
        }
        let today: String = sqlx::query_scalar("SELECT date('now', '+8 hours')")
            .fetch_one(&self.pool)
            .await?;
        let users: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE daily_report = 1 AND notify_enabled = 1 ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut reports = Vec::new();
        for user_id in users {
            let sent_key = format!("daily_report_sent:{user_id}");
            if self.setting(&sent_key).await?.as_deref() == Some(today.as_str()) {
                continue;
            }
            let cursor_key = format!("daily_report_cursor:{user_id}");
            let cursor = self
                .setting(&cursor_key)
                .await?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            let rows = if cursor > 0 {
                sqlx::query(
                    "SELECT p.id, k.name, p.title, p.content FROM posts p
                     JOIN kols k ON k.id = p.kol_id
                     JOIN subscriptions s ON s.user_id = ? AND s.kol_id = p.kol_id
                     WHERE p.id > ? ORDER BY p.id LIMIT 30",
                )
                .bind(user_id)
                .bind(cursor)
                .fetch_all(&self.pool)
                .await?
            } else {
                sqlx::query(
                    "SELECT p.id, k.name, p.title, p.content FROM posts p
                     JOIN kols k ON k.id = p.kol_id
                     JOIN subscriptions s ON s.user_id = ? AND s.kol_id = p.kol_id
                     WHERE p.fetched_at >= datetime('now', '-1 day') ORDER BY p.id LIMIT 30",
                )
                .bind(user_id)
                .fetch_all(&self.pool)
                .await?
            };
            if rows.is_empty() {
                continue;
            }
            let mut last = cursor;
            let mut lines = vec!["【每日精选】".to_string()];
            let mut current = String::new();
            for row in rows {
                let id: i64 = row.get("id");
                last = id;
                let name: String = row.get("name");
                if name != current {
                    lines.push(name.clone());
                    current = name;
                }
                let title: String = row.get("title");
                let content: String = row.get("content");
                let text = if title.trim().is_empty() {
                    content
                } else {
                    title
                };
                let text = text.chars().take(80).collect::<String>();
                lines.push(format!("- {text}"));
            }
            self.set_setting(&cursor_key, &last.to_string()).await?;
            self.set_setting(&sent_key, &today).await?;
            reports.push((user_id, lines.join("\n")));
        }
        Ok(reports)
    }

    pub async fn stamp_keyword_since(&self, user_id: i64, column: &str) -> Result<(), sqlx::Error> {
        let sql = match column {
            "keywords_match_news_since" => "UPDATE users SET keywords_match_news_since = datetime('now') WHERE id = ? AND keywords_match_news_since = ''",
            "keywords_match_reports_since" => "UPDATE users SET keywords_match_reports_since = datetime('now') WHERE id = ? AND keywords_match_reports_since = ''",
            _ => return Ok(()),
        };
        sqlx::query(sql).bind(user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn pending_keyword_digests(&self) -> Result<Vec<KeywordDigest>, sqlx::Error> {
        let mut digests = Vec::new();
        let news_users = sqlx::query(
            "SELECT id, keywords, dnd_start, dnd_end, keywords_match_news_since FROM users WHERE notify_enabled = 1 AND keywords_match_news = 1 ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in news_users {
            let user_id: i64 = row.get("id");
            if dnd_active(
                &row.get::<String, _>("dnd_start"),
                &row.get::<String, _>("dnd_end"),
            ) {
                continue;
            }
            let since: String = row.get("keywords_match_news_since");
            if since.trim().is_empty() {
                self.stamp_keyword_since(user_id, "keywords_match_news_since")
                    .await?;
                continue;
            }
            let articles = sqlx::query(
                "SELECT a.id, a.title, a.summary, a.author, s.name AS source_name FROM news_articles a
                 JOIN news_sources s ON s.id = a.source_id
                 WHERE s.archived_at IS NULL AND s.internal = 0
                 AND COALESCE(NULLIF(a.fetched_at, ''), a.published_at) >= ?
                 AND NOT EXISTS (SELECT 1 FROM news_keyword_notified n WHERE n.user_id = ? AND n.article_id = a.id)
                 ORDER BY a.id DESC LIMIT 400",
            )
            .bind(since.trim())
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
            let keywords: String = row.get("keywords");
            let mut matched = Vec::new();
            for article in articles {
                let text = format!(
                    "{}\n{}\n{}\n{}",
                    article.get::<String, _>("title"),
                    article.get::<String, _>("summary"),
                    article.get::<String, _>("author"),
                    article.get::<String, _>("source_name")
                );
                if keyword_hits(&keywords, &text).is_empty() {
                    continue;
                }
                matched.push(article);
            }
            if matched.is_empty() {
                continue;
            }
            let shown = matched.len().min(8);
            let extra = matched.len() - shown;
            let mut lines = vec![
                format!("财经资讯 {} 条命中关键词", matched.len()),
                String::new(),
            ];
            for article in matched.iter().take(shown) {
                let source: String = article.get("source_name");
                let suffix = if source.trim().is_empty() {
                    String::new()
                } else {
                    format!("（{}）", source.trim())
                };
                lines.push(format!(
                    "· {}{suffix}",
                    clip_title(&article.get::<String, _>("title"), "财经资讯")
                ));
            }
            if extra > 0 {
                lines.push(format!("· 还有 {extra} 条"));
            }
            lines.push(String::new());
            lines.push("打开财经资讯查看 https://vpush.net/news".into());
            digests.push(KeywordDigest {
                user_id,
                text: lines.join("\n"),
                articles: matched.iter().map(|row| row.get("id")).collect(),
                docs: Vec::new(),
            });
        }
        let report_users = sqlx::query(
            "SELECT id, is_admin, keywords, dnd_start, dnd_end, keywords_match_reports_since FROM users WHERE notify_enabled = 1 AND keywords_match_reports = 1 ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in report_users {
            let user_id: i64 = row.get("id");
            if dnd_active(
                &row.get::<String, _>("dnd_start"),
                &row.get::<String, _>("dnd_end"),
            ) {
                continue;
            }
            let since: String = row.get("keywords_match_reports_since");
            if since.trim().is_empty() {
                self.stamp_keyword_since(user_id, "keywords_match_reports_since")
                    .await?;
                continue;
            }
            let admin = row.get::<i64, _>("is_admin") != 0;
            let (acl, subscribed) = if admin {
                (HashSet::new(), HashSet::new())
            } else {
                self.ima_access(user_id).await?
            };
            let docs = sqlx::query(
                "SELECT d.group_id, d.media_id, d.name, d.abstract, d.group_name, COALESCE(t.abstract_zh, '') AS abstract_zh
                 FROM ima_document_index d
                 LEFT JOIN ima_abstract_translations t ON t.group_id = d.group_id AND t.media_id = d.media_id
                 WHERE d.downloaded_at >= ?
                 AND NOT EXISTS (SELECT 1 FROM knowledge_keyword_notified n WHERE n.user_id = ? AND n.group_id = d.group_id AND n.media_id = d.media_id)
                 ORDER BY d.downloaded_at DESC LIMIT 400",
            )
            .bind(since.trim())
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;
            let keywords: String = row.get("keywords");
            let mut matched = Vec::new();
            for doc in docs {
                let group: String = doc.get("group_id");
                if !(admin || acl.contains(&group) && subscribed.contains(&group)) {
                    continue;
                }
                let text = format!(
                    "{}\n{}\n{}\n{}",
                    doc.get::<String, _>("name"),
                    doc.get::<String, _>("abstract"),
                    doc.get::<String, _>("abstract_zh"),
                    doc.get::<String, _>("group_name")
                );
                if keyword_hits(&keywords, &text).is_empty() {
                    continue;
                }
                matched.push(doc);
            }
            if matched.is_empty() {
                continue;
            }
            let shown = matched.len().min(8);
            let extra = matched.len() - shown;
            let mut lines = vec![
                format!("今日研报 {} 篇命中关键词", matched.len()),
                String::new(),
            ];
            for doc in matched.iter().take(shown) {
                let source: String = doc.get("group_name");
                let suffix = if source.trim().is_empty() {
                    String::new()
                } else {
                    format!("（{}）", source.trim())
                };
                lines.push(format!(
                    "· {}{suffix}",
                    clip_title(&doc.get::<String, _>("name"), "研报")
                ));
            }
            if extra > 0 {
                lines.push(format!("· 还有 {extra} 篇"));
            }
            lines.push(String::new());
            lines.push("打开研报库查看 https://vpush.net/knowledge".into());
            digests.push(KeywordDigest {
                user_id,
                text: lines.join("\n"),
                articles: Vec::new(),
                docs: matched
                    .iter()
                    .map(|row| (row.get("group_id"), row.get("media_id")))
                    .collect(),
            });
        }
        Ok(digests)
    }

    pub async fn mark_keyword_digest(&self, digest: &KeywordDigest) -> Result<(), sqlx::Error> {
        for id in &digest.articles {
            sqlx::query(
                "INSERT OR IGNORE INTO news_keyword_notified (user_id, article_id) VALUES (?, ?)",
            )
            .bind(digest.user_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        for (group, media) in &digest.docs {
            sqlx::query("INSERT OR IGNORE INTO knowledge_keyword_notified (user_id, group_id, media_id) VALUES (?, ?, ?)").bind(digest.user_id).bind(group).bind(media).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn add_error_log(
        &self,
        level: &str,
        logger: &str,
        message: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO error_logs (level, logger, message) VALUES (?, ?, ?)")
            .bind(level.to_ascii_uppercase())
            .bind(logger)
            .bind(message)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn list_error_logs(
        &self,
        limit: i64,
        level: &str,
        q: &str,
    ) -> Result<Vec<Value>, CatalogError> {
        let level = level.trim().to_ascii_uppercase();
        let rank = match level.as_str() {
            "" => 0,
            "DEBUG" => 10,
            "INFO" => 20,
            "WARNING" => 30,
            "ERROR" => 40,
            "CRITICAL" => 50,
            _ => {
                return Err(CatalogError::Bad(
                    "level 需为 DEBUG/INFO/WARNING/ERROR/CRITICAL",
                ))
            }
        };
        let mut sql = sqlx::QueryBuilder::new(
            "SELECT id, level, logger, message, created_at FROM error_logs WHERE 1 = 1",
        );
        if rank > 0 {
            sql.push(" AND CASE level WHEN 'DEBUG' THEN 10 WHEN 'INFO' THEN 20 WHEN 'WARNING' THEN 30 WHEN 'ERROR' THEN 40 WHEN 'CRITICAL' THEN 50 ELSE 0 END >= ");
            sql.push_bind(rank);
        }
        if !q.trim().is_empty() {
            let like = like_pattern(q.trim());
            sql.push(" AND (logger LIKE ");
            sql.push_bind(like.clone());
            sql.push(" ESCAPE '!' OR message LIKE ");
            sql.push_bind(like);
            sql.push(" ESCAPE '!')");
        }
        sql.push(" ORDER BY id DESC LIMIT ");
        sql.push_bind(limit.clamp(10, 2000));
        let rows = sql.build().fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "level": row.get::<String, _>("level"),
                    "logger": row.get::<String, _>("logger"),
                    "message": row.get::<String, _>("message"),
                    "created_at": row.get::<String, _>("created_at"),
                })
            })
            .collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn add_kol(
        &self,
        platform: &str,
        name: &str,
        external_id: &str,
        category_id: Option<i64>,
        priority: bool,
        secondary: bool,
        original_only: bool,
    ) -> Result<i64, CatalogError> {
        if !PLATFORMS.contains(&platform) {
            return Err(CatalogError::Bad("不支持的平台"));
        }
        let external_id = normalize_external_id(platform, external_id);
        let name = name.trim();
        if external_id.is_empty() || name.is_empty() {
            return Err(CatalogError::Bad("名称和外部 ID 不能为空"));
        }
        if let Some(id) = category_id {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
            if found.is_none() {
                return Err(CatalogError::Bad("分类不存在"));
            }
        }
        match sqlx::query(
            "INSERT INTO kols
                (platform, name, external_id, category_id, priority, secondary, original_only)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(platform)
        .bind(name)
        .bind(&external_id)
        .bind(category_id)
        .bind(i64::from(priority))
        .bind(i64::from(secondary))
        .bind(i64::from(original_only))
        .execute(&self.pool)
        .await
        {
            Ok(res) => Ok(res.last_insert_rowid()),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                Err(CatalogError::Bad("该大V已在目录中"))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn delete_kol(&self, id: i64) -> Result<(), CatalogError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM posts WHERE kol_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM subscriptions WHERE kol_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM kol_acl WHERE kol_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let gone = sqlx::query("DELETE FROM kols WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if gone.rows_affected() == 0 {
            return Err(CatalogError::Missing("大V不存在"));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn patch_kol(&self, id: i64, patch: KolPatch<'_>) -> Result<(), CatalogError> {
        let row = sqlx::query("SELECT platform, name, external_id, enabled, category_id, priority, secondary, is_private, original_only, recommend_weight FROM kols WHERE id = ?")
            .bind(id).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Err(CatalogError::Missing("大V不存在"));
        };
        let platform: String = row.get("platform");
        let current_name: String = row.get("name");
        let name = patch.name.unwrap_or(current_name.as_str()).trim();
        if name.is_empty() {
            return Err(CatalogError::Bad("昵称与外部ID不能为空"));
        }
        let external_id = match patch.external_id {
            Some(value) => normalize_external_id(&platform, value),
            None => row.get("external_id"),
        };
        if external_id.is_empty() {
            return Err(CatalogError::Bad("昵称与外部ID不能为空"));
        }
        if patch.external_id.is_some() {
            let dup: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM kols WHERE platform = ? AND external_id = ? AND id != ?",
            )
            .bind(&platform)
            .bind(&external_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
            if dup.is_some() {
                return Err(CatalogError::Bad("该平台已存在相同的外部ID"));
            }
        }
        let category_id = match patch.category_id {
            Some(value) => value,
            None => row.get("category_id"),
        };
        if let Some(category_id) = category_id {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                .bind(category_id)
                .fetch_optional(&self.pool)
                .await?;
            if found.is_none() {
                return Err(CatalogError::Bad("分类不存在"));
            }
        }
        let mut priority = patch.priority.unwrap_or(row.get::<i64, _>("priority") != 0);
        let mut secondary = patch
            .secondary
            .unwrap_or(row.get::<i64, _>("secondary") != 0);
        if patch.secondary == Some(true) {
            priority = false;
        } else if patch.priority == Some(true) {
            secondary = false;
        }
        let enabled = patch.enabled.unwrap_or(row.get::<i64, _>("enabled") != 0);
        let is_private = patch
            .is_private
            .unwrap_or(row.get::<i64, _>("is_private") != 0);
        let original_only = patch
            .original_only
            .unwrap_or(row.get::<i64, _>("original_only") != 0);
        let weight = patch
            .recommend_weight
            .unwrap_or(row.get("recommend_weight"))
            .max(0);
        let mut user_ids = Vec::new();
        if let Some(names) = patch.visible_users {
            for name in names {
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                match self.user_by_username(name).await? {
                    Some(user) => user_ids.push(user.id),
                    None => return Err(CatalogError::Invalid(format!("用户不存在: {name}"))),
                }
            }
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE kols SET name = ?, external_id = ?, enabled = ?, category_id = ?, priority = ?, secondary = ?, is_private = ?, original_only = ?, recommend_weight = ? WHERE id = ?")
            .bind(name).bind(&external_id).bind(i64::from(enabled)).bind(category_id).bind(i64::from(priority)).bind(i64::from(secondary)).bind(i64::from(is_private)).bind(i64::from(original_only)).bind(weight).bind(id)
            .execute(&mut *tx).await?;
        if patch.visible_users.is_some() {
            sqlx::query("DELETE FROM kol_acl WHERE kol_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            for user_id in user_ids {
                sqlx::query("INSERT OR IGNORE INTO kol_acl (kol_id, user_id) VALUES (?, ?)")
                    .bind(id)
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn kol_acl_usernames(&self, kol_id: i64) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT u.username FROM kol_acl a JOIN users u ON u.id = a.user_id WHERE a.kol_id = ? ORDER BY u.username")
            .bind(kol_id).fetch_all(&self.pool).await
    }

    pub async fn batch_kols(
        &self,
        ids: &[i64],
        action: &str,
        value: Option<&Value>,
    ) -> Result<i64, CatalogError> {
        if ids.is_empty() {
            return Err(CatalogError::Bad("请先选择大V"));
        }
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() > 500 {
            return Err(CatalogError::Bad("一次最多操作 500 个大V"));
        }
        let mut tx = self.pool.begin().await?;
        let marks = vec!["?"; ids.len()].join(",");
        let found: i64 = sqlx::query(&format!(
            "SELECT COUNT(*) AS n FROM kols WHERE id IN ({marks})"
        ))
        .bind_ids(&ids)
        .fetch_one(&mut *tx)
        .await?
        .get("n");
        if found != ids.len() as i64 {
            return Err(CatalogError::Missing("大V不存在"));
        }
        match action {
            "enable" | "disable" => {
                sqlx::query(&format!(
                    "UPDATE kols SET enabled = ? WHERE id IN ({marks})"
                ))
                .bind(i64::from(action == "enable"))
                .bind_ids(&ids)
                .execute(&mut *tx)
                .await?;
            }
            "priority" | "secondary" => {
                let on = value
                    .and_then(Value::as_bool)
                    .ok_or(CatalogError::Bad("缺少开关值"))?;
                let (col, other) = if action == "priority" {
                    ("priority", "secondary")
                } else {
                    ("secondary", "priority")
                };
                let sql = if on {
                    format!("UPDATE kols SET {col} = 1, {other} = 0 WHERE id IN ({marks})")
                } else {
                    format!("UPDATE kols SET {col} = 0 WHERE id IN ({marks})")
                };
                sqlx::query(&sql).bind_ids(&ids).execute(&mut *tx).await?;
            }
            "normal" => {
                sqlx::query(&format!(
                    "UPDATE kols SET priority = 0, secondary = 0 WHERE id IN ({marks})"
                ))
                .bind_ids(&ids)
                .execute(&mut *tx)
                .await?;
            }
            "category" => {
                let category_id = match value {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(v.as_i64().ok_or(CatalogError::Bad("分类不正确"))?),
                };
                if let Some(id) = category_id {
                    let exists: Option<i64> =
                        sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                            .bind(id)
                            .fetch_optional(&mut *tx)
                            .await?;
                    if exists.is_none() {
                        return Err(CatalogError::Bad("分类不存在"));
                    }
                }
                sqlx::query(&format!(
                    "UPDATE kols SET category_id = ? WHERE id IN ({marks})"
                ))
                .bind(category_id)
                .bind_ids(&ids)
                .execute(&mut *tx)
                .await?;
            }
            "delete" => {
                for id in &ids {
                    sqlx::query("DELETE FROM posts WHERE kol_id = ?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("DELETE FROM subscriptions WHERE kol_id = ?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("DELETE FROM kol_acl WHERE kol_id = ?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    sqlx::query("DELETE FROM kols WHERE id = ?")
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                }
            }
            _ => return Err(CatalogError::Bad("不支持的操作")),
        }
        tx.commit().await?;
        Ok(ids.len() as i64)
    }

    pub async fn batch_add_kols(
        &self,
        lines: &str,
        category_id: Option<i64>,
    ) -> Result<Value, CatalogError> {
        if let Some(id) = category_id {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
            if found.is_none() {
                return Err(CatalogError::Bad("分类不存在"));
            }
        }
        let mut results = Vec::new();
        for raw in lines.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let (platform, external_id, nickname) = match parse_batch_kol_line(line) {
                Ok(parsed) => parsed,
                Err(err) => {
                    results.push(json!({"ok": false, "line": clip_line(line), "error": err}));
                    continue;
                }
            };
            let name = if nickname.is_empty() {
                format!("{platform}_{external_id}")
            } else {
                nickname
            };
            match self
                .add_kol(
                    &platform,
                    &name,
                    &external_id,
                    category_id,
                    false,
                    false,
                    false,
                )
                .await
            {
                Ok(id) => results
                    .push(json!({"ok": true, "id": id, "name": name, "external_id": external_id})),
                Err(CatalogError::Bad(msg)) => {
                    results.push(json!({"ok": false, "line": clip_line(line), "error": msg}))
                }
                Err(CatalogError::Invalid(msg)) => {
                    results.push(json!({"ok": false, "line": clip_line(line), "error": msg}))
                }
                Err(err) => return Err(err),
            }
        }
        let ids: Vec<i64> = results
            .iter()
            .filter_map(|row| row.get("id").and_then(Value::as_i64))
            .collect();
        let ok = ids.len();
        Ok(json!({
            "total": results.len(),
            "ok": ok,
            "ids": ids,
            "failed": results.into_iter().filter(|row| row.get("ok") != Some(&Value::Bool(true))).collect::<Vec<_>>(),
        }))
    }

    pub async fn catalog_page(
        &self,
        user_id: i64,
        is_admin: bool,
        page_size: usize,
        offset: i64,
    ) -> Result<(i64, Vec<Value>), sqlx::Error> {
        let limit = page_size.clamp(1, 20) as i64;
        let offset = offset.max(0);
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM kols k WHERE k.enabled = 1 AND {VISIBLE}"
        ))
        .bind(i64::from(is_admin))
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(&format!(
            "{KOL_SELECT} WHERE k.enabled = 1 AND {VISIBLE} ORDER BY subscribed DESC, k.priority DESC, last_post_at DESC, k.id DESC LIMIT ? OFFSET ?"
        ))
        .bind(user_id)
        .bind(i64::from(is_admin))
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let mut items: Vec<Value> = rows.iter().map(kol_json).collect();
        self.attach_quotes(&mut items).await?;
        Ok((total, items))
    }

    pub async fn catalog(
        &self,
        user_id: i64,
        is_admin: bool,
        platform: &str,
        category_id: i64,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(&format!("{KOL_SELECT} WHERE k.enabled = 1 AND {VISIBLE} AND (? = '' OR k.platform = ?) AND (? = 0 OR k.category_id = ?) ORDER BY subscribed DESC, k.priority DESC, last_post_at DESC, k.id DESC"))
            .bind(user_id)
            .bind(i64::from(is_admin))
            .bind(user_id)
            .bind(platform)
            .bind(platform)
            .bind(category_id)
            .bind(category_id)
            .fetch_all(&self.pool)
            .await?;
        let mut out: Vec<Value> = rows.iter().map(kol_json).collect();
        self.attach_quotes(&mut out).await?;
        Ok(out)
    }

    pub async fn admin_kol_page(
        &self,
        platform: &str,
        category_id: i64,
        q: &str,
        status: Option<i64>,
        limit: i64,
        offset: i64,
    ) -> Result<Value, sqlx::Error> {
        let like = like_contains(q);
        let status_flag = status.unwrap_or(-1);
        let filter = "(? = '' OR k.platform = ?) AND (? = 0 OR k.category_id = ?) AND (? = '' OR k.name LIKE ? ESCAPE '\\' OR k.external_id LIKE ? ESCAPE '\\') AND (? = -1 OR k.enabled = ?)";
        let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM kols k WHERE {filter}"))
            .bind(platform)
            .bind(platform)
            .bind(category_id)
            .bind(category_id)
            .bind(q)
            .bind(&like)
            .bind(&like)
            .bind(status_flag)
            .bind(status_flag)
            .fetch_one(&self.pool)
            .await?;
        let id_rows = sqlx::query(&format!(
            "SELECT k.id FROM kols k WHERE {filter} ORDER BY k.id"
        ))
        .bind(platform)
        .bind(platform)
        .bind(category_id)
        .bind(category_id)
        .bind(q)
        .bind(&like)
        .bind(&like)
        .bind(status_flag)
        .bind(status_flag)
        .fetch_all(&self.pool)
        .await?;
        let ids: Vec<i64> = id_rows.iter().map(|row| row.get("id")).collect();
        let rows = sqlx::query(&format!(
            "SELECT k.id, k.platform, k.name, k.external_id, k.avatar_url, k.enabled, k.is_private, k.original_only, k.priority, k.secondary, k.category_id, COALESCE(c.name, '') AS category_name, COALESCE(sc.n, 0) AS subscriber_count FROM kols k LEFT JOIN categories c ON c.id = k.category_id LEFT JOIN (SELECT kol_id, COUNT(*) AS n FROM subscriptions GROUP BY kol_id) sc ON sc.kol_id = k.id WHERE {filter} ORDER BY k.id LIMIT ? OFFSET ?"
        ))
        .bind(platform).bind(platform).bind(category_id).bind(category_id).bind(q).bind(&like).bind(&like).bind(status_flag).bind(status_flag).bind(limit).bind(offset)
        .fetch_all(&self.pool).await?;
        let items: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "platform": row.get::<String, _>("platform"),
                    "name": row.get::<String, _>("name"),
                    "external_id": row.get::<String, _>("external_id"),
                    "avatar_url": row.get::<String, _>("avatar_url"),
                    "enabled": row.get::<i64, _>("enabled") != 0,
                    "is_private": row.get::<i64, _>("is_private") != 0,
                    "original_only": row.get::<i64, _>("original_only") != 0,
                    "priority": row.get::<i64, _>("priority") != 0,
                    "secondary": row.get::<i64, _>("secondary") != 0,
                    "category_id": row.get::<Option<i64>, _>("category_id"),
                    "category_name": row.get::<String, _>("category_name"),
                    "subscriber_count": row.get::<i64, _>("subscriber_count"),
                })
            })
            .collect();
        Ok(json!({"total": total, "items": items, "ids": ids}))
    }

    pub async fn kol_for(
        &self,
        user_id: i64,
        is_admin: bool,
        kol_id: i64,
    ) -> Result<Option<Value>, sqlx::Error> {
        let row = sqlx::query(&format!(
            "{KOL_SELECT} WHERE k.id = ? AND (? = 1 OR k.enabled = 1) AND {VISIBLE}"
        ))
        .bind(user_id)
        .bind(kol_id)
        .bind(i64::from(is_admin))
        .bind(i64::from(is_admin))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut value = kol_json(&row);
        if is_admin {
            value["visible_users"] = json!(self.kol_acl_usernames(kol_id).await?);
        }
        self.attach_quotes(std::slice::from_mut(&mut value)).await?;
        Ok(Some(value))
    }

    pub async fn recommendations(
        &self,
        user_id: i64,
        is_admin: bool,
        unsubscribed: bool,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(&format!(
            "SELECT k.id, k.name, k.platform, k.avatar_url, COALESCE(c.name, '') AS category_name,
                    (SELECT COUNT(*) FROM subscriptions x WHERE x.kol_id = k.id) AS subscriber_count,
                    CASE WHEN s.user_id IS NULL THEN 0 ELSE 1 END AS subscribed
             FROM kols k
             LEFT JOIN categories c ON c.id = k.category_id
             LEFT JOIN subscriptions s ON s.kol_id = k.id AND s.user_id = ?
             WHERE k.enabled = 1 AND {VISIBLE}
             ORDER BY subscriber_count DESC, k.priority DESC, k.id DESC
             LIMIT 16"
        ))
        .bind(user_id)
        .bind(i64::from(is_admin))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let subscribed = row.get::<i64, _>("subscribed") != 0;
            if unsubscribed && subscribed {
                continue;
            }
            out.push(json!({
                "id": row.get::<i64, _>("id"),
                "name": row.get::<String, _>("name"),
                "platform": row.get::<String, _>("platform"),
                "avatar_url": row.get::<String, _>("avatar_url"),
                "category_name": row.get::<String, _>("category_name"),
                "subscriber_count": row.get::<i64, _>("subscriber_count"),
                "subscribed": subscribed,
            }));
            if out.len() == 4 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn subscribe(
        &self,
        user_id: i64,
        is_admin: bool,
        kol_id: i64,
        kind: &str,
    ) -> Result<(), CatalogError> {
        if !matches!(kind, "post" | "reply" | "both") {
            return Err(CatalogError::Bad("订阅类型需为 post / reply / both"));
        }
        self.visible_enabled(user_id, is_admin, kol_id).await?;
        sqlx::query(
            "INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, ?)
             ON CONFLICT(user_id, kol_id) DO UPDATE SET type = excluded.type",
        )
        .bind(user_id)
        .bind(kol_id)
        .bind(kind)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn unsubscribe(&self, user_id: i64, kol_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM subscriptions WHERE user_id = ? AND kol_id = ?")
            .bind(user_id)
            .bind(kol_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_subscription_type(
        &self,
        user_id: i64,
        kol_id: i64,
        kind: &str,
    ) -> Result<(), CatalogError> {
        if !matches!(kind, "post" | "reply" | "both") {
            return Err(CatalogError::Bad("订阅类型需为 post / reply / both"));
        }
        self.touch_sub(
            "UPDATE subscriptions SET type = ? WHERE user_id = ? AND kol_id = ?",
            user_id,
            kol_id,
            kind,
        )
        .await
    }

    pub async fn set_favorite(
        &self,
        user_id: i64,
        kol_id: i64,
        on: bool,
    ) -> Result<(), CatalogError> {
        self.touch_flag(user_id, kol_id, "favorite", on).await
    }

    pub async fn set_secondary(
        &self,
        user_id: i64,
        kol_id: i64,
        on: bool,
    ) -> Result<(), CatalogError> {
        self.touch_flag(user_id, kol_id, "secondary", on).await
    }

    pub async fn set_hide_images(
        &self,
        user_id: i64,
        kol_id: i64,
        on: bool,
    ) -> Result<(), CatalogError> {
        self.touch_flag(user_id, kol_id, "hide_images", on).await
    }

    pub async fn visible_my_subscriptions(
        &self,
        user_id: i64,
        is_admin: bool,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(&format!(
            "SELECT k.id, k.platform, k.name, k.external_id, k.avatar_url,
                    s.type AS subscribe_type, s.favorite, s.secondary, s.hide_images
             FROM subscriptions s
             JOIN kols k ON k.id = s.kol_id
             WHERE s.user_id = ? AND k.enabled = 1 AND {VISIBLE}
             ORDER BY k.name, k.id"
        ))
        .bind(user_id)
        .bind(i64::from(is_admin))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| subscription_json(&row))
            .collect())
    }

    pub async fn my_subscriptions(&self, user_id: i64) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT k.id, k.platform, k.name, k.external_id, k.avatar_url,
                    s.type AS subscribe_type, s.favorite, s.secondary, s.hide_images
             FROM subscriptions s
             JOIN kols k ON k.id = s.kol_id
             WHERE s.user_id = ?
             ORDER BY k.name, k.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| subscription_json(&row))
            .collect())
    }

    async fn touch_flag(
        &self,
        user_id: i64,
        kol_id: i64,
        column: &str,
        on: bool,
    ) -> Result<(), CatalogError> {
        let sql = match column {
            "favorite" => "UPDATE subscriptions SET favorite = ? WHERE user_id = ? AND kol_id = ?",
            "secondary" => {
                "UPDATE subscriptions SET secondary = ? WHERE user_id = ? AND kol_id = ?"
            }
            "hide_images" => {
                "UPDATE subscriptions SET hide_images = ? WHERE user_id = ? AND kol_id = ?"
            }
            _ => return Err(CatalogError::Bad("未知字段")),
        };
        let n = sqlx::query(sql)
            .bind(i64::from(on))
            .bind(user_id)
            .bind(kol_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if n == 0 {
            self.missing_sub(kol_id).await?;
        }
        Ok(())
    }

    async fn touch_sub(
        &self,
        sql: &str,
        user_id: i64,
        kol_id: i64,
        kind: &str,
    ) -> Result<(), CatalogError> {
        let n = sqlx::query(sql)
            .bind(kind)
            .bind(user_id)
            .bind(kol_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if n == 0 {
            self.missing_sub(kol_id).await?;
        }
        Ok(())
    }

    async fn missing_sub(&self, kol_id: i64) -> Result<(), CatalogError> {
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM kols WHERE id = ?")
            .bind(kol_id)
            .fetch_optional(&self.pool)
            .await?;
        Err(if exists.is_some() {
            CatalogError::Missing("尚未订阅该大V")
        } else {
            CatalogError::Missing("大V不存在")
        })
    }

    async fn visible_enabled(
        &self,
        user_id: i64,
        is_admin: bool,
        kol_id: i64,
    ) -> Result<(), CatalogError> {
        let row = sqlx::query(&format!(
            "SELECT k.enabled FROM kols k WHERE k.id = ? AND k.enabled = 1 AND {VISIBLE}"
        ))
        .bind(kol_id)
        .bind(i64::from(is_admin))
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;
        if row.is_none() {
            return Err(CatalogError::Missing("大V不存在"));
        }
        Ok(())
    }

    pub async fn feed(
        &self,
        user_id: i64,
        is_admin: bool,
        f: &FeedFilter<'_>,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let like = like_pattern(f.q);
        let tag = tag_pattern(f.tag);
        let rows = sqlx::query(
            "SELECT p.id, p.platform, p.kol_id, p.external_id, p.title, p.content,
                    p.title_src, p.content_src, p.post_type, p.images, p.tags, p.url, p.detail,
                    p.published_at, p.fetched_at,
                    k.name AS kol_name, k.avatar_url, k.external_id AS kol_external_id,
                    k.category_id, COALESCE(c.name, '') AS category_name,
                    COALESCE(s.favorite, 0) AS favorite, COALESCE(s.hide_images, 0) AS hide_images
             FROM posts p
             JOIN kols k ON k.id = p.kol_id
             JOIN subscriptions s ON s.kol_id = p.kol_id AND s.user_id = ?
             LEFT JOIN categories c ON c.id = k.category_id
             WHERE k.enabled = 1 AND (? = 1 OR k.is_private = 0 OR EXISTS (
                 SELECT 1 FROM kol_acl a WHERE a.kol_id = k.id AND a.user_id = ?))
               AND (? = '' OR p.platform = ?)
               AND (? = 0 OR k.category_id = ?)
               AND (? = '' OR p.title LIKE ? ESCAPE '!' OR p.content LIKE ? ESCAPE '!')
               AND (? = '' OR p.tags LIKE ? ESCAPE '!')
               AND (? = 0 OR s.favorite = 1)
               AND (? = 1 OR ? != '' OR s.favorite = 1 OR (k.secondary = 0 AND s.secondary = 0))
               AND (? = 0 OR (p.id > ? AND p.published_at >= substr(datetime('now', '+8 hours', '-48 hours'), 1, 16)))
             ORDER BY p.published_at DESC, p.id DESC
             LIMIT ? OFFSET ?",
        )
        .bind(user_id)
        .bind(i64::from(is_admin))
        .bind(user_id)
        .bind(f.platform)
        .bind(f.platform)
        .bind(f.category_id)
        .bind(f.category_id)
        .bind(f.q)
        .bind(&like)
        .bind(&like)
        .bind(f.tag)
        .bind(&tag)
        .bind(i64::from(f.favorite))
        .bind(i64::from(f.include_secondary))
        .bind(f.platform)
        .bind(i64::from(f.since_id > 0))
        .bind(f.since_id)
        .bind(f.limit)
        .bind(f.offset)
        .fetch_all(&self.pool)
        .await?;
        let mut posts: Vec<Value> = rows.iter().map(post_json).collect();
        crate::imgbed::rewrite_posts(self, &mut posts).await?;
        Ok(posts)
    }

    pub async fn kol_posts(
        &self,
        user_id: i64,
        is_admin: bool,
        kol_id: i64,
        limit: i64,
    ) -> Result<Option<Vec<Value>>, sqlx::Error> {
        if self.kol_for(user_id, is_admin, kol_id).await?.is_none() {
            return Ok(None);
        }
        let rows = sqlx::query(
            "SELECT p.id, p.platform, p.kol_id, p.external_id, p.title, p.content,
                    p.title_src, p.content_src, p.post_type, p.images, p.tags, p.url, p.detail,
                    p.published_at, p.fetched_at,
                    k.name AS kol_name, k.avatar_url, k.external_id AS kol_external_id,
                    k.category_id, COALESCE(c.name, '') AS category_name,
                    0 AS favorite, 0 AS hide_images
             FROM posts p
             JOIN kols k ON k.id = p.kol_id
             LEFT JOIN categories c ON c.id = k.category_id
             WHERE p.kol_id = ?
             ORDER BY p.published_at DESC, p.id DESC
             LIMIT ?",
        )
        .bind(kol_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let mut posts: Vec<Value> = rows.iter().map(post_json).collect();
        crate::imgbed::rewrite_posts(self, &mut posts).await?;
        Ok(Some(posts))
    }

    pub async fn dynamic_tags(&self) -> Result<Vec<String>, sqlx::Error> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT tags FROM posts WHERE tags NOT IN ('', '[]') ORDER BY id DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for raw in rows {
            let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) else {
                continue;
            };
            for tag in list {
                let tag = tag.trim();
                if tag.is_empty() {
                    continue;
                }
                *counts.entry(tag.to_string()).or_default() += 1;
            }
        }
        let mut tags: Vec<_> = counts.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        tags.truncate(40);
        Ok(tags.into_iter().map(|(tag, _)| tag).collect())
    }

    pub async fn insert_post(
        &self,
        kol_id: i64,
        external_id: &str,
        content: &str,
        published_at: &str,
        tags_json: &str,
    ) -> Result<i64, sqlx::Error> {
        let platform: String = sqlx::query_scalar("SELECT platform FROM kols WHERE id = ?")
            .bind(kol_id)
            .fetch_one(&self.pool)
            .await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO posts (platform, kol_id, external_id, content, tags, published_at)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(platform, external_id) DO UPDATE SET
                content = excluded.content,
                tags = excluded.tags,
                published_at = excluded.published_at
             RETURNING id",
        )
        .bind(platform)
        .bind(kol_id)
        .bind(external_id)
        .bind(content)
        .bind(tags_json)
        .bind(published_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn kols_to_fetch(
        &self,
        platform: &str,
    ) -> Result<Vec<(i64, String, String)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, name, external_id FROM kols
             WHERE enabled = 1 AND platform = ?
               AND EXISTS (SELECT 1 FROM subscriptions s WHERE s.kol_id = kols.id)
             ORDER BY priority DESC, id",
        )
        .bind(platform)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get("id"), row.get("name"), row.get("external_id")))
            .collect())
    }

    pub async fn weibo_kols(&self) -> Result<Vec<(i64, String, String, bool)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, name, external_id, original_only FROM kols
             WHERE enabled = 1 AND platform = 'weibo'
               AND EXISTS (SELECT 1 FROM subscriptions s WHERE s.kol_id = kols.id)
             ORDER BY priority DESC, id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.get("id"),
                    row.get("name"),
                    row.get("external_id"),
                    row.get::<i64, _>("original_only") != 0,
                )
            })
            .collect())
    }

    pub async fn has_post(&self, platform: &str, external_id: &str) -> Result<bool, sqlx::Error> {
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM posts WHERE platform = ? AND external_id = ? LIMIT 1",
        )
        .bind(platform)
        .bind(external_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_fetched(
        &self,
        kol_id: i64,
        external_id: &str,
        title: &str,
        content: &str,
        post_type: &str,
        images_json: &str,
        url: &str,
        published_at: &str,
    ) -> Result<(), sqlx::Error> {
        self.save_fetched_src(
            kol_id,
            external_id,
            title,
            content,
            "",
            "",
            post_type,
            images_json,
            url,
            published_at,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_fetched_src(
        &self,
        kol_id: i64,
        external_id: &str,
        title: &str,
        content: &str,
        title_src: &str,
        content_src: &str,
        post_type: &str,
        images_json: &str,
        url: &str,
        published_at: &str,
    ) -> Result<(), sqlx::Error> {
        let title = crate::zh_simp::to_simplified(title);
        let content = crate::zh_simp::to_simplified(content);
        let platform: String = sqlx::query_scalar("SELECT platform FROM kols WHERE id = ?")
            .bind(kol_id)
            .fetch_one(&self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO posts
                (platform, kol_id, external_id, title, content, title_src, content_src, post_type, images, url, published_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(platform, external_id) DO UPDATE SET
                title = excluded.title,
                content = excluded.content,
                title_src = CASE WHEN excluded.title_src != '' THEN excluded.title_src ELSE title_src END,
                content_src = CASE WHEN excluded.content_src != '' THEN excluded.content_src ELSE content_src END,
                post_type = excluded.post_type,
                images = excluded.images,
                url = excluded.url,
                published_at = excluded.published_at",
        )
        .bind(&platform)
        .bind(kol_id)
        .bind(external_id)
        .bind(&title)
        .bind(&content)
        .bind(title_src)
        .bind(content_src)
        .bind(post_type)
        .bind(images_json)
        .bind(url)
        .bind(published_at)
        .execute(&self.pool)
        .await?;
        crate::imgbed::enqueue_images(self, images_json).await?;
        Ok(())
    }

    pub async fn set_platform_detail(
        &self,
        platform: &str,
        external_id: &str,
        detail: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET detail = ? WHERE platform = ? AND external_id = ?")
            .bind(detail)
            .bind(platform)
            .bind(external_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn replace_post_detail(&self, id: i64, detail: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE posts SET detail = ? WHERE id = ?")
            .bind(detail)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn sqlite_path(&self) -> Result<String, sqlx::Error> {
        let path: Option<String> =
            sqlx::query_scalar("SELECT file FROM pragma_database_list WHERE name = 'main'")
                .fetch_one(&self.pool)
                .await?;
        Ok(path.unwrap_or_default())
    }

    pub async fn zsxq_file_posts(
        &self,
        file_id: &str,
    ) -> Result<Vec<(i64, i64, String)>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT id, kol_id, detail FROM posts WHERE platform = 'zsxq' AND (detail LIKE ? OR detail LIKE ? OR detail LIKE ?)",
        )
        .bind(format!("%\"file_id\": \"{file_id}\"%"))
        .bind(format!("%\"file_id\":\"{file_id}\"%"))
        .bind(format!("%\"file_id\": {file_id}%"))
        .fetch_all(&self.pool)
        .await?;
        let mut hits = Vec::new();
        for row in rows {
            let detail: String = row.get("detail");
            let Ok(parsed) = serde_json::from_str::<Value>(&detail) else {
                continue;
            };
            let Some(files) = parsed.get("files").and_then(Value::as_array) else {
                continue;
            };
            let matched = files.iter().any(|file| {
                let id = match &file["file_id"] {
                    Value::String(text) => text.as_str(),
                    Value::Number(number) => return number.to_string() == file_id,
                    _ => return false,
                };
                id == file_id
            });
            if matched {
                hits.push((row.get("id"), row.get("kol_id"), detail));
            }
        }
        Ok(hits)
    }

    pub async fn is_subscribed(&self, user_id: i64, kol_id: i64) -> Result<bool, sqlx::Error> {
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM subscriptions WHERE user_id = ? AND kol_id = ? LIMIT 1",
        )
        .bind(user_id)
        .bind(kol_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn save_combo(
        &self,
        kol_id: i64,
        external_id: &str,
        title: &str,
        content: &str,
        url: &str,
        published_at: &str,
        detail: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO posts
                (platform, kol_id, external_id, title, content, url, published_at, detail)
             VALUES ('combination', ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(platform, external_id) DO UPDATE SET
                title = excluded.title,
                content = excluded.content,
                url = excluded.url,
                published_at = excluded.published_at,
                detail = excluded.detail",
        )
        .bind(kol_id)
        .bind(external_id)
        .bind(title)
        .bind(content)
        .bind(url)
        .bind(published_at)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn max_external_num(&self, kol_id: i64) -> Result<i64, sqlx::Error> {
        let value: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(CAST(external_id AS INTEGER)) FROM posts
             WHERE kol_id = ? AND external_id GLOB '[0-9]*'",
        )
        .bind(kol_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(value.unwrap_or(0))
    }

    pub async fn max_published_at(&self, kol_id: i64) -> Result<String, sqlx::Error> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT MAX(published_at) FROM posts WHERE kol_id = ? AND published_at != ''",
        )
        .bind(kol_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(value.unwrap_or_default())
    }

    pub async fn cube_snapshot(
        &self,
        kol_id: i64,
        kind: &str,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT payload, fetched_at FROM cube_snapshots WHERE kol_id = ? AND kind = ?",
        )
        .bind(kol_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| (row.get("payload"), row.get("fetched_at"))))
    }

    pub async fn cube_fresh(
        &self,
        kol_id: i64,
        kind: &str,
        ttl_secs: i64,
    ) -> Result<bool, sqlx::Error> {
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM cube_snapshots
             WHERE kol_id = ? AND kind = ? AND fetched_at >= datetime('now', ?)",
        )
        .bind(kol_id)
        .bind(kind)
        .bind(format!("-{ttl_secs} seconds"))
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.is_some())
    }

    pub async fn set_cube_snapshot(
        &self,
        kol_id: i64,
        kind: &str,
        payload: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO cube_snapshots (kol_id, kind, payload, fetched_at)
             VALUES (?, ?, ?, datetime('now'))
             ON CONFLICT(kol_id, kind) DO UPDATE SET
                payload = excluded.payload,
                fetched_at = excluded.fetched_at",
        )
        .bind(kol_id)
        .bind(kind)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn attach_quotes(&self, rows: &mut [Value]) -> Result<(), sqlx::Error> {
        if !rows.iter().any(|row| row["platform"] == "combination") {
            return Ok(());
        }
        let snaps = sqlx::query(
            "SELECT kol_id, payload, fetched_at FROM cube_snapshots WHERE kind = 'quote'",
        )
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            if row["platform"] != "combination" {
                continue;
            }
            let id = row["id"].as_i64().unwrap_or(0);
            let found = snaps.iter().find(|snap| snap.get::<i64, _>("kol_id") == id);
            if let Some(snap) = found {
                let payload: String = snap.get("payload");
                row["quote"] = serde_json::from_str(&payload).unwrap_or(Value::Null);
                row["quote_at"] = Value::String(snap.get("fetched_at"));
            } else {
                row["quote"] = Value::Null;
                row["quote_at"] = Value::String(String::new());
            }
        }
        Ok(())
    }

    pub async fn should_push(&self, kol_id: i64, post_type: &str) -> Result<bool, sqlx::Error> {
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM subscriptions s
             JOIN kols k ON k.id = s.kol_id
             WHERE s.kol_id = ?
               AND (s.type = 'all' OR s.type = ?)
               AND (s.favorite = 1 OR (k.secondary = 0 AND s.secondary = 0))
             LIMIT 1",
        )
        .bind(kol_id)
        .bind(post_type)
        .fetch_optional(&self.pool)
        .await?;
        Ok(hit.is_some())
    }

    pub async fn set_avatar(&self, kol_id: i64, url: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE kols SET avatar_url = ? WHERE id = ? AND avatar_url != ?")
            .bind(url)
            .bind(kol_id)
            .bind(url)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn add_kol_request(
        &self,
        platform: &str,
        raw: &str,
        user_id: i64,
        name: &str,
        category_id: Option<i64>,
    ) -> Result<i64, CatalogError> {
        self.add_kol_request_inner(platform, raw, user_id, name, category_id, true)
            .await
    }

    pub async fn add_kol_request_without_category(
        &self,
        platform: &str,
        raw: &str,
        user_id: i64,
        name: &str,
    ) -> Result<i64, CatalogError> {
        self.add_kol_request_inner(platform, raw, user_id, name, None, false)
            .await
    }

    async fn add_kol_request_inner(
        &self,
        platform: &str,
        raw: &str,
        user_id: i64,
        name: &str,
        category_id: Option<i64>,
        require_category: bool,
    ) -> Result<i64, CatalogError> {
        if !PLATFORMS.contains(&platform) {
            return Err(CatalogError::Invalid(format!("不支持的平台: {platform}")));
        }
        let external_id = normalize_kol_request(platform, raw).map_err(CatalogError::Invalid)?;
        if require_category && category_id.is_none() {
            return Err(CatalogError::Bad("请选择分类"));
        }
        if let Some(category_id) = category_id {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                .bind(category_id)
                .fetch_optional(&self.pool)
                .await?;
            if found.is_none() {
                return Err(CatalogError::Bad("分类不存在"));
            }
        }
        let pending: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM kol_requests WHERE platform = ? AND external_id = ? AND status = 'pending'",
        )
        .bind(platform)
        .bind(&external_id)
        .fetch_optional(&self.pool)
        .await?;
        if pending.is_some() {
            return Err(CatalogError::Bad("该大V的申请已在处理中"));
        }
        let listed: Option<i64> =
            sqlx::query_scalar("SELECT id FROM kols WHERE platform = ? AND external_id = ?")
                .bind(platform)
                .bind(&external_id)
                .fetch_optional(&self.pool)
                .await?;
        if listed.is_some() {
            return Err(CatalogError::Bad("该大V已在目录中，直接订阅即可"));
        }
        match sqlx::query(
            "INSERT INTO kol_requests (platform, name, external_id, user_id, category_id)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(platform)
        .bind(name.trim())
        .bind(&external_id)
        .bind(user_id)
        .bind(category_id)
        .execute(&self.pool)
        .await
        {
            Ok(res) => Ok(res.last_insert_rowid()),
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                Err(CatalogError::Bad("该大V的申请已在处理中"))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn telegram_admin_chat_ids(&self) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT telegram_chat_id FROM users
             WHERE is_admin = 1 AND telegram_chat_id != ''
             ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn kol_request_pending(&self, request_id: i64) -> Result<bool, sqlx::Error> {
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM kol_requests WHERE id = ?")
                .bind(request_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(status.as_deref() == Some("pending"))
    }

    pub async fn list_kol_requests(
        &self,
        status: &str,
        user_id: i64,
    ) -> Result<Vec<Value>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.id, r.platform, r.name, r.external_id, r.user_id, r.category_id,
                    r.status, r.created_at, COALESCE(r.handled_at, '') AS handled_at,
                    COALESCE(u.username, '') AS requester,
                    COALESCE(c.name, '') AS category_name
             FROM kol_requests r
             LEFT JOIN users u ON u.id = r.user_id
             LEFT JOIN categories c ON c.id = r.category_id
             WHERE (? = '' OR r.status = ?) AND (? = 0 OR r.user_id = ?)
             ORDER BY r.id DESC",
        )
        .bind(status)
        .bind(status)
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| request_json(&row)).collect())
    }

    pub async fn approve_kol_request(&self, request_id: i64) -> Result<i64, CatalogError> {
        Ok(self
            .approve_kol_request_as(request_id, None, 0)
            .await?
            .kol_id
            .expect("approved request has a kol id"))
    }

    pub async fn approve_kol_request_as(
        &self,
        request_id: i64,
        category_override: Option<i64>,
        actor_id: i64,
    ) -> Result<KolRequestEffect, CatalogError> {
        let row = sqlx::query(
            "SELECT r.platform, r.name, r.external_id, r.user_id, r.category_id, r.status,
                    COALESCE(u.telegram_chat_id, '') AS applicant_chat_id,
                    COALESCE(u.username, '') AS applicant_name
             FROM kol_requests r LEFT JOIN users u ON u.id = r.user_id
             WHERE r.id = ?",
        )
        .bind(request_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        };
        let status: String = row.get("status");
        if status != "pending" {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        }
        let platform: String = row.get("platform");
        let external_id: String = row.get("external_id");
        let stored = external_id.trim_start_matches('@');
        match normalize_kol_request(&platform, &external_id) {
            Ok(normalized) if normalized == stored => {}
            Ok(_) => {
                return Err(CatalogError::Invalid(format!(
                    "该申请的外部ID「{external_id}」无效（格式不符），建议点「拒绝」"
                )))
            }
            Err(err) => {
                return Err(CatalogError::Invalid(format!(
                    "该申请的外部ID「{external_id}」无效（{err}），建议点「拒绝」"
                )))
            }
        }
        let given: String = row.get("name");
        let name = if given.trim().is_empty() {
            format!("{platform}_{stored}")
        } else {
            given.trim().to_string()
        };
        let stored_category_id: Option<i64> = row.get("category_id");
        let category_id = category_override
            .map(|id| (id > 0).then_some(id))
            .unwrap_or(stored_category_id);
        if let Some(id) = category_id {
            let found: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
            if found.is_none() {
                return Err(CatalogError::Bad("分类不存在"));
            }
        }
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE kol_requests SET status = 'approved', category_id = ?, handled_at = datetime('now')
             WHERE id = ? AND status = 'pending'",
        )
        .bind(category_id)
        .bind(request_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        }
        let kol_id = match sqlx::query(
            "INSERT INTO kols
                (platform, name, external_id, category_id, priority, secondary, original_only)
             VALUES (?, ?, ?, ?, 0, 0, 0)",
        )
        .bind(&platform)
        .bind(&name)
        .bind(stored)
        .bind(category_id)
        .execute(&mut *tx)
        .await
        {
            Ok(result) => result.last_insert_rowid(),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                return Err(CatalogError::Bad("该大V已在目录中"));
            }
            Err(error) => return Err(error.into()),
        };
        let user_id: i64 = row.get("user_id");
        sqlx::query(
            "INSERT INTO subscriptions (user_id, kol_id, type) VALUES (?, ?, 'post')
             ON CONFLICT(user_id, kol_id) DO UPDATE SET type = excluded.type",
        )
        .bind(user_id)
        .bind(kol_id)
        .execute(&mut *tx)
        .await?;
        if actor_id > 0 {
            sqlx::query(
                "INSERT INTO admin_logs (user_id, action, target, detail)
                 VALUES (?, 'approve_kol_request', ?, ?)",
            )
            .bind(actor_id)
            .bind(request_id.to_string())
            .bind(format!(
                "kol_id={kol_id}, category_id={}",
                category_id.unwrap_or(0)
            ))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(KolRequestEffect {
            request_id,
            kol_id: Some(kol_id),
            applicant_user_id: user_id,
            applicant_chat_id: row.get("applicant_chat_id"),
            applicant_name: row.get("applicant_name"),
            platform,
            external_id: stored.to_string(),
            name,
            category_id,
        })
    }

    pub async fn reject_kol_request(&self, request_id: i64) -> Result<(), CatalogError> {
        self.reject_kol_request_as(request_id, 0).await.map(|_| ())
    }

    pub async fn reject_kol_request_as(
        &self,
        request_id: i64,
        actor_id: i64,
    ) -> Result<KolRequestEffect, CatalogError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT r.platform, r.name, r.external_id, r.user_id, r.category_id, r.status,
                    COALESCE(u.telegram_chat_id, '') AS applicant_chat_id,
                    COALESCE(u.username, '') AS applicant_name
             FROM kol_requests r LEFT JOIN users u ON u.id = r.user_id
             WHERE r.id = ?",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        };
        let status: String = row.get("status");
        if status != "pending" {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        }
        let updated = sqlx::query(
            "UPDATE kol_requests SET status = 'rejected', handled_at = datetime('now')
             WHERE id = ? AND status = 'pending'",
        )
        .bind(request_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(CatalogError::Missing("申请不存在或已处理"));
        }
        if actor_id > 0 {
            sqlx::query(
                "INSERT INTO admin_logs (user_id, action, target, detail)
                 VALUES (?, 'reject_kol_request', ?, '')",
            )
            .bind(actor_id)
            .bind(request_id.to_string())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(KolRequestEffect {
            request_id,
            kol_id: None,
            applicant_user_id: row.get("user_id"),
            applicant_chat_id: row.get("applicant_chat_id"),
            applicant_name: row.get("applicant_name"),
            platform: row.get("platform"),
            external_id: row.get("external_id"),
            name: row.get("name"),
            category_id: row.get("category_id"),
        })
    }

    pub async fn note_kol_fetch(
        &self,
        kol_id: i64,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|item| item.as_secs() as i64)
            .unwrap_or(0);
        let detail = error.unwrap_or("").chars().take(300).collect::<String>();
        let row = sqlx::query(
            "SELECT platform, name, fetch_fail_streak, source_alerted FROM kols WHERE id = ?",
        )
        .bind(kol_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else { return Ok(()) };
        let platform: String = row.get("platform");
        let name: String = row.get("name");
        let alerted: i64 = row.get("source_alerted");
        if detail.is_empty() {
            sqlx::query("UPDATE kols SET last_fetch_at = ?, last_fetch_error = '', fetch_fail_streak = 0, source_alerted = 0 WHERE id = ?")
                .bind(now.to_string())
                .bind(kol_id)
                .execute(&self.pool)
                .await?;
            self.insert_source_event(&platform, "ok", "", 1, 0).await?;
            if alerted == 1 {
                self.emit_kol_alert(
                    &crate::alerts::kol_recovered_message(&platform, &name),
                    "kol_recovered",
                    &name,
                )
                .await?;
            }
            return Ok(());
        }
        let streak = row.get::<i64, _>("fetch_fail_streak") + 1;
        self.insert_source_event(&platform, "fail", &detail, 0, 1)
            .await?;
        let wide = crate::alerts::platform_wide(&detail);
        if streak >= 5 && crate::alerts::terminal_kol(&detail) && !wide {
            sqlx::query("UPDATE kols SET enabled = 0, last_fetch_at = ?, last_fetch_error = ?, fetch_fail_streak = 0, source_alerted = 0 WHERE id = ?")
                .bind(now.to_string())
                .bind(&detail)
                .bind(kol_id)
                .execute(&self.pool)
                .await?;
            self.emit_kol_alert(
                &crate::alerts::kol_disabled_message(&platform, &name, &detail, streak),
                "kol_disabled",
                &name,
            )
            .await?;
            return Ok(());
        }
        let mut marked = alerted;
        if streak == 3 || streak % 10 == 0 {
            let key = format!("source_alert_{platform}");
            let last = self
                .setting(&key)
                .await?
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(0);
            if crate::alerts::alerts_enabled()
                && (last == 0 || now.saturating_sub(last) >= 6 * 3600)
            {
                self.set_setting(&key, &now.to_string()).await?;
                marked = 1;
                self.emit_kol_alert(
                    &crate::alerts::kol_failure_message(&platform, &name, &detail, streak),
                    "kol_failure",
                    &name,
                )
                .await?;
            }
        }
        sqlx::query("UPDATE kols SET last_fetch_at = ?, last_fetch_error = ?, fetch_fail_streak = ?, source_alerted = ? WHERE id = ?")
            .bind(now.to_string())
            .bind(&detail)
            .bind(streak)
            .bind(marked)
            .bind(kol_id)
            .execute(&self.pool)
            .await?;
        if platform == "weibo"
            && (detail.contains("登录") || detail.to_lowercase().contains("login"))
        {
            self.daily_kol_warning(
                "weibo_warning_date",
                &crate::alerts::weibo_login_message(&detail),
                "weibo_login",
            )
            .await?;
        }
        if platform == "xueqiu"
            && (detail.contains("cookie") || detail.contains("WAF") || detail.contains("反爬"))
        {
            self.daily_kol_warning(
                "xueqiu_warning_date",
                &crate::alerts::xueqiu_cookie_message(&detail),
                "xueqiu_cookie",
            )
            .await?;
        }
        Ok(())
    }

    async fn insert_source_event(
        &self,
        platform: &str,
        status: &str,
        detail: &str,
        ok_count: i64,
        fail_count: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO source_events (platform, status, detail, ok_count, fail_count) VALUES (?, ?, ?, ?, ?)")
            .bind(platform)
            .bind(status)
            .bind(detail)
            .bind(ok_count)
            .bind(fail_count)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn emit_kol_alert(
        &self,
        message: &str,
        action: &str,
        target: &str,
    ) -> Result<(), sqlx::Error> {
        if crate::alerts::alerts_enabled() {
            crate::alerts::notify_admins(self, message).await;
            self.add_admin_log(0, action, target, message).await?;
        }
        Ok(())
    }

    async fn daily_kol_warning(
        &self,
        key: &str,
        message: &str,
        action: &str,
    ) -> Result<(), sqlx::Error> {
        if !crate::alerts::alerts_enabled() {
            return Ok(());
        }
        let today: String = sqlx::query_scalar("SELECT date('now', '+8 hours')")
            .fetch_one(&self.pool)
            .await?;
        if self.setting(key).await?.as_deref() == Some(today.as_str()) {
            return Ok(());
        }
        self.set_setting(key, &today).await?;
        self.emit_kol_alert(message, action, "").await
    }
}

const PLATFORMS: &[&str] = &[
    "xueqiu",
    "combination",
    "weibo",
    "twitter",
    "ima",
    "zsxq",
    "truth",
];

const PLAZA_PLATFORMS: &[&str] = &["xueqiu", "combination", "weibo", "twitter", "zsxq", "truth"];

const POLL_FIELDS: &[(&str, &str, i64, i64, i64)] = &[
    ("interval_seconds", "config_interval_seconds", 180, 1, 3600),
    (
        "priority_interval_seconds",
        "config_priority_interval_seconds",
        60,
        1,
        600,
    ),
    (
        "truth_interval_seconds",
        "config_truth_interval_seconds",
        0,
        0,
        600,
    ),
    (
        "digest_interval_seconds",
        "config_digest_interval_seconds",
        0,
        0,
        86400,
    ),
    (
        "source_probe_interval_seconds",
        "config_source_probe_interval_seconds",
        0,
        0,
        86400,
    ),
    (
        "cookie_keepalive_interval_seconds",
        "config_cookie_keepalive_interval_seconds",
        0,
        0,
        86400,
    ),
    ("daily_report_hour", "config_daily_report_hour", 8, 0, 23),
    (
        "combination_base_seconds",
        "config_combination_base_seconds",
        60,
        5,
        3600,
    ),
    (
        "combination_idle_cap_seconds",
        "config_combination_idle_cap_seconds",
        600,
        5,
        86400,
    ),
    (
        "normal_idle_cap_seconds",
        "config_normal_idle_cap_seconds",
        1800,
        5,
        86400,
    ),
    (
        "priority_idle_cap_seconds",
        "config_priority_idle_cap_seconds",
        600,
        5,
        86400,
    ),
    (
        "x_fallback_cap_seconds",
        "config_x_fallback_cap_seconds",
        600,
        5,
        86400,
    ),
    (
        "secondary_interval_seconds",
        "config_secondary_base_seconds",
        3600,
        60,
        86400,
    ),
    (
        "secondary_idle_cap_seconds",
        "config_secondary_idle_cap_seconds",
        21600,
        60,
        86400,
    ),
    (
        "secondary_digest_interval_seconds",
        "config_secondary_digest_interval_seconds",
        0,
        0,
        86400,
    ),
    (
        "secondary_min_digest_count",
        "config_secondary_min_digest_count",
        1,
        1,
        100,
    ),
];

const ZSXQ_INTS: &[(&str, &str, i64, i64)] = &[
    ("zsxq_max_pages", "zsxq_max_pages", 1, 20),
    ("zsxq_max_comment_pages", "zsxq_max_comment_pages", 1, 10),
    ("zsxq_comment_budget", "zsxq_comment_budget", 1, 200),
];

const ZSXQ_FLOATS: &[(&str, &str, f64, f64)] = &[
    (
        "zsxq_fetch_delay_seconds",
        "zsxq_fetch_delay_seconds",
        0.2,
        10.0,
    ),
    (
        "zsxq_file_delay_seconds",
        "zsxq_file_delay_seconds",
        0.2,
        10.0,
    ),
];

const VISIBLE: &str = "(? = 1 OR k.is_private = 0 OR EXISTS (
    SELECT 1 FROM kol_acl a WHERE a.kol_id = k.id AND a.user_id = ?))";

const KOL_SELECT: &str = "
SELECT k.id, k.platform, k.name, k.external_id, k.avatar_url, k.enabled,
       k.is_private, k.original_only, k.priority, k.recommend_weight, k.category_id,
       COALESCE(c.name, '') AS category_name,
       CASE WHEN s.user_id IS NULL THEN 0 ELSE 1 END AS subscribed,
       COALESCE(s.type, 'post') AS subscribe_type,
       COALESCE(s.favorite, 0) AS favorite,
       COALESCE(s.secondary, 0) AS secondary,
       COALESCE((SELECT MAX(p.published_at) FROM posts p WHERE p.kol_id = k.id), '') AS last_post_at
FROM kols k
LEFT JOIN categories c ON c.id = k.category_id
LEFT JOIN subscriptions s ON s.kol_id = k.id AND s.user_id = ?";

#[derive(Clone, Copy)]
pub struct FeedFilter<'a> {
    pub limit: i64,
    pub offset: i64,
    pub platform: &'a str,
    pub category_id: i64,
    pub q: &'a str,
    pub favorite: bool,
    pub tag: &'a str,
    pub include_secondary: bool,
    pub since_id: i64,
}

fn like_contains(q: &str) -> String {
    let mut out = String::from('%');
    for ch in q.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

fn clip_line(line: &str) -> String {
    line.chars().take(80).collect()
}

pub fn parse_batch_kol_line(line: &str) -> Result<(String, String, String), String> {
    let mut nickname = String::new();
    let mut external_id = String::new();
    let mut platform = "";
    let mut parse_error = None;
    let mut unrecognized = false;
    for token in line.split_whitespace() {
        if let Some(detected) = detect_platform(token) {
            match normalize_kol_request(detected, token) {
                Ok(ext) => {
                    platform = detected;
                    external_id = ext;
                    parse_error = None;
                    unrecognized = false;
                }
                Err(err) => parse_error = Some(err),
            }
            continue;
        }
        if token.starts_with("http://")
            || token.starts_with("https://")
            || token.chars().all(|c| c.is_ascii_digit())
        {
            unrecognized = true;
            continue;
        }
        if !nickname.is_empty() {
            nickname.push(' ');
        }
        nickname.push_str(token);
    }
    if external_id.is_empty() {
        return Err(if unrecognized {
            "无法识别平台，请粘贴雪球/微博/X/知识星球主页链接".into()
        } else {
            parse_error.unwrap_or_else(|| "未识别到链接或ID".into())
        });
    }
    Ok((
        platform.to_string(),
        normalize_kol_request(platform, &external_id)?,
        nickname,
    ))
}

trait BindIds<'q> {
    fn bind_ids(
        self,
        ids: &'q [i64],
    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>;
}

impl<'q> BindIds<'q> for sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    fn bind_ids(mut self, ids: &'q [i64]) -> Self {
        for id in ids {
            self = self.bind(id);
        }
        self
    }
}

pub fn normalize_external_id(platform: &str, raw: &str) -> String {
    let raw = raw.trim();
    let no_query = raw.split(['?', '#']).next().unwrap_or(raw);
    let seg = no_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(no_query)
        .trim_matches(|c: char| c == '@' || c.is_whitespace());
    match platform {
        "xueqiu" | "weibo" | "zsxq"
            if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) =>
        {
            seg.to_string()
        }
        _ => seg.to_string(),
    }
}

const TWITTER_PAGES: &[&str] = &[
    "home",
    "explore",
    "search",
    "settings",
    "notifications",
    "messages",
    "compose",
    "bookmarks",
    "jobs",
    "login",
    "signup",
    "account",
    "i",
];

pub fn normalize_kol_request(platform: &str, raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("请输入大V主页链接或 ID".into());
    }
    if let Some(detected) = detect_platform(text) {
        if detected != platform {
            return Err(format!(
                "检测到这是「{}」的主页链接，请把平台切换为「{}」（当前选的是「{}」）",
                platform_label(detected),
                platform_label(detected),
                platform_label(platform)
            ));
        }
    }
    match platform {
        "xueqiu" => digits_after(text, &["xueqiu.com/u/", "xueqiu.com/"])
            .or_else(|| only_digits(text))
            .ok_or_else(|| "无法识别的雪球主页链接，请使用 xueqiu.com/u/<数字ID> 形式（或直接填数字 ID）".into()),
        "combination" => zh_code(text)
            .ok_or_else(|| "无法识别的雪球组合链接，请使用 xueqiu.com/P/ZHxxxxxx 或组合代码 ZHxxxxxx".into()),
        "weibo" => digits_after(text, &["weibo.com/u/", "m.weibo.cn/u/"])
            .or_else(|| only_digits(text))
            .ok_or_else(|| "无法识别的微博主页链接，请复制对方主页「.../u/<数字UID>」形式的链接".into()),
        "ima" => ima_id(text)
            .ok_or_else(|| "无法识别的 ima 知识库链接，请使用 wiki URL 里的 knowledgeBaseId（或直接填知识库 ID）".into()),
        "zsxq" => zsxq_id(text)
            .or_else(|| only_digits(text))
            .ok_or_else(|| "无法识别的知识星球链接，请使用 wx.zsxq.com 群链接或星球 ID".into()),
        "twitter" => twitter_id(text),
        _ => Err(format!("不支持的平台: {platform}")),
    }
}

fn platform_label(platform: &str) -> &str {
    match platform {
        "xueqiu" => "雪球",
        "combination" => "雪球组合",
        "weibo" => "微博",
        "twitter" => "X",
        "ima" => "ima",
        "zsxq" => "知识星球",
        "truth" => "Truth",
        _ => platform,
    }
}

fn detect_platform(text: &str) -> Option<&'static str> {
    if text.contains("xueqiu.com/P/") || zh_code(text).is_some() {
        Some("combination")
    } else if text.contains("xueqiu.com") {
        Some("xueqiu")
    } else if text.contains("weibo.com") || text.contains("weibo.cn") {
        Some("weibo")
    } else if text.contains("twitter.com") || has_boundary(text, "x.com") {
        Some("twitter")
    } else if text.contains("ima.qq.com") {
        Some("ima")
    } else if text.contains("zsxq.com") {
        Some("zsxq")
    } else if has_boundary(text, "truthsocial.com") {
        Some("truth")
    } else {
        None
    }
}

fn has_boundary(text: &str, marker: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find(marker) {
        let abs = from + at;
        if abs == 0 || matches!(bytes[abs - 1], b'/' | b':' | b'.') {
            return true;
        }
        from = abs + 1;
    }
    false
}

fn digits_after(text: &str, markers: &[&str]) -> Option<String> {
    for marker in markers {
        let Some((_, rest)) = text.split_once(marker) else {
            continue;
        };
        let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !id.is_empty() {
            return Some(id);
        }
    }
    None
}

fn only_digits(text: &str) -> Option<String> {
    text.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| text.to_string())
}

fn zh_code(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find("ZH") {
        let tail = &rest[at + 2..];
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(format!("ZH{digits}"));
        }
        rest = &rest[at + 2..];
    }
    None
}

fn ima_id(text: &str) -> Option<String> {
    if text.contains("ima.qq.com") {
        if let Some((_, rest)) = text.split_once("knowledgeBaseId=") {
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    let ok = (6..=64).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    ok.then(|| text.to_string())
}

fn zsxq_id(text: &str) -> Option<String> {
    for marker in ["group/", "group_id="] {
        let Some((_, rest)) = text.split_once(marker) else {
            continue;
        };
        let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if id.len() >= 6 {
            return Some(id);
        }
    }
    None
}

fn twitter_id(text: &str) -> Result<String, String> {
    if let Some(path) = twitter_path(text) {
        if text.contains("/status/")
            || text.contains("/i/")
            || text.ends_with("/i")
            || TWITTER_PAGES.contains(&path.as_str())
        {
            return Err("这是 X 的系统页面/推文链接，请复制用户主页链接（x.com/<用户名>）".into());
        }
        return Ok(path);
    }
    let handle = text.strip_prefix('@').unwrap_or(text);
    if (1..=15).contains(&handle.len())
        && handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Ok(handle.to_string());
    }
    Err("无法识别的 X 用户名，请使用 x.com/<用户名> 链接或 @用户名".into())
}

fn twitter_path(text: &str) -> Option<String> {
    for marker in ["x.com/", "twitter.com/"] {
        let Some((_, rest)) = text.split_once(marker) else {
            continue;
        };
        let path: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

fn plaza_modes(raw: Option<&str>) -> HashMap<String, String> {
    let mut out = PLAZA_PLATFORMS
        .iter()
        .map(|platform| ((*platform).to_string(), "auto".to_string()))
        .collect::<HashMap<_, _>>();
    let Ok(value) = serde_json::from_str::<Value>(raw.unwrap_or("")) else {
        return out;
    };
    let Some(obj) = value.as_object() else {
        return out;
    };
    for (platform, mode) in obj {
        let Some(mode) = mode.as_str() else { continue };
        if out.contains_key(platform) && matches!(mode, "auto" | "show" | "hide") {
            out.insert(platform.clone(), mode.to_string());
        }
    }
    out
}

fn plaza_rows(counts: &HashMap<String, i64>, settings: &HashMap<String, String>) -> Vec<Value> {
    let modes = plaza_modes(settings.get("plaza_source_visibility").map(String::as_str));
    PLAZA_PLATFORMS
        .iter()
        .map(|platform| {
            let enabled = counts.get(*platform).copied().unwrap_or(0);
            let mode = modes.get(*platform).map(String::as_str).unwrap_or("auto");
            json!({
                "platform": platform,
                "mode": mode,
                "enabled_kols": enabled,
                "visible": mode == "show" || (mode == "auto" && enabled > 0),
            })
        })
        .collect()
}

fn polling_json(settings: &HashMap<String, String>) -> Value {
    let mut out = serde_json::Map::new();
    for (name, key, default, _, _) in POLL_FIELDS {
        out.insert((*name).into(), json!(int_setting(settings, key, *default)));
    }
    out.insert(
        "translate_twitter_content".into(),
        json!(settings
            .get("config_translate_twitter_content")
            .is_some_and(|v| v == "1")),
    );
    out.insert(
        "telegram_rich_messages".into(),
        json!(settings
            .get("config_telegram_rich_messages")
            .map(|v| v == "1")
            .unwrap_or(true)),
    );
    out.insert(
        "zsxq_max_pages".into(),
        json!(int_setting(settings, "zsxq_max_pages", 3)),
    );
    out.insert(
        "zsxq_max_comment_pages".into(),
        json!(int_setting(settings, "zsxq_max_comment_pages", 3)),
    );
    out.insert(
        "zsxq_comment_budget".into(),
        json!(int_setting(settings, "zsxq_comment_budget", 30)),
    );
    out.insert(
        "zsxq_fetch_delay_seconds".into(),
        json!(float_setting(settings, "zsxq_fetch_delay_seconds", 1.0)),
    );
    out.insert(
        "zsxq_file_delay_seconds".into(),
        json!(float_setting(settings, "zsxq_file_delay_seconds", 1.0)),
    );
    out.insert(
        "zsxq_prefetch_files".into(),
        json!(settings
            .get("zsxq_prefetch_files")
            .is_some_and(|v| v == "1")),
    );
    out.insert(
        "zsxq_fetch_comments".into(),
        json!(settings
            .get("zsxq_fetch_comments")
            .is_some_and(|v| v == "1")),
    );
    out.insert(
        "zsxq_app_channel".into(),
        json!(settings.get("zsxq_app_channel").is_some_and(|v| v == "1")),
    );
    let device = settings
        .get("zsxq_app_device")
        .filter(|v| !v.is_empty())
        .map(String::as_str)
        .unwrap_or("16 OnePlus_PJD110");
    out.insert("zsxq_app_device".into(), json!(device));
    Value::Object(out)
}

fn int_setting(settings: &HashMap<String, String>, key: &str, default: i64) -> i64 {
    settings
        .get(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn float_setting(settings: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    settings
        .get(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn cookie_status(settings: &HashMap<String, String>, key: &str, env: Option<String>) -> Value {
    let saved = settings.get(key).map(String::as_str).unwrap_or("").trim();
    if !saved.is_empty() {
        return json!({
            "set": true,
            "updated_at": settings.get(&format!("{key}_updated_at")).cloned().unwrap_or_default(),
            "preview": "已配置",
        });
    }
    if env.is_some() {
        return json!({"set": true, "updated_at": "", "preview": "已配置", "from_env": true});
    }
    json!({"set": false, "updated_at": "", "preview": ""})
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn zsxq_cookie_value(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains('=') || raw.contains(';') {
        raw.rsplit('=').next().unwrap_or("").trim().to_string()
    } else {
        raw.to_string()
    }
}

fn admin_user_json(row: &sqlx::sqlite::SqliteRow, after: i64, purge: i64) -> Value {
    let username: String = row.get("username");
    let register_code: String = row.get("register_code");
    let last_login: Option<String> = row.get("last_login_at");
    let created_at: String = row.get("created_at");
    let subscribed: i64 = row.get("subscription_count");
    let bound = !row.get::<String, _>("telegram_chat_id").is_empty()
        || !row.get::<String, _>("feishu_open_id").is_empty()
        || !row.get::<String, _>("feishu_chat_id").is_empty()
        || !row.get::<String, _>("wecom_webhook").is_empty()
        || !row.get::<String, _>("bark_key").is_empty()
        || row.get::<i64, _>("webpush_count") > 0;
    let admin = row.get::<i64, _>("is_admin") != 0;
    let never_logged_in = last_login.as_deref().unwrap_or("").is_empty();
    let old_enough = after > 0 && created_before(&created_at, after);
    let inactive = !admin && never_logged_in && old_enough && !bound && subscribed == 0;
    let origin = if register_code.is_empty() {
        "web"
    } else {
        "invite"
    };
    json!({
        "id": row.get::<i64, _>("id"),
        "username": username,
        "is_admin": admin,
        "created_at": created_at,
        "notify_enabled": row.get::<i64, _>("notify_enabled") != 0,
        "daily_report_enabled": row.get::<i64, _>("daily_report") != 0,
        "dnd_enabled": !row.get::<String, _>("dnd_start").is_empty(),
        "push_channels": row.get::<String, _>("push_channels"),
        "subscription_count": subscribed,
        "telegram_bound": !row.get::<String, _>("telegram_chat_id").is_empty(),
        "feishu_bound": !row.get::<String, _>("feishu_open_id").is_empty() || !row.get::<String, _>("feishu_chat_id").is_empty(),
        "wecom_bound": !row.get::<String, _>("wecom_webhook").is_empty(),
        "bark_bound": !row.get::<String, _>("bark_key").is_empty(),
        "webpush_bound": row.get::<i64, _>("webpush_count") > 0,
        "custom_telegram_bot": !row.get::<String, _>("telegram_bot_token").is_empty(),
        "register_code": register_code,
        "register_note": "",
        "inactive": inactive,
        "days_until_purge": if inactive && purge > 0 { days_left(&row.get::<String, _>("created_at"), after + purge) } else { Value::Null },
        "origin": origin,
        "origin_label": if origin == "invite" { "邀请码" } else { "网页" },
        "has_password": row.get::<i64, _>("has_password") != 0,
        "last_login_at": last_login.unwrap_or_default(),
        "username_valid": crate::auth::validate_username(&row.get::<String, _>("username")).is_ok(),
        "ima_kb_groups": [],
        "ima_kb_subscribed": [],
    })
}

fn created_before(created_at: &str, days: i64) -> bool {
    days_left(created_at, days)
        .as_i64()
        .is_some_and(|left| left <= 0)
}

fn days_left(created_at: &str, days: i64) -> Value {
    let Some(created) = utc_unix(created_at) else {
        return Value::Null;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let left = (created + days * 86400 - now).div_euclid(86400);
    json!(left.max(0))
}

fn utc_unix(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.len() < 19 {
        return None;
    }
    let year: i32 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: i64 = text[11..13].parse().ok()?;
    let minute: i64 = text[14..16].parse().ok()?;
    let second: i64 = text[17..19].parse().ok()?;
    let mut y = year;
    y -= i32::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    let days = era as i64 * 146097 + doe as i64 - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

fn news_item(row: &sqlx::sqlite::SqliteRow, full: bool) -> Value {
    let is_read = row.get::<i64, _>("is_read") != 0;
    let mut value = json!({
        "id": row.get::<i64, _>("id"),
        "source_id": row.get::<i64, _>("source_id"),
        "source_name": row.get::<String, _>("source_name"),
        "source_platform": row.get::<String, _>("source_platform"),
        "feed_name": row.get::<Option<String>, _>("feed_name").unwrap_or_default(),
        "title": row.get::<String, _>("title"),
        "summary": row.get::<String, _>("summary"),
        "url": row.get::<String, _>("url"),
        "author": row.get::<String, _>("author"),
        "published_at": row.get::<String, _>("published_at"),
        "is_read": is_read,
        "is_new": !is_read,
        "has_image": row.get::<i64, _>("has_image") != 0,
        "topics": topics_of(&row.get::<String, _>("topics")),
    });
    if full {
        let content: String = row.get("content");
        let summary: String = row.get("summary");
        value["content"] = json!(&content);
        value["content_html"] = json!(reader_html(&content, &summary));
    }
    value
}

fn topics_of(raw: &str) -> Value {
    let Ok(list) = serde_json::from_str::<Vec<Value>>(raw) else {
        return json!([]);
    };
    Value::Array(
        list.into_iter()
            .filter_map(|item| {
                item.as_str()
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(|item| Value::String(item.to_string()))
            })
            .collect(),
    )
}

fn reader_html(content: &str, summary: &str) -> String {
    if !crate::news::plain(content).trim().is_empty() {
        return content.to_string();
    }
    let summary = summary.trim();
    if summary.is_empty() {
        return content.to_string();
    }
    let para = format!("<p>{}</p>", escape_html(summary));
    if content.contains("<img") {
        format!("{content}{para}")
    } else {
        para
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn request_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "platform": row.get::<String, _>("platform"),
        "name": row.get::<String, _>("name"),
        "external_id": row.get::<String, _>("external_id"),
        "user_id": row.get::<i64, _>("user_id"),
        "category_id": row.get::<Option<i64>, _>("category_id"),
        "status": row.get::<String, _>("status"),
        "created_at": row.get::<String, _>("created_at"),
        "handled_at": row.get::<String, _>("handled_at"),
        "requester": row.get::<String, _>("requester"),
        "category_name": row.get::<String, _>("category_name"),
    })
}

fn like_pattern(q: &str) -> String {
    if q.is_empty() {
        return String::new();
    }
    let mut out = String::from("%");
    for c in q.chars() {
        if matches!(c, '!' | '%' | '_') {
            out.push('!');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn tag_pattern(tag: &str) -> String {
    if tag.is_empty() {
        return String::new();
    }
    let quoted = serde_json::to_string(tag).unwrap_or_else(|_| "\"\"".into());
    let inner = quoted.trim_matches('"');
    let mut out = String::from("%\"");
    for c in inner.chars() {
        if matches!(c, '!' | '%' | '_') {
            out.push('!');
        }
        out.push(c);
    }
    out.push('"');
    out.push('%');
    out
}

fn subscription_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "platform": row.get::<String, _>("platform"),
        "name": row.get::<String, _>("name"),
        "external_id": row.get::<String, _>("external_id"),
        "avatar_url": row.get::<String, _>("avatar_url"),
        "subscribe_type": row.get::<String, _>("subscribe_type"),
        "favorite": row.get::<i64, _>("favorite") != 0,
        "secondary": row.get::<i64, _>("secondary") != 0,
        "hide_images": row.get::<i64, _>("hide_images") != 0,
    })
}

fn kol_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    let platform: String = row.get("platform");
    let mut value = json!({
        "id": row.get::<i64, _>("id"),
        "platform": platform,
        "name": row.get::<String, _>("name"),
        "external_id": row.get::<String, _>("external_id"),
        "avatar_url": row.get::<String, _>("avatar_url"),
        "enabled": row.get::<i64, _>("enabled") != 0,
        "is_private": row.get::<i64, _>("is_private") != 0,
        "original_only": row.get::<i64, _>("original_only") != 0,
        "priority": row.get::<i64, _>("priority") != 0,
        "recommend_weight": row.get::<i64, _>("recommend_weight"),
        "category_id": row.get::<Option<i64>, _>("category_id"),
        "category_name": row.get::<String, _>("category_name"),
        "subscribed": row.get::<i64, _>("subscribed") != 0,
        "subscribe_type": row.get::<String, _>("subscribe_type"),
        "favorite": row.get::<i64, _>("favorite") != 0,
        "secondary": row.get::<i64, _>("secondary") != 0,
        "last_post_at": row.get::<String, _>("last_post_at"),
    });
    if platform == "combination" {
        value["quote"] = Value::Null;
    }
    value
}

fn clean_category_name(name: &str) -> Result<&str, CatalogError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(CatalogError::Bad("分类名不能为空"));
    }
    Ok(name)
}

fn post_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    let hide = row.get::<i64, _>("hide_images") != 0;
    let images = serde_json::from_str::<Value>(row.get::<String, _>("images").as_str())
        .unwrap_or_else(|_| json!([]));
    let tags = serde_json::from_str::<Value>(row.get::<String, _>("tags").as_str())
        .unwrap_or_else(|_| json!([]));
    let detail_raw: String = row.get("detail");
    let detail = if detail_raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&detail_raw).unwrap_or(Value::String(detail_raw))
    };
    json!({
        "id": row.get::<i64, _>("id"),
        "platform": row.get::<String, _>("platform"),
        "kol_id": row.get::<i64, _>("kol_id"),
        "external_id": row.get::<String, _>("external_id"),
        "title": row.get::<String, _>("title"),
        "content": row.get::<String, _>("content"),
        "title_src": row.get::<String, _>("title_src"),
        "content_src": row.get::<String, _>("content_src"),
        "post_type": row.get::<String, _>("post_type"),
        "images": if hide { json!([]) } else { images },
        "tags": tags,
        "url": row.get::<String, _>("url"),
        "detail": detail,
        "published_at": row.get::<String, _>("published_at"),
        "fetched_at": row.get::<String, _>("fetched_at"),
        "kol_name": row.get::<String, _>("kol_name"),
        "avatar_url": row.get::<String, _>("avatar_url"),
        "kol_external_id": row.get::<String, _>("kol_external_id"),
        "category_id": row.get::<Option<i64>, _>("category_id"),
        "category_name": row.get::<String, _>("category_name"),
        "favorite": row.get::<i64, _>("favorite") != 0,
    })
}

fn ima_index_row(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "group_id": row.get::<String, _>("group_id"),
        "media_id": row.get::<String, _>("media_id"),
        "day": row.get::<String, _>("day"),
        "sort_date": row.get::<String, _>("sort_date"),
        "name": row.get::<String, _>("name"),
        "group_name": row.get::<String, _>("group_name"),
        "abstract": row.get::<String, _>("abstract"),
        "size": row.get::<i64, _>("size"),
        "chars": row.get::<i64, _>("chars"),
        "pdf_path": row.get::<String, _>("pdf_path"),
        "txt_path": row.get::<String, _>("txt_path"),
        "downloaded_at": row.get::<String, _>("downloaded_at"),
    })
}

fn news_source_json(row: &sqlx::sqlite::SqliteRow, feeds: &[sqlx::sqlite::SqliteRow]) -> Value {
    let id: i64 = row.get("id");
    let enabled = row.get::<i64, _>("enabled") != 0;
    let archived: Option<String> = row.get("archived_at");
    let feed_rows = feeds
        .iter()
        .filter(|feed| feed.get::<i64, _>("source_id") == id)
        .map(news_feed_json)
        .collect::<Vec<_>>();
    json!({
        "id": id,
        "slug": row.get::<String, _>("slug"),
        "name": row.get::<String, _>("name"),
        "group_name": row.get::<String, _>("group_name"),
        "enabled": enabled,
        "kind": row.get::<String, _>("kind"),
        "internal": row.get::<i64, _>("internal"),
        "archived_at": archived,
        "article_count": row.get::<i64, _>("article_count"),
        "status": if enabled { "ok" } else { "paused" },
        "last_success_at": row.get::<String, _>("last_success_at"),
        "feeds": feed_rows,
    })
}

fn news_feed_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    let archived: Option<String> = row.get("archived_at");
    json!({
        "id": row.get::<i64, _>("id"),
        "source_id": row.get::<i64, _>("source_id"),
        "name": row.get::<String, _>("name"),
        "url": row.get::<String, _>("url"),
        "enabled": row.get::<i64, _>("enabled") != 0,
        "archived_at": archived,
        "consecutive_failures": row.get::<i64, _>("consecutive_failures"),
        "last_success_at": row.get::<String, _>("last_success_at"),
        "last_error_detail": row.get::<String, _>("last_error_detail"),
    })
}

async fn tables_with_column(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    column: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut matched = Vec::new();
    for name in names {
        if !sql_ident(&name) {
            continue;
        }
        let cols: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{name}')"))
                .fetch_all(&mut **tx)
                .await?;
        if cols.iter().any(|col| col == column) {
            matched.push(name);
        }
    }
    Ok(matched)
}

fn sql_ident(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn sql_mentions_name(sql: &str, name: &str) -> bool {
    let sql = sql.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let bytes = sql.as_bytes();
    let needle = name.as_bytes();
    if needle.is_empty() {
        return false;
    }
    let mut start = 0;
    while let Some(rel) = sql[start..].find(&name) {
        let at = start + rel;
        let before_ok =
            at == 0 || (!bytes[at - 1].is_ascii_alphanumeric() && bytes[at - 1] != b'_');
        let after = at + needle.len();
        let after_ok =
            after >= bytes.len() || (!bytes[after].is_ascii_alphanumeric() && bytes[after] != b'_');
        if before_ok && after_ok {
            return true;
        }
        start = at + name.len().max(1);
    }
    false
}

async fn views_touching_users(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<Vec<(String, String)>, sqlx::Error> {
    let mut pending: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master WHERE type = 'view' AND sql IS NOT NULL ORDER BY name",
    )
    .fetch_all(&mut **tx)
    .await?;
    let mut selected: Vec<(String, String)> = Vec::new();
    loop {
        let mut next: Vec<(String, String)> = Vec::new();
        let mut grew = false;
        for (name, sql) in pending {
            let hits_users = sql_mentions_name(&sql, "users");
            let hits_selected = selected.iter().any(|(dep, _)| sql_mentions_name(&sql, dep));
            if hits_users || hits_selected {
                selected.push((name, sql));
                grew = true;
            } else {
                next.push((name, sql));
            }
        }
        pending = next;
        if !grew {
            break;
        }
    }
    Ok(selected)
}

async fn register_code_columns(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<Vec<String>, sqlx::Error> {
    let exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'register_codes'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT name FROM pragma_table_info('register_codes')")
        .fetch_all(&mut **tx)
        .await
}

async fn drop_named(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    kind: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    if !sql_ident(name) {
        return Err(users_migrate_err(format!(
            "无法临时移除引用 users 的对象 {name}。请先备份数据库并人工处理"
        )));
    }
    sqlx::query(&format!("DROP {kind} \"{name}\""))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn ensure_users_autoincrement(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let sql: Option<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'users'")
            .fetch_optional(pool)
            .await?;
    let Some(sql) = sql else {
        return Ok(());
    };
    if sql.to_ascii_uppercase().contains("AUTOINCREMENT") {
        return Ok(());
    }
    let create_sql = match rewrite_users_ddl(&sql) {
        Ok(sql) => sql,
        Err(msg) => {
            let mut conn = pool.acquire().await?;
            let hint = users_dependent_hint(&mut conn).await.unwrap_or_default();
            let detail = if hint.is_empty() {
                msg
            } else {
                format!("{msg} 相关对象: {hint}")
            };
            return Err(users_migrate_err(detail));
        }
    };
    let mut conn = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await?;
    let fk_flag: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await?;
    if fk_flag != 0 {
        return Err(users_migrate_err(
            "无法在事务外关闭外键检查，users 迁移已中止。请先备份数据库并人工处理",
        ));
    }
    let result = rebuild_users_autoincrement(&mut conn, &create_sql).await;
    let _ = sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&mut *conn)
        .await;
    result
}

async fn users_dependent_hint(conn: &mut sqlx::SqliteConnection) -> Result<String, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT type, name FROM sqlite_master
         WHERE name != 'users'
           AND sql IS NOT NULL
           AND (
                (type IN ('view', 'trigger') AND instr(lower(sql), 'users') > 0)
                OR (type = 'table' AND instr(lower(sql), 'references') > 0 AND instr(lower(sql), 'users') > 0)
           )
         ORDER BY type, name",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, name)| format!("{kind}:{name}"))
        .collect::<Vec<_>>()
        .join(", "))
}

async fn rebuild_users_autoincrement(
    conn: &mut sqlx::SqliteConnection,
    create_sql: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = conn.begin().await?;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND tbl_name = 'users' AND sql IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?;
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'users' AND sql IS NOT NULL",
    )
    .fetch_all(&mut *tx)
    .await?;
    let views = views_touching_users(&mut tx).await?;
    let view_names: Vec<String> = views.iter().map(|(name, _)| name.clone()).collect();
    let trigger_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT name, tbl_name, sql FROM sqlite_master
         WHERE type = 'trigger' AND tbl_name != 'users' AND sql IS NOT NULL
         ORDER BY name",
    )
    .fetch_all(&mut *tx)
    .await?;
    // INSTEAD OF triggers are dropped with their view, so capture every trigger on a
    // view we are about to remove, plus triggers whose body names users.
    let outside_triggers: Vec<(String, String)> = trigger_rows
        .into_iter()
        .filter(|(_, tbl, sql)| {
            view_names.iter().any(|view| view == tbl) || sql_mentions_name(sql, "users")
        })
        .map(|(name, _, sql)| (name, sql))
        .collect();
    sqlx::query("DROP TABLE IF EXISTS users__autoinc")
        .execute(&mut *tx)
        .await?;
    sqlx::query(create_sql).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO users__autoinc SELECT * FROM users")
        .execute(&mut *tx)
        .await?;
    for (name, _) in outside_triggers.iter().rev() {
        drop_named(&mut tx, "TRIGGER", name).await?;
    }
    for (name, _) in views.iter().rev() {
        drop_named(&mut tx, "VIEW", name).await?;
    }
    sqlx::query("DROP TABLE users").execute(&mut *tx).await?;
    sqlx::query("ALTER TABLE users__autoinc RENAME TO users")
        .execute(&mut *tx)
        .await?;
    for index in indexes {
        sqlx::query(&index).execute(&mut *tx).await?;
    }
    for trigger in &triggers {
        sqlx::query(trigger).execute(&mut *tx).await?;
    }
    for (_, sql) in &views {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    for (_, sql) in &outside_triggers {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    let tables = tables_with_column(&mut tx, "user_id").await?;
    let code_cols = register_code_columns(&mut tx).await?;
    let mut seq: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    for table in &tables {
        let max_user: i64 = sqlx::query_scalar(&format!(
            "SELECT COALESCE(MAX(user_id), 0) FROM \"{table}\""
        ))
        .fetch_one(&mut *tx)
        .await?;
        if max_user > seq {
            seq = max_user;
        }
    }
    for column in ["used_by", "created_by"] {
        if !code_cols.iter().any(|name| name == column) {
            continue;
        }
        let max_id: i64 = sqlx::query_scalar(&format!(
            "SELECT COALESCE(MAX(\"{column}\"), 0) FROM register_codes"
        ))
        .fetch_one(&mut *tx)
        .await?;
        if max_id > seq {
            seq = max_id;
        }
    }
    for table in &tables {
        sqlx::query(&format!(
            "DELETE FROM \"{table}\" WHERE user_id IS NOT NULL AND user_id NOT IN (SELECT id FROM users)"
        ))
        .execute(&mut *tx)
        .await?;
    }
    for column in ["used_by", "created_by"] {
        if !code_cols.iter().any(|name| name == column) {
            continue;
        }
        sqlx::query(&format!(
            "UPDATE register_codes SET \"{column}\" = NULL
             WHERE \"{column}\" IS NOT NULL AND \"{column}\" NOT IN (SELECT id FROM users)"
        ))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM sqlite_sequence WHERE name IN ('users', 'users__autoinc')")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO sqlite_sequence (name, seq) VALUES ('users', ?)")
        .bind(seq)
        .execute(&mut *tx)
        .await?;
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *tx)
        .await?;
    if !violations.is_empty() {
        return Err(users_migrate_err(
            "users 迁移后外键校验失败，已回滚。请先备份数据库并人工处理",
        ));
    }
    tx.commit().await?;
    Ok(())
}

fn rewrite_users_ddl(sql: &str) -> Result<String, String> {
    let renamed = rename_created_table(sql, "users__autoinc")?;
    add_autoincrement_pk(&renamed)
}

fn users_migrate_err(msg: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Configuration(std::io::Error::other(msg.into()).into())
}

fn rename_created_table(sql: &str, new_name: &str) -> Result<String, String> {
    let upper = sql.to_ascii_uppercase();
    let Some(start) = upper.find("CREATE TABLE") else {
        return Err("users 建表语句无法识别。请先备份数据库并人工处理".into());
    };
    let mut i = start + "CREATE TABLE".len();
    i = skip_sql_ws(sql, i);
    if sql[i..].to_ascii_uppercase().starts_with("IF NOT EXISTS") {
        i += "IF NOT EXISTS".len();
        i = skip_sql_ws(sql, i);
    }
    let (name_end, name) = read_sql_name(sql, i)?;
    if !name.eq_ignore_ascii_case("users") {
        return Err("users 建表语句无法识别。请先备份数据库并人工处理".into());
    }
    let mut out = String::new();
    out.push_str(&sql[..i]);
    out.push_str(new_name);
    out.push_str(&sql[name_end..]);
    Ok(out)
}

fn add_autoincrement_pk(sql: &str) -> Result<String, String> {
    let upper = sql.to_ascii_uppercase();
    let mut search = 0;
    while let Some(rel) = upper[search..].find("INTEGER") {
        let at = search + rel;
        let after_int = at + "INTEGER".len();
        let Some(primary_at) = match_sql_keyword(&upper, after_int, "PRIMARY") else {
            search = after_int;
            continue;
        };
        let Some(key_at) = match_sql_keyword(&upper, primary_at + "PRIMARY".len(), "KEY") else {
            search = after_int;
            continue;
        };
        let after_key = key_at + "KEY".len();
        let after_ws = skip_sql_ws(&upper, after_key);
        if upper[after_ws..].starts_with("AUTOINCREMENT") {
            return Ok(sql.to_string());
        }
        let mut out = String::new();
        out.push_str(&sql[..after_key]);
        out.push_str(" AUTOINCREMENT");
        out.push_str(&sql[after_key..]);
        return Ok(out);
    }
    Err(
        "users 表没有 INTEGER PRIMARY KEY，无法自动改为 AUTOINCREMENT。请先备份数据库并人工处理"
            .into(),
    )
}

fn match_sql_keyword(sql: &str, start: usize, keyword: &str) -> Option<usize> {
    let at = skip_sql_ws(sql, start);
    sql[at..].starts_with(keyword).then_some(at)
}

fn skip_sql_ws(sql: &str, mut index: usize) -> usize {
    while let Some(ch) = sql[index..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn read_sql_name(sql: &str, index: usize) -> Result<(usize, String), String> {
    let rest = &sql[index..];
    let invalid = "users 建表语句无法识别。请先备份数据库并人工处理";
    if let Some(quote) = rest.chars().next() {
        if quote == '"' || quote == '`' || quote == '[' {
            let end_ch = if quote == '[' { ']' } else { quote };
            let close = rest[1..].find(end_ch).ok_or_else(|| invalid.to_string())?;
            return Ok((
                index + 1 + close + 1,
                rest[1..1 + close].replace("\"\"", "\""),
            ));
        }
    }
    let end = rest
        .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .unwrap_or(rest.len());
    if end == 0 {
        return Err(invalid.into());
    }
    Ok((index + end, rest[..end].to_string()))
}

async fn add_column(pool: &SqlitePool, sql: &str) -> Result<(), sqlx::Error> {
    if let Err(err) = sqlx::raw_sql(sql).execute(pool).await {
        if !err.to_string().contains("duplicate column name") {
            return Err(err);
        }
    }
    Ok(())
}

async fn ensure_news_admin_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let sources: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('news_sources')")
            .fetch_all(pool)
            .await?;
    if !sources.is_empty() && !sources.iter().any(|column| column == "internal") {
        add_column(
            pool,
            "ALTER TABLE news_sources ADD COLUMN internal INTEGER NOT NULL DEFAULT 0",
        )
        .await?;
    }
    if !sources.is_empty() && !sources.iter().any(|column| column == "last_success_at") {
        add_column(
            pool,
            "ALTER TABLE news_sources ADD COLUMN last_success_at TEXT NOT NULL DEFAULT ''",
        )
        .await?;
    }
    if !sources.is_empty() && !sources.iter().any(|column| column == "platform") {
        add_column(
            pool,
            "ALTER TABLE news_sources ADD COLUMN platform TEXT NOT NULL DEFAULT ''",
        )
        .await?;
    }
    sqlx::query("UPDATE news_sources SET internal = 1 WHERE slug LIKE 'xincai-%' AND internal = 0")
        .execute(pool)
        .await?;
    let feeds: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('news_feeds')")
        .fetch_all(pool)
        .await?;
    let columns = [
        ("archived_at", "TEXT"),
        ("consecutive_failures", "INTEGER NOT NULL DEFAULT 0"),
        ("last_success_at", "TEXT NOT NULL DEFAULT ''"),
        ("last_error_detail", "TEXT NOT NULL DEFAULT ''"),
        ("normalized_url", "TEXT NOT NULL DEFAULT ''"),
    ];
    for (name, def) in columns {
        if feeds.iter().any(|column| column == name) {
            continue;
        }
        add_column(
            pool,
            &format!("ALTER TABLE news_feeds ADD COLUMN {name} {def}"),
        )
        .await?;
    }
    Ok(())
}

async fn ensure_news_article_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('news_articles')")
            .fetch_all(pool)
            .await?;
    if !existing.iter().any(|column| column == "content") {
        add_column(
            pool,
            "ALTER TABLE news_articles ADD COLUMN content TEXT NOT NULL DEFAULT ''",
        )
        .await?;
    }
    if existing.iter().any(|column| column == "content_html") {
        sqlx::query(
            "UPDATE news_articles SET content = content_html
             WHERE content = '' AND COALESCE(content_html, '') != ''",
        )
        .execute(pool)
        .await?;
    }
    let columns = [
        ("issue_key", "TEXT NOT NULL DEFAULT ''"),
        ("issue_label", "TEXT NOT NULL DEFAULT ''"),
        ("issue_title", "TEXT NOT NULL DEFAULT ''"),
        ("issue_cover", "TEXT NOT NULL DEFAULT ''"),
        ("section", "TEXT NOT NULL DEFAULT ''"),
        ("toc_order", "INTEGER NOT NULL DEFAULT 0"),
        ("images", "TEXT NOT NULL DEFAULT '[]'"),
        ("fetched_at", "TEXT NOT NULL DEFAULT ''"),
        ("content_hash", "TEXT NOT NULL DEFAULT ''"),
        ("topics", "TEXT NOT NULL DEFAULT '[]'"),
    ];
    for (name, def) in columns {
        if existing.iter().any(|column| column == name) {
            continue;
        }
        add_column(
            pool,
            &format!("ALTER TABLE news_articles ADD COLUMN {name} {def}"),
        )
        .await?;
    }
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS news_keyword_notified (
            user_id INTEGER NOT NULL, article_id INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, article_id))",
    )
    .execute(pool)
    .await?;
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS knowledge_keyword_notified (
            user_id INTEGER NOT NULL, group_id TEXT NOT NULL, media_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (user_id, group_id, media_id))",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_feishu_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('feishu_document_sources')")
            .fetch_all(pool)
            .await?;
    let columns = [
        ("asset_root", "TEXT NOT NULL DEFAULT ''"),
        ("source_type", "TEXT NOT NULL DEFAULT 'docx'"),
        ("display_name", "TEXT NOT NULL DEFAULT ''"),
        ("display_mode", "TEXT NOT NULL DEFAULT 'timeline'"),
        ("sync_status", "TEXT NOT NULL DEFAULT 'pending'"),
        ("entry_count", "INTEGER NOT NULL DEFAULT 0"),
        ("last_checked_at", "TEXT NOT NULL DEFAULT ''"),
        ("last_error", "TEXT NOT NULL DEFAULT ''"),
        ("source_token", "TEXT NOT NULL DEFAULT ''"),
        ("host", "TEXT NOT NULL DEFAULT ''"),
    ];
    for (name, def) in columns {
        if existing.is_empty() || existing.iter().any(|column| column == name) {
            continue;
        }
        add_column(
            pool,
            &format!("ALTER TABLE feishu_document_sources ADD COLUMN {name} {def}"),
        )
        .await?;
    }
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS feishu_oauth_sessions (state_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL, code_verifier TEXT NOT NULL, expires_at INTEGER NOT NULL)")
        .execute(pool)
        .await?;
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS feishu_oauth_credentials (id INTEGER PRIMARY KEY CHECK (id = 1), access_token TEXT NOT NULL, refresh_token TEXT NOT NULL DEFAULT '', expires_at INTEGER NOT NULL DEFAULT 0, refresh_expires_at INTEGER NOT NULL DEFAULT 0)")
        .execute(pool)
        .await?;
    Ok(())
}

async fn ensure_user_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let existing: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(pool)
        .await?;
    let columns = [
        ("telegram_chat_id", "TEXT NOT NULL DEFAULT ''"),
        ("telegram_bot_token", "TEXT NOT NULL DEFAULT ''"),
        ("feishu_open_id", "TEXT NOT NULL DEFAULT ''"),
        ("feishu_chat_id", "TEXT NOT NULL DEFAULT ''"),
        ("wecom_webhook", "TEXT NOT NULL DEFAULT ''"),
        ("bark_key", "TEXT NOT NULL DEFAULT ''"),
        ("notify_enabled", "INTEGER NOT NULL DEFAULT 1"),
        ("daily_report", "INTEGER NOT NULL DEFAULT 0"),
        ("translate_twitter", "INTEGER NOT NULL DEFAULT 1"),
        ("push_channels", "TEXT NOT NULL DEFAULT ''"),
        ("dnd_start", "TEXT NOT NULL DEFAULT ''"),
        ("dnd_end", "TEXT NOT NULL DEFAULT ''"),
        ("dnd_allow_favorite", "INTEGER NOT NULL DEFAULT 0"),
        ("keywords", "TEXT NOT NULL DEFAULT '[]'"),
        ("keywords_match_reports", "INTEGER NOT NULL DEFAULT 0"),
        ("keywords_match_reports_since", "TEXT NOT NULL DEFAULT ''"),
        ("keywords_match_news", "INTEGER NOT NULL DEFAULT 0"),
        ("keywords_match_news_since", "TEXT NOT NULL DEFAULT ''"),
        ("news_font_size", "TEXT NOT NULL DEFAULT ''"),
        ("llm_api_base", "TEXT NOT NULL DEFAULT ''"),
        ("llm_api_key", "TEXT NOT NULL DEFAULT ''"),
        ("llm_model", "TEXT NOT NULL DEFAULT ''"),
        ("llm_api_format", "TEXT NOT NULL DEFAULT 'chat'"),
        ("news_last_seen_at", "TEXT NOT NULL DEFAULT ''"),
        ("telegram_provisional", "INTEGER NOT NULL DEFAULT 0"),
        ("wechat_openid", "TEXT NOT NULL DEFAULT ''"),
    ];
    for (name, def) in columns {
        if existing.iter().any(|column| column == name) {
            continue;
        }
        add_column(pool, &format!("ALTER TABLE users ADD COLUMN {name} {def}")).await?;
    }
    sqlx::raw_sql("CREATE UNIQUE INDEX IF NOT EXISTS idx_users_wechat_openid ON users(wechat_openid) WHERE wechat_openid != ''")
        .execute(pool)
        .await?;
    Ok(())
}

async fn ensure_hot_indexes(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        "CREATE INDEX IF NOT EXISTS idx_push_logs_post ON push_logs(post_id, channel, user_id);
         CREATE INDEX IF NOT EXISTS idx_push_logs_created ON push_logs(created_at);
         CREATE INDEX IF NOT EXISTS idx_push_logs_user ON push_logs(user_id);
         CREATE INDEX IF NOT EXISTS idx_posts_fetched ON posts(fetched_at);
         CREATE INDEX IF NOT EXISTS idx_news_articles_published ON news_articles(published_at);",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_register_code_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let existing: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('register_codes')")
            .fetch_all(pool)
            .await?;
    let columns = [
        ("note", "TEXT NOT NULL DEFAULT ''"),
        ("created_at", "TEXT NOT NULL DEFAULT ''"),
        ("batch_id", "TEXT NOT NULL DEFAULT ''"),
        ("created_by", "INTEGER"),
    ];
    for (name, def) in columns {
        if existing.iter().any(|column| column == name) {
            continue;
        }
        add_column(
            pool,
            &format!("ALTER TABLE register_codes ADD COLUMN {name} {def}"),
        )
        .await?;
    }
    Ok(())
}

async fn ensure_kol_columns(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let existing: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('kols')")
        .fetch_all(pool)
        .await?;
    let columns = [
        ("recommend_weight", "INTEGER NOT NULL DEFAULT 0"),
        ("last_fetch_at", "TEXT NOT NULL DEFAULT ''"),
        ("last_fetch_error", "TEXT NOT NULL DEFAULT ''"),
        ("fetch_fail_streak", "INTEGER NOT NULL DEFAULT 0"),
        ("source_alerted", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, def) in columns {
        if existing.iter().any(|column| column == name) {
            continue;
        }
        add_column(pool, &format!("ALTER TABLE kols ADD COLUMN {name} {def}")).await?;
    }
    Ok(())
}

fn bind_code_text() -> Result<String, CatalogError> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    Ok(random_bytes(8)?
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect())
}

fn bind_code_digest(code: &str) -> String {
    let code = code.trim().to_ascii_uppercase();
    if code.is_empty() {
        return String::new();
    }
    use sha2::{Digest, Sha256};
    Sha256::digest(code.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random_invite_code() -> Result<String, CatalogError> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let bytes = random_bytes(8)?;
    Ok(bytes
        .iter()
        .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
        .collect())
}

fn random_hex(n: usize) -> Result<String, CatalogError> {
    Ok(random_bytes(n)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn random_bytes(n: usize) -> Result<Vec<u8>, CatalogError> {
    let mut buf = vec![0u8; n];
    let mut file =
        std::fs::File::open("/dev/urandom").map_err(|_| CatalogError::Bad("无法生成邀请码"))?;
    std::io::Read::read_exact(&mut file, &mut buf)
        .map_err(|_| CatalogError::Bad("无法生成邀请码"))?;
    Ok(buf)
}

fn user_from_row(row: sqlx::sqlite::SqliteRow) -> User {
    let flag = |name| row.get::<i64, _>(name) != 0;
    User {
        id: row.get("id"),
        username: row.get("username"),
        password_hash: row.get("password_hash"),
        is_admin: flag("is_admin"),
        token_version: row.get("token_version"),
        created_at: row.get("created_at"),
        telegram_chat_id: row.get("telegram_chat_id"),
        telegram_bot_token: row.get("telegram_bot_token"),
        feishu_open_id: row.get("feishu_open_id"),
        feishu_chat_id: row.get("feishu_chat_id"),
        wecom_webhook: row.get("wecom_webhook"),
        bark_key: row.get("bark_key"),
        notify_enabled: flag("notify_enabled"),
        daily_report: flag("daily_report"),
        translate_twitter: flag("translate_twitter"),
        push_channels: row.get("push_channels"),
        dnd_start: row.get("dnd_start"),
        dnd_end: row.get("dnd_end"),
        dnd_allow_favorite: flag("dnd_allow_favorite"),
        keywords: row.get("keywords"),
        keywords_match_reports: flag("keywords_match_reports"),
        keywords_match_news: flag("keywords_match_news"),
        news_font_size: row.get("news_font_size"),
        llm_api_base: row.get("llm_api_base"),
        llm_api_key: row.get("llm_api_key"),
        llm_model: row.get("llm_model"),
        llm_api_format: row.get("llm_api_format"),
        wechat_openid: row.get("wechat_openid"),
    }
}

#[derive(Debug)]
pub enum RegisterError {
    Rejected(&'static str),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for RegisterError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

#[derive(Debug)]
pub struct XincaiRow {
    pub key: String,
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub external_id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub url: String,
    pub author: String,
    pub published_at: String,
    pub issue_key: String,
    pub issue_label: String,
    pub issue_title: String,
    pub issue_cover: String,
    pub section: String,
    pub toc_order: i64,
    pub images: String,
    pub topics: String,
    pub platform: String,
    pub fetched_at: String,
    pub content_hash: String,
}

#[derive(Debug)]
pub struct KolPatch<'a> {
    pub name: Option<&'a str>,
    pub external_id: Option<&'a str>,
    pub enabled: Option<bool>,
    pub category_id: Option<Option<i64>>,
    pub priority: Option<bool>,
    pub secondary: Option<bool>,
    pub is_private: Option<bool>,
    pub original_only: Option<bool>,
    pub recommend_weight: Option<i64>,
    pub visible_users: Option<&'a [String]>,
}

fn feishu_identity(value: &str) -> Result<&str, CatalogError> {
    let value = value.trim();
    if (1..=80).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        Ok(value)
    } else {
        Err(CatalogError::Bad("飞书身份无效"))
    }
}

#[derive(Debug)]
pub enum CatalogError {
    Missing(&'static str),
    Bad(&'static str),
    Invalid(String),
    Limited(&'static str),
    Conflict(&'static str),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for CatalogError {
    fn from(err: sqlx::Error) -> Self {
        Self::Db(err)
    }
}

fn dnd_active(start: &str, end: &str) -> bool {
    let Some(start) = clock_minutes(start) else {
        return false;
    };
    let Some(end) = clock_minutes(end) else {
        return false;
    };
    if start == end {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let now = ((now + 8 * 3600) % 86400 / 60) as u32;
    if start < end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

fn clock_minutes(value: &str) -> Option<u32> {
    let (hour, minute) = value.split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

fn keyword_hits(raw: &str, text: &str) -> Vec<String> {
    let Ok(list) = serde_json::from_str::<Vec<String>>(raw) else {
        return Vec::new();
    };
    let text = text.to_lowercase();
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for keyword in list {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        let key = keyword.to_lowercase();
        if !seen.insert(key.clone()) || !text.contains(&key) {
            continue;
        }
        hits.push(keyword.to_string());
    }
    hits
}

fn clip_title(title: &str, fallback: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { fallback } else { title };
    let chars: Vec<char> = title.chars().collect();
    if chars.len() > 80 {
        format!("{}…", chars[..79].iter().collect::<String>())
    } else {
        title.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[tokio::test]
    async fn open_adds_hot_indexes_to_existing_databases_and_syncs_normal() {
        let path = std::env::temp_dir().join(format!(
            "vpush-hot-indexes-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let names = [
            "idx_news_articles_published",
            "idx_posts_fetched",
            "idx_push_logs_created",
            "idx_push_logs_post",
            "idx_push_logs_user",
        ];
        let db = Db::open(&path).await.unwrap();
        for name in names {
            sqlx::query(&format!("DROP INDEX {name}"))
                .execute(db.pool())
                .await
                .unwrap();
        }
        db.pool().close().await;
        let db = Db::open(&path).await.unwrap();
        let found: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name IN
             ('idx_news_articles_published', 'idx_posts_fetched', 'idx_push_logs_created',
              'idx_push_logs_post', 'idx_push_logs_user') ORDER BY name",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(found, names);
        let plan = |sql: &'static str| {
            let db = db.clone();
            async move {
                sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                    .fetch_all(db.pool())
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|row| row.get::<String, _>("detail"))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        };
        let failed = plan("SELECT COUNT(*) FROM push_logs WHERE post_id = 1 AND channel = 'telegram' AND user_id = 2 AND status = 'failed'").await;
        assert!(failed.contains("idx_push_logs_post"), "{failed}");
        let expired =
            plan("SELECT id FROM push_logs WHERE created_at < datetime('now', '-90 days')").await;
        assert!(expired.contains("idx_push_logs_created"), "{expired}");
        let owned = plan("SELECT 1 FROM push_logs WHERE user_id = 3").await;
        assert!(owned.contains("idx_push_logs_user"), "{owned}");
        let old_posts =
            plan("SELECT id FROM posts WHERE fetched_at < datetime('now', '-30 days')").await;
        assert!(old_posts.contains("idx_posts_fetched"), "{old_posts}");
        let old_news = plan("SELECT id FROM news_articles WHERE published_at != '' AND published_at < datetime('now', '-30 days')").await;
        assert!(
            old_news.contains("idx_news_articles_published"),
            "{old_news}"
        );
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(synchronous, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn telegram_token_uniqueness_compares_decrypted_enc2_values() {
        let _env_lock = crate::feishu_personal::TEST_ENV_LOCK
            .get_or_init(|| async { tokio::sync::Mutex::new(()) })
            .await
            .lock()
            .await;
        let key = base64::engine::general_purpose::URL_SAFE.encode([8u8; 32]);
        let previous = std::env::var_os("FEISHU_CREDENTIAL_KEY");
        std::env::set_var("FEISHU_CREDENTIAL_KEY", &key);
        let path = std::env::temp_dir().join(format!(
            "vpush-telegram-unique-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let other_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let token = "123456:ABCDEFGHIJKLMNOPQRST";
        let first = crate::feishu_personal::seal(&key, token).unwrap();
        let second = crate::feishu_personal::seal(&key, token).unwrap();
        assert_ne!(first, second);
        sqlx::query("INSERT INTO users (username, password_hash, telegram_bot_token) VALUES ('other', 'hash', ?)")
            .bind(format!("enc2:{first}"))
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .other_user_has("telegram_bot_token", token, other_id)
            .await
            .unwrap());
        sqlx::query("UPDATE users SET telegram_bot_token = ? WHERE username = 'other'")
            .bind(format!("enc2:{second}"))
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .other_user_has("telegram_bot_token", token, other_id)
            .await
            .unwrap());
        let _ = std::fs::remove_file(&path);
        match previous {
            Some(value) => std::env::set_var("FEISHU_CREDENTIAL_KEY", value),
            None => std::env::remove_var("FEISHU_CREDENTIAL_KEY"),
        }
    }

    #[tokio::test]
    async fn ticker_page_hides_digest_the_reader_cannot_fully_see() {
        let path = std::env::temp_dir().join(format!(
            "vpush-ticker-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        for (group, media, day) in [
            ("reports", "m1", "2026-08-02"),
            ("secret", "m2", "2026-08-01"),
        ] {
            sqlx::query("INSERT INTO ima_document_index (group_id, media_id, name, sort_date) VALUES (?, ?, '研报', ?)")
                .bind(group).bind(media).bind(day).execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO report_extraction_tickers (group_id, media_id, code, name) VALUES (?, ?, '600000', '浦发银行')")
                .bind(group).bind(media).execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO report_extractions (group_id, media_id, rating, thesis) VALUES (?, ?, '买入', '要点')")
                .bind(group).bind(media).execute(db.pool()).await.unwrap();
        }
        sqlx::query("INSERT INTO ima_ticker_digests (kind, code, name, source_count, digest, status, updated_at) VALUES ('ticker', '600000', '浦发银行', 2, ?, 'ok', '2026-08-02 10:00')")
            .bind(r#"{"consensus":"一致看多","divergence":"估值","evolution":[{"date":"2026-08-01","point":"上调"}]}"#)
            .execute(db.pool()).await.unwrap();
        let full = db
            .ima_ticker_page("浦发银行", &["reports".into(), "secret".into()], 100)
            .await
            .unwrap();
        assert_eq!(full["code"], "600000");
        assert_eq!(full["name"], "浦发银行");
        assert_eq!(full["count"], 2);
        assert_eq!(full["items"][0]["sort_date"], "2026-08-02");
        assert_eq!(full["digest"]["consensus"], "一致看多");
        let partial = db
            .ima_ticker_page("600000", &["reports".into()], 100)
            .await
            .unwrap();
        assert_eq!(partial["count"], 1);
        assert!(partial["digest"].as_object().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn patch_kol_keeps_unspecified_fields_and_switches_tier() {
        let path = std::env::temp_dir().join(format!(
            "vpush-patch-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let category = db.add_category("实盘").await.unwrap();
        let id = db
            .add_kol("xueqiu", "旧名", "111", Some(category), true, false, false)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin) VALUES ('Alice', 'x', 0)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        let users = vec!["alice".to_string()];
        db.patch_kol(
            id,
            KolPatch {
                name: Some("段永平"),
                external_id: None,
                enabled: None,
                category_id: None,
                priority: None,
                secondary: None,
                is_private: Some(true),
                original_only: None,
                recommend_weight: Some(5),
                visible_users: Some(&users),
            },
        )
        .await
        .unwrap();
        let kol = db.kol_for(1, true, id).await.unwrap().unwrap();
        assert_eq!(kol["name"], "段永平");
        assert_eq!(kol["priority"], true);
        assert_eq!(kol["category_id"], category);
        assert_eq!(kol["recommend_weight"], 5);
        assert_eq!(kol["visible_users"][0], "Alice");
        db.patch_kol(
            id,
            KolPatch {
                name: None,
                external_id: None,
                enabled: Some(false),
                category_id: None,
                priority: None,
                secondary: Some(true),
                is_private: None,
                original_only: None,
                recommend_weight: None,
                visible_users: None,
            },
        )
        .await
        .unwrap();
        let kol = db.kol_for(1, true, id).await.unwrap().unwrap();
        let listed = db.admin_kol_page("", 0, "", None, 50, 0).await.unwrap();
        assert_eq!(kol["enabled"], false);
        assert_eq!(kol["priority"], false);
        assert_eq!(listed["items"][0]["secondary"], true);
        assert_eq!(kol["name"], "段永平");
        assert_eq!(kol["visible_users"][0], "Alice");
        let err = db
            .patch_kol(
                id,
                KolPatch {
                    name: None,
                    external_id: None,
                    enabled: None,
                    category_id: None,
                    priority: None,
                    secondary: None,
                    is_private: None,
                    original_only: None,
                    recommend_weight: None,
                    visible_users: Some(&["missing".into()]),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, CatalogError::Invalid(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn admin_kol_page_filters_without_hiding_disabled() {
        let path = std::env::temp_dir().join(format!(
            "vpush-kols-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let first = db
            .add_kol("xueqiu", "段永平", "111", None, true, false, false)
            .await
            .unwrap();
        let second = db
            .add_kol("weibo", "乙", "222", None, false, true, false)
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET enabled = 0 WHERE id = ?")
            .bind(second)
            .execute(db.pool())
            .await
            .unwrap();
        db.subscribe(1, true, first, "post").await.unwrap();
        let page = db.admin_kol_page("", 0, "", None, 1, 0).await.unwrap();
        assert_eq!(page["total"], 2);
        assert_eq!(page["ids"], serde_json::json!([first, second]));
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["subscriber_count"], 1);
        assert_eq!(page["items"][0]["priority"], true);
        let weibo = db
            .admin_kol_page("weibo", 0, "", Some(0), 50, 0)
            .await
            .unwrap();
        assert_eq!(weibo["total"], 1);
        assert_eq!(weibo["items"][0]["secondary"], true);
        assert_eq!(weibo["items"][0]["enabled"], false);
        let named = db.admin_kol_page("", 0, "段%", None, 50, 0).await.unwrap();
        assert_eq!(named["total"], 0);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn invite_is_single_use_and_bad_name_rolls_back() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO register_codes (code) VALUES ('INVITE1'), ('INVITE2')")
            .execute(&db.pool)
            .await
            .unwrap();
        db.ensure_admin("hash").await.unwrap();
        let taken = db.register("invite2", "admin", "hash").await;
        assert!(matches!(
            taken,
            Err(RegisterError::Rejected("用户名已存在"))
        ));
        let id = db.register("invite2", "abcdef", "hash").await.unwrap();
        assert!(id > 0);
        let again = db.register("INVITE2", "other1", "hash").await;
        assert!(matches!(
            again,
            Err(RegisterError::Rejected("邀请码无效或已被使用"))
        ));
        let missing = db.register("NOPE", "other2", "hash").await;
        assert!(matches!(
            missing,
            Err(RegisterError::Rejected("邀请码无效或已被使用"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn external_id_keeps_the_last_segment() {
        assert_eq!(
            normalize_external_id("xueqiu", "https://xueqiu.com/u/1234567890?from=timeline"),
            "1234567890"
        );
        assert_eq!(
            normalize_external_id("twitter", "https://x.com/Some_One"),
            "Some_One"
        );
        assert_eq!(
            normalize_external_id("combination", "https://xueqiu.com/P/ZH123"),
            "ZH123"
        );
    }

    #[tokio::test]
    async fn feed_hides_secondary_and_private() {
        let path = std::env::temp_dir().join(format!(
            "vpush-feed-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let cat = db.add_category("宏观").await.unwrap();
        let main = db
            .add_kol(
                "xueqiu",
                "甲",
                "https://xueqiu.com/u/111",
                Some(cat),
                false,
                false,
                false,
            )
            .await
            .unwrap();
        let side = db
            .add_kol("xueqiu", "乙", "222", Some(cat), false, true, false)
            .await
            .unwrap();
        let hidden = db
            .add_kol(
                "weibo",
                "丙",
                "https://weibo.com/u/333",
                None,
                false,
                false,
                false,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE kols SET is_private = 1 WHERE id = ?")
            .bind(hidden)
            .execute(&db.pool)
            .await
            .unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader', 'x')")
            .execute(&db.pool)
            .await
            .unwrap();
        let reader = db.user_by_username("reader").await.unwrap().unwrap();
        db.subscribe(admin.id, true, main, "post").await.unwrap();
        db.subscribe(admin.id, true, side, "post").await.unwrap();
        db.insert_post(main, "p1", "宏观正文", "2026-09-26 12:00", "[\"宏观\"]")
            .await
            .unwrap();
        db.insert_post(side, "p2", "次要", "2026-09-26 12:01", "[]")
            .await
            .unwrap();
        let open = FeedFilter {
            limit: 20,
            offset: 0,
            platform: "",
            category_id: 0,
            q: "",
            favorite: false,
            tag: "",
            include_secondary: false,
            since_id: 0,
        };
        let posts = db.feed(admin.id, true, &open).await.unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["content"], "宏观正文");
        assert_eq!(posts[0]["tags"][0], "宏观");
        let mut with_side = open;
        with_side.include_secondary = true;
        assert_eq!(db.feed(admin.id, true, &with_side).await.unwrap().len(), 2);
        assert!(db.should_push(main, "post").await.unwrap());
        assert!(!db.should_push(main, "reply").await.unwrap());
        assert!(!db.should_push(side, "post").await.unwrap());
        db.set_favorite(admin.id, side, true).await.unwrap();
        assert!(db.should_push(side, "post").await.unwrap());
        assert_eq!(db.feed(admin.id, true, &open).await.unwrap().len(), 2);
        let tagged = FeedFilter {
            tag: "宏观",
            ..open
        };
        assert_eq!(db.feed(admin.id, true, &tagged).await.unwrap().len(), 1);
        let plaza = db.catalog(reader.id, false, "", 0).await.unwrap();
        assert!(plaza.iter().all(|row| row["name"] != "丙"));
        let admin_plaza = db.catalog(admin.id, true, "", 0).await.unwrap();
        assert!(admin_plaza.iter().any(|row| row["name"] == "丙"));
        assert!(db
            .subscribe(reader.id, false, hidden, "post")
            .await
            .is_err());
        sqlx::query("UPDATE posts SET images = ? WHERE external_id = 'p1'")
            .bind(r#"["https://img.example/a.jpg"]"#)
            .execute(&db.pool)
            .await
            .unwrap();
        let shown = db.feed(admin.id, true, &open).await.unwrap();
        let main_post = shown
            .iter()
            .find(|row| row["content"] == "宏观正文")
            .unwrap();
        assert_eq!(main_post["images"][0], "https://img.example/a.jpg");
        db.set_hide_images(admin.id, main, true).await.unwrap();
        let hidden_imgs = db.feed(admin.id, true, &open).await.unwrap();
        let main_post = hidden_imgs
            .iter()
            .find(|row| row["content"] == "宏观正文")
            .unwrap();
        assert_eq!(main_post["images"].as_array().unwrap().len(), 0);
        let subs = db.my_subscriptions(admin.id).await.unwrap();
        let row = subs.iter().find(|row| row["id"] == main).unwrap();
        assert_eq!(row["hide_images"], true);
        assert_eq!(row["name"], "甲");
        assert!(db.set_hide_images(reader.id, main, true).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn profile_fields_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "vpush-profile-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let user = db.user_by_username("admin").await.unwrap().unwrap();
        assert!(user.notify_enabled);
        assert!(user.translate_twitter);
        db.set_user_flag(user.id, "notify_enabled", false)
            .await
            .unwrap();
        db.set_user_text(user.id, "push_channels", "wecom,bark")
            .await
            .unwrap();
        db.set_user_text(user.id, "keywords", r#"["宏观"]"#)
            .await
            .unwrap();
        db.set_user_text(
            user.id,
            "wecom_webhook",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc",
        )
        .await
        .unwrap();
        let saved = db.user_by_id(user.id).await.unwrap().unwrap();
        assert!(!saved.notify_enabled);
        assert_eq!(saved.push_channels, "wecom,bark");
        assert_eq!(saved.keywords, r#"["宏观"]"#);
        let kol = db
            .add_kol("xueqiu", "甲", "1", None, false, false, false)
            .await
            .unwrap();
        db.subscribe(user.id, true, kol, "post").await.unwrap();
        let targets = db.push_targets(kol).await.unwrap();
        assert_eq!(targets.len(), 1);
        assert!(!targets[0].notify_enabled);
        assert!(targets[0].wecom_webhook.contains("qyapi"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn android_devices_are_upserted_and_user_scoped() {
        let path = std::env::temp_dir().join(format!(
            "vpush-android-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        db.upsert_android_device("installation-1", admin.id, "token", "fcm", "Pixel", "1.0")
            .await
            .unwrap();
        assert_eq!(db.android_device_count(admin.id).await.unwrap(), 1);
        db.upsert_android_device(
            "installation-1",
            admin.id,
            "token-2",
            "fcm",
            "Pixel 2",
            "2.0",
        )
        .await
        .unwrap();
        assert_eq!(db.android_device_count(admin.id).await.unwrap(), 1);
        db.delete_android_device("installation-1", admin.id + 1)
            .await
            .unwrap();
        assert_eq!(db.android_device_count(admin.id).await.unwrap(), 1);
        db.delete_android_device("installation-1", admin.id)
            .await
            .unwrap();
        assert_eq!(db.android_device_count(admin.id).await.unwrap(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn kol_request_links_are_normalized() {
        assert_eq!(
            normalize_kol_request("xueqiu", "https://xueqiu.com/u/4514680565").unwrap(),
            "4514680565"
        );
        assert_eq!(
            normalize_kol_request("combination", "ZH123456").unwrap(),
            "ZH123456"
        );
        assert_eq!(
            normalize_kol_request("twitter", "@some_user").unwrap(),
            "some_user"
        );
        let switched = normalize_kol_request("weibo", "https://xueqiu.com/u/1").unwrap_err();
        assert!(switched.contains("雪球") && switched.contains("微博"));
        assert!(normalize_kol_request("twitter", "https://x.com/home").is_err());
        assert!(normalize_kol_request("truth", "123").is_err());
        assert_eq!(zsxq_cookie_value("zsxq_access_token=abc"), "abc");
    }

    #[tokio::test]
    async fn stats_hides_cookie_and_saves_poll_interval() {
        let path = std::env::temp_dir().join(format!(
            "vpush-stats-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.save_cookie("xueqiu_cookie", "xqsecret").await.unwrap();
        let stats = db.admin_stats().await.unwrap();
        assert_eq!(stats["xueqiu_cookie"]["set"], true);
        assert_eq!(stats["xueqiu_cookie"]["preview"], "已配置");
        assert!(!stats.to_string().contains("xqsecret"));
        assert_eq!(stats["polling_config"]["interval_seconds"], 180);
        let mut body = serde_json::Map::new();
        body.insert("interval_seconds".into(), json!(90));
        let saved = db.update_polling(&body).await.unwrap();
        assert_eq!(saved["interval_seconds"], 90);
        body.insert("interval_seconds".into(), json!(0));
        assert!(db.update_polling(&body).await.is_err());
        db.clear_cookie("xueqiu").await.unwrap();
        let cleared = db.admin_stats().await.unwrap();
        assert_eq!(cleared["xueqiu_cookie"]["set"], false);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn admin_can_rename_and_cannot_delete_self() {
        let path = std::env::temp_dir().join(format!(
            "vpush-users-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, created_at) VALUES ('reader01', 'x', '2000-01-01 00:00:00')")
            .execute(&db.pool)
            .await
            .unwrap();
        let reader = db.user_by_username("reader01").await.unwrap().unwrap();
        sqlx::query("INSERT INTO ima_document_index (group_id, media_id, day, name) VALUES ('reports', 'm1', '2026-08-01', 'a.pdf')")
            .execute(&db.pool)
            .await
            .unwrap();
        db.set_ima_kb_acl_for_user(reader.id, &["reports".into()])
            .await
            .unwrap();
        assert_eq!(
            db.ima_kb_group_ids_for_user(reader.id).await.unwrap(),
            vec!["reports".to_string()]
        );
        assert_eq!(
            db.ima_kb_subscribed_group_ids_for_user(reader.id)
                .await
                .unwrap(),
            vec!["reports".to_string()]
        );
        db.set_ima_kb_acl_for_user(reader.id, &[]).await.unwrap();
        assert!(db
            .ima_kb_group_ids_for_user(reader.id)
            .await
            .unwrap()
            .is_empty());
        assert!(db
            .ima_kb_subscribed_group_ids_for_user(reader.id)
            .await
            .unwrap()
            .is_empty());
        let listed = db.list_admin_users().await.unwrap();
        assert!(listed.iter().any(|row| row["username"] == "reader01"
            && row["inactive"] == true
            && row["ima_kb_groups"].as_array().unwrap().is_empty()));
        assert!(listed
            .iter()
            .all(|row| row.get("password_hash").is_none() && row.get("wecom_webhook").is_none()));
        assert!(db.delete_user(admin.id, admin.id).await.is_err());
        assert!(db
            .update_admin_user(admin.id, admin.id, None, None, Some(false))
            .await
            .is_err());
        db.update_admin_user(
            admin.id,
            reader.id,
            Some("reader02"),
            Some("new-hash"),
            None,
        )
        .await
        .unwrap();
        let renamed = db.user_by_username("reader02").await.unwrap().unwrap();
        assert_eq!(renamed.token_version, 1);
        db.delete_user(admin.id, reader.id).await.unwrap();
        assert!(db.user_by_username("reader02").await.unwrap().is_none());
        let policy = db.set_inactive_policy(10, 5).await.unwrap();
        assert_eq!(policy["inactive_after_days"], 10);
        assert!(db.set_inactive_policy(4000, 1).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn hidden_plaza_platform_leaves_the_feed() {
        let path = std::env::temp_dir().join(format!(
            "vpush-plaza-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        let snow = db
            .add_kol("xueqiu", "甲", "1", None, false, false, false)
            .await
            .unwrap();
        let blog = db
            .add_kol("weibo", "乙", "2", None, false, false, false)
            .await
            .unwrap();
        db.subscribe(admin.id, true, snow, "post").await.unwrap();
        db.subscribe(admin.id, true, blog, "post").await.unwrap();
        db.insert_post(snow, "a", "雪球", "2026-09-26 12:00", "[]")
            .await
            .unwrap();
        db.insert_post(blog, "b", "微博", "2026-09-26 12:01", "[]")
            .await
            .unwrap();
        let open = FeedFilter {
            limit: 20,
            offset: 0,
            platform: "",
            category_id: 0,
            q: "",
            favorite: false,
            tag: "",
            include_secondary: true,
            since_id: 0,
        };
        assert_eq!(db.feed(admin.id, true, &open).await.unwrap().len(), 2);
        let weibo = db
            .plaza_sources()
            .await
            .unwrap()
            .into_iter()
            .find(|row| row["platform"] == "weibo")
            .unwrap();
        assert_eq!(weibo["visible"], true);
        let mut update = serde_json::Map::new();
        update.insert("weibo".into(), json!("hide"));
        db.set_plaza_visibility(&update).await.unwrap();
        let hidden = db
            .plaza_sources()
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row["visible"] == false)
            .map(|row| row["platform"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        let posts = db.feed(admin.id, true, &open).await.unwrap();
        let posts = posts
            .into_iter()
            .filter(|row| !hidden.iter().any(|platform| row["platform"] == *platform))
            .collect::<Vec<_>>();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0]["content"], "雪球");
        update.insert("ima".into(), json!("show"));
        assert!(db.set_plaza_visibility(&update).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn news_article_can_be_marked_read() {
        let path = std::env::temp_dir().join(format!(
            "vpush-news-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        let source = db.add_news_source("甲报", "宏观").await.unwrap();
        let feed = db
            .add_news_feed(source, "甲", "https://example.com/rss")
            .await
            .unwrap();
        let added = db
            .save_news_entries(
                source,
                feed,
                &[NewsEntry {
                    external_id: "a1".into(),
                    title: "标题".into(),
                    summary: "摘要".into(),
                    content: "正文".into(),
                    url: "https://example.com/a".into(),
                    author: "".into(),
                    published_at: "2026-09-26 12:30".into(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(added, 1);
        let page = db.list_news(admin.id, 0, "", false, 10, 0).await.unwrap();
        assert_eq!(page["items"][0]["is_read"], false);
        let id = page["items"][0]["id"].as_i64().unwrap();
        assert!(db.mark_news_read(admin.id, id).await.unwrap());
        let article = db.news_article(admin.id, id).await.unwrap().unwrap();
        assert_eq!(article["is_read"], true);
        assert_eq!(article["content"], "正文");
        sqlx::query("ALTER TABLE news_articles RENAME COLUMN content TO content_html")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE news_articles SET content_html = '<p>旧正文</p>' WHERE id = ?")
            .bind(id)
            .execute(db.pool())
            .await
            .unwrap();
        ensure_news_article_columns(db.pool()).await.unwrap();
        let restored = db.news_article(admin.id, id).await.unwrap().unwrap();
        assert_eq!(restored["content"], "<p>旧正文</p>");
        ensure_news_article_columns(db.pool()).await.unwrap();
        let sources = db.user_news_sources(admin.id).await.unwrap();
        assert_eq!(sources["items"][0]["unread_count"], 0);
        let other = db.add_news_source("乙报", "").await.unwrap();
        let other_feed = db
            .add_news_feed(other, "乙", "https://example.com/b")
            .await
            .unwrap();
        db.save_news_entries(
            other,
            other_feed,
            &[NewsEntry {
                external_id: "b1".into(),
                title: "另一条".into(),
                summary: "".into(),
                content: "".into(),
                url: "https://example.com/b".into(),
                author: "".into(),
                published_at: "2026-09-26 13:00".into(),
            }],
        )
        .await
        .unwrap();
        assert_eq!(
            db.list_news(admin.id, 0, "", false, 10, 0).await.unwrap()["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        db.set_user_news_sources(admin.id, &[source]).await.unwrap();
        let kept = db.list_news(admin.id, 0, "", false, 10, 0).await.unwrap();
        assert_eq!(kept["items"].as_array().unwrap().len(), 1);
        assert_eq!(kept["items"][0]["title"], "标题");
        assert!(db.news_article(admin.id, id).await.unwrap().is_some());
        assert!(db.set_user_news_sources(admin.id, &[999]).await.is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn kol_request_approve_subscribes_and_reject_is_once() {
        let path = std::env::temp_dir().join(format!(
            "vpush-ask-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let cat = db.add_category("宏观").await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader', 'x')")
            .execute(&db.pool)
            .await
            .unwrap();
        let reader = db.user_by_username("reader").await.unwrap().unwrap();
        let id = db
            .add_kol_request(
                "xueqiu",
                "https://xueqiu.com/u/111",
                reader.id,
                "",
                Some(cat),
            )
            .await
            .unwrap();
        assert!(db
            .add_kol_request("xueqiu", "111", reader.id, "", Some(cat))
            .await
            .is_err());
        let kol = db.approve_kol_request(id).await.unwrap();
        assert!(db.approve_kol_request(id).await.is_err());
        let subs = db.my_subscriptions(reader.id).await.unwrap();
        assert_eq!(subs[0]["id"], kol);
        assert_eq!(subs[0]["name"], "xueqiu_111");
        let mine = db.list_kol_requests("", reader.id).await.unwrap();
        assert_eq!(mine[0]["status"], "approved");
        assert_eq!(mine[0]["category_name"], "宏观");
        assert_eq!(mine[0]["requester"], "reader");
        let second = db
            .add_kol_request("weibo", "222", reader.id, "乙", Some(cat))
            .await
            .unwrap();
        db.reject_kol_request(second).await.unwrap();
        assert!(db.reject_kol_request(second).await.is_err());
        assert!(db
            .add_kol_request("xueqiu", "111", reader.id, "", Some(cat))
            .await
            .is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn kol_request_transition_rolls_back_and_records_actor_atomically() {
        let db = Db::open(std::path::Path::new(":memory:")).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader', 'x')")
            .execute(db.pool())
            .await
            .unwrap();
        let reader = db.user_by_username("reader").await.unwrap().unwrap();
        let failing = db
            .add_kol_request_without_category("xueqiu", "900", reader.id, "")
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER fail_kol_request_subscription
             BEFORE INSERT ON subscriptions WHEN NEW.user_id = {}
             BEGIN SELECT RAISE(ABORT, 'forced subscription failure'); END",
            reader.id
        ))
        .execute(db.pool())
        .await
        .unwrap();

        assert!(db
            .approve_kol_request_as(failing, None, admin.id)
            .await
            .is_err());
        assert!(db.kol_request_pending(failing).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM kols WHERE platform = 'xueqiu' AND external_id = '900'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM subscriptions WHERE user_id = ?",)
                .bind(reader.id)
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0
        );
        assert!(db.list_admin_logs(20).await.unwrap().is_empty());
        sqlx::query("DROP TRIGGER fail_kol_request_subscription")
            .execute(db.pool())
            .await
            .unwrap();

        let approved = db
            .approve_kol_request_as(failing, None, admin.id)
            .await
            .unwrap();
        assert!(approved.kol_id.is_some());
        let logs = db.list_admin_logs(20).await.unwrap();
        assert!(logs
            .iter()
            .any(|log| { log["user_id"] == admin.id && log["action"] == "approve_kol_request" }));

        let rejected = db
            .add_kol_request_without_category("weibo", "901", reader.id, "")
            .await
            .unwrap();
        db.reject_kol_request_as(rejected, admin.id).await.unwrap();
        assert!(db
            .approve_kol_request_as(rejected, None, admin.id)
            .await
            .is_err());
        let logs = db.list_admin_logs(20).await.unwrap();
        assert!(logs
            .iter()
            .any(|log| { log["user_id"] == admin.id && log["action"] == "reject_kol_request" }));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM kols WHERE platform = 'weibo' AND external_id = '901'",
            )
            .fetch_one(db.pool())
            .await
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn cicc_batch_counts_rows_from_the_same_second() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.insert_ima_document("local-cicc-research", "a", "一", "2020-01-01", "", "")
            .await
            .unwrap();
        db.insert_ima_document("local-cicc-research", "b", "二", "2020-01-01", "", "")
            .await
            .unwrap();
        sqlx::query("UPDATE ima_document_index SET downloaded_at = '2020-01-01T00:00:00+00:00'")
            .execute(&db.pool)
            .await
            .unwrap();
        let (stamp, count) = db.ima_latest_batch("local-cicc-research").await.unwrap();
        assert_eq!(stamp, "2020-01-01T00:00:00+00:00");
        assert_eq!(count, 2);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn category_rename_delete_and_admin_lists() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let cat = db.add_category("宏观").await.unwrap();
        let other = db.add_category("其他").await.unwrap();
        assert!(matches!(
            db.rename_category(cat, "其他").await,
            Err(CatalogError::Bad("分类已存在"))
        ));
        let renamed = db.rename_category(cat, "策略").await.unwrap();
        assert_eq!(renamed["name"], "策略");
        let kol = db
            .add_kol("xueqiu", "甲", "1", Some(cat), false, false, false)
            .await
            .unwrap();
        db.insert_post(kol, "p1", "百分号 % 正文", "2020-01-02 00:00", "[]")
            .await
            .unwrap();
        let hit = db
            .list_posts(10, 0, "xueqiu", Some(kol), "%")
            .await
            .unwrap();
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0]["kol_name"], "甲");
        assert_eq!(hit[0]["category_name"], "策略");
        let miss = db.list_posts(10, 0, "weibo", None, "").await.unwrap();
        assert!(miss.is_empty());
        let post_id = hit[0]["id"].as_i64().unwrap();
        db.add_push_log(post_id, "telegram", "failed", "timeout", None)
            .await
            .unwrap();
        let logs = db
            .list_push_logs(10, None, "telegram", "failed")
            .await
            .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["kol_name"], "甲");
        assert!(logs[0]["user_name"].is_null());
        let hidden = db.list_push_logs(10, None, "wecom", "").await.unwrap();
        assert!(hidden.is_empty());
        db.delete_category(cat).await.unwrap();
        let category_id: Option<i64> =
            sqlx::query_scalar("SELECT category_id FROM kols WHERE id = ?")
                .bind(kol)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert!(category_id.is_none());
        assert!(matches!(
            db.delete_category(cat).await,
            Err(CatalogError::Missing("分类不存在"))
        ));
        assert!(db.category(other).await.unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn invite_codes_can_be_issued_revoked_and_purged() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let made = db
            .generate_register_codes(2, "内测", Some(7), admin)
            .await
            .unwrap();
        assert_eq!(made["count"], 2);
        assert_eq!(made["note"], "内测");
        assert!(made["expires_at"].as_str().unwrap().len() >= 10);
        let codes: Vec<String> = serde_json::from_value(made["codes"].clone()).unwrap();
        let listed = db.list_register_codes().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .all(|row| row["used_by"].is_null() && row["note"] == "内测"));
        db.register(&codes[0], "member", "hash").await.unwrap();
        assert!(matches!(
            db.revoke_register_code(&codes[0]).await,
            Err(CatalogError::Bad("该注册码已被使用，不能删除"))
        ));
        db.revoke_register_code(&codes[1]).await.unwrap();
        let (deleted, skipped) = db.register_codes_batch("delete", &codes).await.unwrap();
        assert_eq!(deleted, 2);
        assert_eq!(skipped, 0);
        assert!(db.list_register_codes().await.unwrap().is_empty());
        assert!(matches!(
            db.generate_register_codes(1, "x", Some(3), admin).await,
            Err(CatalogError::Bad("有效期需为 1、7、30 天或永不过期"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn dashboard_counts_people_posts_and_pushes() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let codes = db
            .generate_register_codes(1, "", Some(7), admin)
            .await
            .unwrap();
        let code = codes["codes"][0].as_str().unwrap();
        db.register(code, "member", "hash").await.unwrap();
        let member = db.user_by_username("member").await.unwrap().unwrap();
        db.set_user_text(
            member.id,
            "wecom_webhook",
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=abc",
        )
        .await
        .unwrap();
        let kol = db
            .add_kol("weibo", "甲", "100", None, false, false, false)
            .await
            .unwrap();
        db.subscribe(member.id, false, kol, "post").await.unwrap();
        sqlx::query("UPDATE subscriptions SET favorite = 1 WHERE user_id = ?")
            .bind(member.id)
            .execute(&db.pool)
            .await
            .unwrap();
        let post = db
            .insert_post(kol, "p1", "正文", "2026-07-01 00:00:00", "[]")
            .await
            .unwrap();
        for status in ["success", "failed"] {
            sqlx::query("INSERT INTO push_logs (post_id, channel, status, user_id) VALUES (?, 'wecom', ?, ?)")
                .bind(post).bind(status).bind(member.id).execute(&db.pool).await.unwrap();
        }
        let board = db.dashboard().await.unwrap();
        assert_eq!(board["users"]["total"], 2);
        assert_eq!(board["users"]["admins"], 1);
        assert_eq!(board["users"]["bound"], 1);
        assert_eq!(board["subscriptions"]["total"], 1);
        assert_eq!(board["subscriptions"]["favorite"], 1);
        assert_eq!(board["posts"]["total"], 1);
        assert_eq!(board["posts"]["by_platform"]["weibo"], 1);
        assert_eq!(board["pushes"]["total_7d"], 2);
        assert_eq!(board["pushes"]["ok_7d"], 1);
        assert_eq!(board["pushes"]["fail_7d"], 1);
        assert_eq!(board["pushes"]["success_rate"], json!(50.0));
        assert_eq!(board["pushes"]["by_channel"]["wecom"]["ok"], 1);
        assert_eq!(board["pushes"]["trend_14d"].as_array().unwrap().len(), 1);
        assert!(board["sources_fail_24h"].as_object().unwrap().is_empty());

        db.add_admin_log(admin, "generate_register_codes", "batch", "1")
            .await
            .unwrap();
        let logs = db.list_admin_logs(10).await.unwrap();
        assert_eq!(logs[0]["username"], "admin");
        assert_eq!(logs[0]["action"], "generate_register_codes");
        db.add_error_log("INFO", "vpush.fetch", "ok").await.unwrap();
        db.add_error_log("ERROR", "vpush.fetch", "雪球超时")
            .await
            .unwrap();
        let errors = db.list_error_logs(10, "WARNING", "雪球").await.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["level"], "ERROR");
        assert!(matches!(
            db.list_error_logs(10, "NOISE", "").await,
            Err(CatalogError::Bad(
                "level 需为 DEBUG/INFO/WARNING/ERROR/CRITICAL"
            ))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn bind_code_is_hashed_single_use_and_rate_limited() {
        let path = std::env::temp_dir().join(format!(
            "vpush-db-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let (code, ttl) = db.issue_bind_code(admin, 1_000).await.unwrap();
        assert_eq!(ttl, 600);
        assert_eq!(code.len(), 8);
        assert!(code
            .bytes()
            .all(|byte| b"ABCDEFGHJKMNPQRSTUVWXYZ23456789".contains(&byte)));
        let stored: String = sqlx::query_scalar("SELECT code FROM bind_codes")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_ne!(stored, code);
        assert_eq!(stored, bind_code_digest(&code.to_ascii_lowercase()));
        assert_eq!(
            db.consume_bind_code(&code, "telegram_chat_id", "1001", 1_100)
                .await
                .unwrap(),
            Some(admin)
        );
        let chat: String = sqlx::query_scalar("SELECT telegram_chat_id FROM users WHERE id = ?")
            .bind(admin)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(chat, "1001");
        assert_eq!(
            db.consume_bind_code(&code, "telegram_chat_id", "1001", 1_100)
                .await
                .unwrap(),
            None
        );
        let (again, _) = db.issue_bind_code(admin, 1_001).await.unwrap();
        assert!(db
            .consume_bind_code(&again, "feishu_open_id", "ou_1", 1_700)
            .await
            .unwrap()
            .is_none());
        db.issue_bind_code(admin, 1_002).await.unwrap();
        assert!(matches!(
            db.issue_bind_code(admin, 1_003).await,
            Err(CatalogError::Limited("绑定码生成过于频繁，请稍后再试"))
        ));
        let (fresh, _) = db.issue_bind_code(admin, 1_600).await.unwrap();
        let other = db
            .generate_register_codes(1, "", Some(7), admin)
            .await
            .unwrap();
        db.register(other["codes"][0].as_str().unwrap(), "member", "hash")
            .await
            .unwrap();
        let member = db.user_by_username("member").await.unwrap().unwrap();
        db.set_user_text(member.id, "telegram_chat_id", "2002")
            .await
            .unwrap();
        assert!(matches!(
            db.consume_bind_code(&fresh, "telegram_chat_id", "2002", 1_610)
                .await,
            Err(CatalogError::Bad("该渠道已绑定其他账号"))
        ));
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bind_codes WHERE code = ?")
            .bind(bind_code_digest(&fresh))
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(still, 1);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn telegram_first_contact_and_bind_merge() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('Alice', 'hash')")
            .execute(db.pool())
            .await
            .unwrap();
        assert!(db
            .get_or_create_telegram_user("-100", "Alice", false)
            .await
            .unwrap()
            .is_none());
        assert!(db.user_by_telegram_chat_id("-100").await.unwrap().is_none());
        let source = db
            .get_or_create_telegram_user("123", "Alice", true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.username, "tg_123");
        let marker: i64 = sqlx::query_scalar("SELECT telegram_provisional FROM users WHERE id = ?")
            .bind(source.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(marker, 1);
        assert_eq!(
            source.id,
            db.get_or_create_telegram_user("123", "Other", true)
                .await
                .unwrap()
                .unwrap()
                .id
        );
        sqlx::query("INSERT INTO users (username) VALUES ('tg_456')")
            .execute(db.pool())
            .await
            .unwrap();
        let collision = db
            .get_or_create_telegram_user("456", "Alice", true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(collision.username, "tg_456_1");
        assert_eq!(
            collision.id,
            db.get_or_create_telegram_user("456", "Alice", true)
                .await
                .unwrap()
                .unwrap()
                .id
        );
        let empty = db
            .get_or_create_telegram_user("7890", "", true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(empty.username, "tg_7890");
        assert!(db
            .get_or_create_telegram_user("", "No ID", true)
            .await
            .is_err());
        let kol = db
            .add_kol("weibo", "a", "a", None, false, false, false)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO subscriptions (user_id, kol_id, type, secondary) VALUES (?, ?, 'post', 1)",
        )
        .bind(target)
        .bind(kol)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO subscriptions (user_id, kol_id, type, favorite, hide_images) VALUES (?, ?, 'reply', 1, 1)")
            .bind(source.id).bind(kol).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO user_news_sources VALUES (?, 1), (?, 2), (?, 2)")
            .bind(target)
            .bind(target)
            .bind(source.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_reads VALUES (?, 4), (?, 4), (?, 5)")
            .bind(target)
            .bind(source.id)
            .bind(source.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_seen VALUES (?, '2026-01-01'), (?, '2026-02-01')")
            .bind(target)
            .bind(source.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE users SET news_last_seen_at = '2026-02-01' WHERE id = ?")
            .bind(source.id)
            .execute(db.pool())
            .await
            .unwrap();
        let (code, _) = db.issue_bind_code(target, 1000).await.unwrap();
        assert_eq!(
            db.consume_bind_code(&code, "telegram_chat_id", "123", 1001)
                .await
                .unwrap(),
            Some(target)
        );
        assert_eq!(
            db.consume_bind_code(&code, "telegram_chat_id", "123", 1001)
                .await
                .unwrap(),
            None
        );
        assert!(db.user_by_id(source.id).await.unwrap().is_none());
        assert_eq!(
            db.user_by_telegram_chat_id("123")
                .await
                .unwrap()
                .unwrap()
                .id,
            target
        );
        let sub: (String, i64, i64, i64) = sqlx::query_as("SELECT type, favorite, secondary, hide_images FROM subscriptions WHERE user_id = ? AND kol_id = ?")
            .bind(target).bind(kol).fetch_one(db.pool()).await.unwrap();
        assert_eq!(sub, ("both".into(), 1, 1, 1));
        let sources: Vec<i64> = sqlx::query_scalar(
            "SELECT source_id FROM user_news_sources WHERE user_id = ? ORDER BY source_id",
        )
        .bind(target)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(sources, vec![1, 2]);
        let reads: Vec<i64> = sqlx::query_scalar(
            "SELECT article_id FROM news_reads WHERE user_id = ? ORDER BY article_id",
        )
        .bind(target)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(reads, vec![4, 5]);
        assert_eq!(db.news_seen(target).await.unwrap(), "2026-02-01");
        let seen: String = sqlx::query_scalar("SELECT news_last_seen_at FROM users WHERE id = ?")
            .bind(target)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(seen, "2026-02-01");
        let (code, _) = db.issue_bind_code(target, 1600).await.unwrap();
        let legacy: i64 = sqlx::query_scalar("INSERT INTO users (username, telegram_chat_id) VALUES ('Legacy Name', '789') RETURNING id")
            .fetch_one(db.pool()).await.unwrap();
        assert!(matches!(
            db.consume_bind_code(&code, "telegram_chat_id", "789", 1601)
                .await,
            Err(CatalogError::Bad("该渠道已绑定其他账号"))
        ));
        assert!(db.user_by_id(legacy).await.unwrap().is_some());
        let code_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bind_codes WHERE code = ?")
            .bind(bind_code_digest(&code))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(code_count, 1);
    }

    #[tokio::test]
    async fn telegram_bind_preserves_push_history_and_retry_collisions() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let source = db
            .get_or_create_telegram_user("history", "Bot", true)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("INSERT INTO push_logs (post_id, channel, status, error, user_id) VALUES (11, 'telegram', 'failed', 'source error', ?), (12, 'telegram', 'success', '', ?)")
            .bind(source.id).bind(target).execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO push_retries (channel, user_id, platform, external_id, post_id, attempts, next_at) VALUES ('telegram', ?, 'weibo', 'collision', 11, 5, 120), ('telegram', ?, 'weibo', 'source-only', 12, 3, 130), ('telegram', ?, 'weibo', 'collision', 10, 2, 200)")
            .bind(source.id).bind(source.id).bind(target).execute(db.pool()).await.unwrap();
        let (code, _) = db.issue_bind_code(target, 1000).await.unwrap();
        assert_eq!(
            db.consume_bind_code(&code, "telegram_chat_id", "history", 1001)
                .await
                .unwrap(),
            Some(target)
        );
        assert!(db.user_by_id(source.id).await.unwrap().is_none());
        let logs: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT post_id, status, error FROM push_logs WHERE user_id = ? ORDER BY post_id",
        )
        .bind(target)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            logs,
            vec![
                (11, "failed".into(), "source error".into()),
                (12, "success".into(), "".into())
            ]
        );
        let retries: Vec<(String, i64, i64, i64)> = sqlx::query_as("SELECT external_id, post_id, attempts, next_at FROM push_retries WHERE user_id = ? ORDER BY external_id")
            .bind(target).fetch_all(db.pool()).await.unwrap();
        assert_eq!(
            retries,
            vec![
                ("collision".into(), 10, 5, 120),
                ("source-only".into(), 12, 3, 130)
            ]
        );
        let old_logs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM push_logs WHERE user_id = ?")
            .bind(source.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        let old_retries: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM push_retries WHERE user_id = ?")
                .bind(source.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((old_logs, old_retries), (0, 0));
    }

    #[tokio::test]
    async fn telegram_bind_rejects_personal_settings_without_consuming_code() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        for (index, (column, value)) in [
            ("notify_enabled", "0"),
            ("daily_report", "1"),
            ("translate_twitter", "0"),
            ("push_channels", "telegram"),
            ("dnd_start", "22:00"),
            ("dnd_end", "06:00"),
            ("dnd_allow_favorite", "1"),
            ("keywords", "[\"word\"]"),
            ("keywords_match_reports", "1"),
            ("keywords_match_reports_since", "yesterday"),
            ("keywords_match_news", "1"),
            ("keywords_match_news_since", "yesterday"),
            ("news_font_size", "large"),
            ("llm_api_base", "https://example.com"),
            ("llm_model", "custom"),
            ("llm_api_format", "other"),
        ]
        .iter()
        .enumerate()
        {
            let identity = format!("settings{index}");
            let source = db
                .get_or_create_telegram_user(&identity, "Bot", true)
                .await
                .unwrap()
                .unwrap();
            sqlx::query(&format!("UPDATE users SET {column} = ? WHERE id = ?"))
                .bind(value)
                .bind(source.id)
                .execute(db.pool())
                .await
                .unwrap();
            let (code, _) = db
                .issue_bind_code(target, 1000 + index as i64 * 600)
                .await
                .unwrap();
            assert!(
                matches!(
                    db.consume_bind_code(
                        &code,
                        "telegram_chat_id",
                        &identity,
                        1001 + index as i64 * 600
                    )
                    .await,
                    Err(CatalogError::Bad("该渠道已绑定其他账号"))
                ),
                "{column}"
            );
            let preserved: String = sqlx::query_scalar(&format!(
                "SELECT CAST({column} AS TEXT) FROM users WHERE id = ?"
            ))
            .bind(source.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            assert_eq!(preserved, *value, "{column}");
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bind_codes WHERE code = ?")
                .bind(bind_code_digest(&code))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(count, 1, "{column}");
            assert_eq!(
                db.user_by_telegram_chat_id(&identity)
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                source.id
            );
        }
    }

    #[tokio::test]
    async fn telegram_bind_rejects_non_provisional_owner_and_rolls_back_code() {
        let db = Db::open(Path::new(":memory:")).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let target: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'admin'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        for (index, column) in [
            "password_hash",
            "is_admin",
            "wechat_openid",
            "feishu_open_id",
            "telegram_bot_token",
            "llm_api_key",
            "wecom_webhook",
            "bark_key",
        ]
        .iter()
        .enumerate()
        {
            let identity = format!("{index}");
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO users (username, telegram_chat_id, telegram_provisional) VALUES (?, ?, 1) RETURNING id",
            )
            .bind(format!("owner{index}"))
            .bind(&identity)
            .fetch_one(db.pool())
            .await
            .unwrap();
            if *column == "is_admin" {
                sqlx::query("UPDATE users SET is_admin = 1 WHERE id = ?")
                    .bind(id)
                    .execute(db.pool())
                    .await
                    .unwrap();
            } else {
                sqlx::query(&format!(
                    "UPDATE users SET {column} = 'credential' WHERE id = ?"
                ))
                .bind(id)
                .execute(db.pool())
                .await
                .unwrap();
            }
            let (code, _) = db
                .issue_bind_code(target, 1000 + index as i64 * 600)
                .await
                .unwrap();
            assert!(matches!(
                db.consume_bind_code(
                    &code,
                    "telegram_chat_id",
                    &identity,
                    1001 + index as i64 * 600
                )
                .await,
                Err(CatalogError::Bad("该渠道已绑定其他账号"))
            ));
            let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bind_codes WHERE code = ?")
                .bind(bind_code_digest(&code))
                .fetch_one(db.pool())
                .await
                .unwrap();
            assert_eq!(still, 1);
            assert_eq!(
                db.user_by_telegram_chat_id(&identity)
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                id
            );
        }
        let id: i64 = sqlx::query_scalar("INSERT INTO users (username, telegram_chat_id, telegram_provisional) VALUES ('device_owner', 'device', 1) RETURNING id")
            .fetch_one(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO webpush_subscriptions (user_id, endpoint, p256dh, auth) VALUES (?, 'https://example.com/push', 'key', 'auth')")
            .bind(id).execute(db.pool()).await.unwrap();
        let (code, _) = db.issue_bind_code(target, 6000).await.unwrap();
        assert!(matches!(
            db.consume_bind_code(&code, "telegram_chat_id", "device", 6001)
                .await,
            Err(CatalogError::Bad("该渠道已绑定其他账号"))
        ));
        assert_eq!(
            db.user_by_telegram_chat_id("device")
                .await
                .unwrap()
                .unwrap()
                .id,
            id
        );
        sqlx::query("INSERT INTO users (username, telegram_chat_id) VALUES ('duplicate1', 'shared'), ('duplicate2', 'shared')")
            .execute(db.pool()).await.unwrap();
        let (code, _) = db.issue_bind_code(target, 6600).await.unwrap();
        assert!(matches!(
            db.consume_bind_code(&code, "telegram_chat_id", "shared", 6601)
                .await,
            Err(CatalogError::Bad("该渠道已绑定其他账号"))
        ));
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE telegram_chat_id = 'shared'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn news_source_archives_and_delete_removes_articles() {
        let path = std::env::temp_dir().join(format!(
            "vpush-news-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let source = db.add_news_source("路透", "国际").await.unwrap();
        let feed = db
            .add_news_feed(source, "要闻", "https://example.com/rss.xml")
            .await
            .unwrap();
        db.save_news_entries(
            source,
            feed,
            &[NewsEntry {
                external_id: "a".into(),
                title: "标题".into(),
                summary: "".into(),
                content: "".into(),
                url: "https://example.com/a".into(),
                author: "".into(),
                published_at: "2026-07-01".into(),
            }],
        )
        .await
        .unwrap();
        db.set_news_source_archived(source, true).await.unwrap();
        assert!(db.admin_news_sources().await.unwrap().is_empty());
        let archived = db.admin_news_sources_all(true).await.unwrap();
        assert!(archived[0]["archived_at"].is_string());
        assert_eq!(archived[0]["article_count"], 1);
        db.set_news_source_archived(source, false).await.unwrap();
        db.set_news_feed_archived(feed, true).await.unwrap();
        assert!(db.enabled_feed_ids(Some(source)).await.unwrap().is_empty());
        db.note_feed_failure(feed, "超时").await.unwrap();
        let row = db.feed_value(feed).await.unwrap().unwrap();
        assert_eq!(row["consecutive_failures"], 1);
        assert_eq!(row["last_error_detail"], "超时");
        let article: i64 = sqlx::query_scalar("SELECT id FROM news_articles")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert!(db.delete_news_article(article).await.unwrap());
        assert!(!db.delete_news_article(article).await.unwrap());
        db.save_news_entries(
            source,
            feed,
            &[NewsEntry {
                external_id: "b".into(),
                title: "另一篇".into(),
                summary: "".into(),
                content: "".into(),
                url: "".into(),
                author: "".into(),
                published_at: "".into(),
            }],
        )
        .await
        .unwrap();
        assert!(db.delete_news_source(source).await.unwrap());
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM news_articles")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
        assert!(matches!(
            db.update_news_source(source, Some("路透"), None, None)
                .await,
            Err(CatalogError::Missing("媒体不存在"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn kol_batch_actions_and_link_import() {
        let path = std::env::temp_dir().join(format!(
            "vpush-kol-batch-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let db = Db::open(&path).await.unwrap();
        let imported = db.batch_add_kols(
            "甲 https://xueqiu.com/u/101\nhttps://x.com/someuser\n12345\nhttps://example.com/nope\n",
            None,
        ).await.unwrap();
        assert_eq!(imported["ok"], 2);
        assert_eq!(imported["total"], 4);
        assert_eq!(
            imported["failed"][0]["error"],
            "无法识别平台，请粘贴雪球/微博/X/知识星球主页链接"
        );
        let again = db
            .batch_add_kols("https://xueqiu.com/u/101", None)
            .await
            .unwrap();
        assert_eq!(again["failed"][0]["error"], "该大V已在目录中");
        let ids: Vec<i64> = imported["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        db.insert_post(ids[0], "p1", "正文", "2026-07-01", "[]")
            .await
            .unwrap();
        assert_eq!(
            db.batch_kols(&ids, "priority", Some(&json!(true)))
                .await
                .unwrap(),
            2
        );
        let flags: (i64, i64) = sqlx::query_as("SELECT priority, secondary FROM kols WHERE id = ?")
            .bind(ids[1])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(flags, (1, 0));
        db.batch_kols(&ids, "secondary", Some(&json!(true)))
            .await
            .unwrap();
        let flags: (i64, i64) = sqlx::query_as("SELECT priority, secondary FROM kols WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(flags, (0, 1));
        db.batch_kols(&ids, "normal", None).await.unwrap();
        let flags: (i64, i64) = sqlx::query_as("SELECT priority, secondary FROM kols WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(flags, (0, 0));
        let category = db.add_category("宏观").await.unwrap();
        db.batch_kols(&ids, "category", Some(&json!(category)))
            .await
            .unwrap();
        let stored: Option<i64> = sqlx::query_scalar("SELECT category_id FROM kols WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(stored, Some(category));
        db.batch_kols(&ids, "category", Some(&Value::Null))
            .await
            .unwrap();
        let stored: Option<i64> = sqlx::query_scalar("SELECT category_id FROM kols WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(stored, None);
        assert!(matches!(
            db.batch_kols(&ids, "category", Some(&json!(999))).await,
            Err(CatalogError::Bad("分类不存在"))
        ));
        db.batch_kols(&ids, "disable", None).await.unwrap();
        let enabled: i64 = sqlx::query_scalar("SELECT enabled FROM kols WHERE id = ?")
            .bind(ids[0])
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(enabled, 0);
        assert!(matches!(
            db.batch_kols(&[ids[0], 999], "enable", None).await,
            Err(CatalogError::Missing("大V不存在"))
        ));
        db.batch_kols(&ids, "delete", None).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM kols")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let posts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM posts")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!((left, posts), (0, 0));
        assert!(matches!(
            db.batch_kols(&[], "enable", None).await,
            Err(CatalogError::Bad("请先选择大V"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn users_autoincrement_keeps_ids_and_does_not_reuse_them() {
        let path = std::env::temp_dir().join(format!(
            "vpush-autoinc-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let opts = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    password_hash TEXT NOT NULL DEFAULT '',
                    is_admin INTEGER NOT NULL DEFAULT 0,
                    token_version INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    wechat_openid TEXT NOT NULL DEFAULT ''
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO users (id, username, password_hash) VALUES (4, 'gone', 'old'), (9, 'kept', 'hash')",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }
        let db = Db::open(&path).await.unwrap();
        let kept = db.user_by_username("kept").await.unwrap().unwrap();
        assert_eq!(kept.id, 9);
        assert_eq!(kept.password_hash, "hash");
        let definition: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'users'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert!(definition.to_ascii_uppercase().contains("AUTOINCREMENT"));
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('fresh', 'p')")
            .execute(db.pool())
            .await
            .unwrap();
        let fresh = db.user_by_username("fresh").await.unwrap().unwrap();
        assert!(fresh.id > 9, "reused {}", fresh.id);
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        db.delete_user(admin.id, fresh.id).await.unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('next', 'p')")
            .execute(db.pool())
            .await
            .unwrap();
        let next = db.user_by_username("next").await.unwrap().unwrap();
        assert!(next.id > fresh.id, "{} reused {}", next.id, fresh.id);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rewrite_users_ddl_keeps_constraints() {
        let sql = "CREATE TABLE \"users\" (
            id INTEGER PRIMARY KEY,
            username TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(username) > 0)
        )";
        let out = rewrite_users_ddl(sql).unwrap();
        assert!(out.starts_with("CREATE TABLE users__autoinc"));
        assert!(out.contains("INTEGER PRIMARY KEY AUTOINCREMENT"));
        assert!(out.contains("COLLATE NOCASE"));
        assert!(out.contains("CHECK(length(username) > 0)"));
        assert!(rewrite_users_ddl("CREATE TABLE users (id INT PRIMARY KEY)").is_err());
    }

    #[tokio::test]
    async fn users_autoincrement_keeps_cascade_children() {
        let path = std::env::temp_dir().join(format!(
            "vpush-autoinc-fk-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let opts = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .foreign_keys(false);
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(opts)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    username TEXT NOT NULL COLLATE NOCASE UNIQUE CHECK(length(username) > 0),
                    password_hash TEXT NOT NULL DEFAULT '',
                    is_admin INTEGER NOT NULL DEFAULT 0,
                    token_version INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    wechat_openid TEXT NOT NULL DEFAULT ''
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE legacy_user_tokens (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    token TEXT NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TRIGGER legacy_users_touch AFTER UPDATE ON users BEGIN SELECT 1; END",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("CREATE VIEW legacy_user_names AS SELECT id, username FROM users")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query(
                "CREATE TRIGGER legacy_names_insert INSTEAD OF INSERT ON legacy_user_names
                 BEGIN
                   INSERT INTO users (username, password_hash) VALUES (NEW.username, 'from-view');
                 END",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE register_codes (
                    code TEXT PRIMARY KEY,
                    used_by INTEGER,
                    created_by INTEGER
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO register_codes (code, used_by, created_by) VALUES
                 ('OLD', 99, 4), ('GHOST', 77, 88)",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO users (id, username, password_hash) VALUES (4, 'kept', 'hash')",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO legacy_user_tokens (id, user_id, token) VALUES (1, 4, 'device'), (2, 99, 'orphan')",
            )
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }
        let db = Db::open(&path).await.unwrap();
        let kept: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM legacy_user_tokens WHERE user_id = 4")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let orphan: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM legacy_user_tokens WHERE user_id = 99")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((kept, orphan), (1, 0));
        let via_view: i64 =
            sqlx::query_scalar("SELECT id FROM legacy_user_names WHERE username = 'kept'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(via_view, 4);
        let trigger: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'legacy_users_touch'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(trigger, 1);
        let instead: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'legacy_names_insert'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(instead, 1);
        sqlx::query("INSERT INTO legacy_user_names (username) VALUES ('via-view')")
            .execute(db.pool())
            .await
            .unwrap();
        let via = db.user_by_username("via-view").await.unwrap().unwrap();
        assert_eq!(via.password_hash, "from-view");
        let old_used: Option<i64> =
            sqlx::query_scalar("SELECT used_by FROM register_codes WHERE code = 'OLD'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let old_created: Option<i64> =
            sqlx::query_scalar("SELECT created_by FROM register_codes WHERE code = 'OLD'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let ghost_used: Option<i64> =
            sqlx::query_scalar("SELECT used_by FROM register_codes WHERE code = 'GHOST'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let ghost_created: Option<i64> =
            sqlx::query_scalar("SELECT created_by FROM register_codes WHERE code = 'GHOST'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(
            (old_used, old_created, ghost_used, ghost_created),
            (None, Some(4), None, None)
        );
        let definition: String = sqlx::query_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'users'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        let upper = definition.to_ascii_uppercase();
        assert!(upper.contains("AUTOINCREMENT"));
        assert!(upper.contains("CHECK"));
        assert!(upper.contains("COLLATE NOCASE"));
        assert!(
            sqlx::query("INSERT INTO users (username, password_hash) VALUES ('', 'x')")
                .execute(db.pool())
                .await
                .is_err()
        );
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('fresh', 'p')")
            .execute(db.pool())
            .await
            .unwrap();
        let fresh = db.user_by_username("fresh").await.unwrap().unwrap();
        assert!(
            fresh.id > 99,
            "reused {} (sequence must keep the orphan id)",
            fresh.id
        );
        let token: String =
            sqlx::query_scalar("SELECT token FROM legacy_user_tokens WHERE user_id = 4")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(token, "device");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn delete_user_clears_every_user_id_table() {
        let path = std::env::temp_dir().join(format!(
            "vpush-delete-user-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).await.unwrap();
        db.ensure_admin("hash").await.unwrap();
        let admin = db.user_by_username("admin").await.unwrap().unwrap();
        sqlx::query("INSERT INTO users (username, password_hash) VALUES ('reader01', 'x')")
            .execute(db.pool())
            .await
            .unwrap();
        let reader = db.user_by_username("reader01").await.unwrap().unwrap();
        sqlx::query("INSERT INTO kols (platform, name, external_id) VALUES ('xueqiu', '甲', '1')")
            .execute(db.pool())
            .await
            .unwrap();
        let kol: i64 = sqlx::query_scalar("SELECT id FROM kols")
            .fetch_one(db.pool())
            .await
            .unwrap();
        for (sql, id) in [
            (
                "INSERT INTO subscriptions (user_id, kol_id) VALUES (?, ?)",
                reader.id,
            ),
            (
                "INSERT INTO subscriptions (user_id, kol_id) VALUES (?, ?)",
                admin.id,
            ),
        ] {
            sqlx::query(sql)
                .bind(id)
                .bind(kol)
                .execute(db.pool())
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO kol_acl (kol_id, user_id) VALUES (?, ?)")
            .bind(kol)
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO kol_requests (platform, external_id, user_id) VALUES ('weibo', '9', ?)",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO news_reads (user_id, article_id) VALUES (?, 1)")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_seen (user_id, seen_at) VALUES (?, '2020-01-01')")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO news_keyword_notified (user_id, article_id) VALUES (?, 1)")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO knowledge_keyword_notified (user_id, group_id, media_id) VALUES (?, 'g', 'm')",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO bind_codes (code, user_id, expires_at) VALUES ('code', ?, 1)")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO feishu_oauth_sessions (state_hash, user_id, code_verifier, expires_at) VALUES ('h', ?, 'v', 1)",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO push_logs (post_id, channel, status, user_id) VALUES (1, 'web', 'ok', ?)",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO push_retries (channel, user_id, platform, external_id, post_id, next_at) VALUES ('web', ?, 'xueqiu', 'e', 1, 1)",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO admin_logs (user_id, action) VALUES (?, 'touch')")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO webpush_subscriptions (user_id, endpoint, p256dh, auth) VALUES (?, 'https://push.example/reader', 'p', 'a')",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO webpush_subscriptions (user_id, endpoint, p256dh, auth) VALUES (?, 'https://push.example/admin', 'p', 'a')",
        )
        .bind(admin.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO android_devices (installation_id, user_id, token, provider) VALUES ('dev', ?, 't', 'fcm')",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO ima_kb_acl (group_id, user_id) VALUES ('reports', ?)")
            .bind(reader.id)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO ima_kb_subscriptions (user_id, group_id, created_at) VALUES (?, 'reports', 1)",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO feishu_personal_bots (user_id, app_id, app_secret_ciphertext, status) VALUES (?, 'cli_test', 'cipher', 'active')",
        )
        .bind(reader.id)
        .execute(db.pool())
        .await
        .unwrap();
        assert!(count_user_rows(&db, reader.id).await >= 14);
        db.delete_user(admin.id, reader.id).await.unwrap();
        assert!(db.user_by_id(reader.id).await.unwrap().is_none());
        assert_eq!(count_user_rows(&db, reader.id).await, 0);
        let admin_push: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM webpush_subscriptions WHERE user_id = ?")
                .bind(admin.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let admin_sub: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM subscriptions WHERE user_id = ?")
                .bind(admin.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!((admin_push, admin_sub), (1, 1));
        let _ = std::fs::remove_file(&path);
    }

    async fn count_user_rows(db: &Db, id: i64) -> i64 {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();
        let mut total = 0;
        for name in names {
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            let cols: Vec<String> =
                sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{name}')"))
                    .fetch_all(db.pool())
                    .await
                    .unwrap();
            if !cols.iter().any(|col| col == "user_id") {
                continue;
            }
            let n: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM \"{name}\" WHERE user_id = ?"
            ))
            .bind(id)
            .fetch_one(db.pool())
            .await
            .unwrap();
            total += n;
        }
        total
    }
}
