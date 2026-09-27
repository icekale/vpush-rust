mod alerts;
mod arm;
mod auth;
mod backup;
mod cicc;
mod combination;
mod db;
mod feishu;
mod feishu_admin;
mod feishu_docs;
mod feishu_personal;
mod feishu_ws;
mod ima_admin;
mod ima_client;
mod ima_collector;
mod img_proxy;
mod imgbed;
mod llm;
mod maintenance;
mod market;
mod news;
mod proxy_admin;
mod push;
mod reports;
mod syslogs;
mod tags;
mod truth;
mod twitter;
mod url_guard;
mod webpush;
mod wechat;
mod weibo;
mod weibo_qr;
mod wscn;
mod xincai;
mod xq_crypto;
mod xueqiu;
mod zsxq;
mod zsxq_file;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{
    ConnectInfo, DefaultBodyLimit, Extension, Multipart, Path as UrlPath, Query, State,
};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::db::{CatalogError, Db, FeedFilter, KolPatch, RegisterError, User};

const APP_VERSION: &str = "1.12.277";
const LOGIN_MAX_FAILURES: usize = 8;
const LOGIN_WINDOW_SECS: u64 = 300;
const SPA: &[&str] = &[
    "timeline",
    "home",
    "combinations",
    "mysubs",
    "settings",
    "news",
    "search",
    "kol",
    "more",
    "admin",
    "zsxq",
    "ima-documents",
    "knowledge",
    "ticker",
];

#[derive(Clone)]
struct ClientIp(String);

#[derive(Clone)]
struct AppState {
    db: Db,
    secret: String,
    allow_register: bool,
    static_dir: PathBuf,
    // ponytail: in-memory IP window; move to sqlite if more than one process serves login
    fails: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

struct ApiError {
    status: StatusCode,
    detail: String,
    retry_after: Option<u64>,
}

impl ApiError {
    fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
            retry_after: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({ "detail": self.detail }))).into_response();
        if let Some(after) = self.retry_after {
            if let Ok(value) = after.to_string().parse() {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

#[derive(Deserialize)]
struct LoginIn {
    username: String,
    password: String,
    #[serde(default, rename = "cf-turnstile-response")]
    turnstile: String,
}

#[derive(Deserialize)]
struct RegisterIn {
    username: String,
    password: String,
    code: String,
    #[serde(default, rename = "cf-turnstile-response")]
    turnstile: String,
}

#[derive(Deserialize)]
struct TurnstileIn {
    enabled: bool,
    #[serde(default)]
    sitekey: String,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    hostnames: String,
}

#[derive(Deserialize)]
struct WechatIn {
    code: String,
    #[serde(default)]
    invite_code: String,
}

#[derive(Deserialize)]
struct MeUpdate {
    telegram_chat_id: Option<String>,
    telegram_bot_token: Option<String>,
    feishu_open_id: Option<String>,
    feishu_chat_id: Option<String>,
    wecom_webhook: Option<String>,
    bark_key: Option<String>,
    notify_enabled: Option<bool>,
    daily_report_enabled: Option<bool>,
    translate_twitter: Option<bool>,
    push_channels: Option<String>,
    dnd_start: Option<String>,
    dnd_end: Option<String>,
    dnd_allow_favorite: Option<bool>,
    keywords: Option<Vec<String>>,
    keywords_match_reports: Option<bool>,
    keywords_match_news: Option<bool>,
    news_font_size: Option<String>,
    news_source_ids: Option<Vec<i64>>,
    llm_api_base: Option<String>,
    llm_api_key: Option<String>,
    llm_model: Option<String>,
    llm_api_format: Option<String>,
}

#[derive(Deserialize)]
struct WebPushIn {
    endpoint: String,
    keys: WebPushKeys,
}

#[derive(Deserialize)]
struct WebPushKeys {
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
struct PasswordChange {
    old_password: String,
    new_password: String,
}

#[tokio::main]
async fn main() {
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vpush=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(syslogs::layer())
        .init();
    let db_path = std::env::var("VPUSH_DB").unwrap_or_else(|_| "data/vpush.db".into());
    let static_dir = std::env::var("VPUSH_STATIC").unwrap_or_else(|_| "static".into());
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let db = Db::open(Path::new(&db_path)).await.expect("open sqlite");
    if let Ok(password) = std::env::var("WEB_ADMIN_PASSWORD") {
        if !password.is_empty() {
            let hash = auth::hash_password(&password, None);
            db.ensure_admin(&hash).await.expect("create admin");
        }
    }
    let secret = match std::env::var("WEB_TOKEN_SECRET") {
        Ok(value) if !value.trim().is_empty() => value,
        _ if host
            .parse::<std::net::IpAddr>()
            .map(|ip| !ip.is_loopback())
            .unwrap_or(true) =>
        {
            panic!("公网监听必须配置 WEB_TOKEN_SECRET");
        }
        _ => match db.setting("token_secret").await.expect("token secret") {
            Some(value) => value,
            None => {
                let value = auth::random_secret();
                db.set_setting("token_secret", &value)
                    .await
                    .expect("save token secret");
                tracing::warn!("WEB_TOKEN_SECRET 未配置，会话签名密钥写在数据库里");
                value
            }
        },
    };
    let allow_register = !matches!(
        std::env::var("WEB_ALLOW_REGISTER").ok().as_deref(),
        Some("0" | "false" | "False")
    );
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8000);
    let state = AppState {
        db,
        secret,
        allow_register,
        static_dir: PathBuf::from(static_dir),
        fails: Arc::new(Mutex::new(HashMap::new())),
    };
    xueqiu::spawn(state.db.clone());
    combination::spawn(state.db.clone());
    weibo::spawn(state.db.clone());
    news::spawn(state.db.clone());
    zsxq::spawn(state.db.clone());
    truth::spawn(state.db.clone());
    twitter::spawn(state.db.clone());
    maintenance::spawn(state.db.clone());
    feishu_ws::resume(state.db.clone()).await;
    let app = router(state);
    let addr: SocketAddr = format!("{host}:{port}").parse().expect("bind address");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/version", get(version))
        .route("/api/auth/turnstile", get(turnstile))
        .route("/api/auth/login", post(login))
        .route("/api/auth/register", post(register))
        .route("/api/auth/wechat", post(wechat_login))
        .route("/api/auth/logout", post(logout))
        .route("/api/me/llm-models", post(llm_models))
        .route("/api/me/llm-test", post(llm_test))
        .route(
            "/api/me/feishu-personal/register",
            post(feishu_personal_register),
        )
        .route(
            "/api/me/feishu-personal/register/{session_id}/refresh-code",
            post(feishu_personal_refresh),
        )
        .route(
            "/api/me/feishu-personal/register/{session_id}/cancel",
            post(feishu_personal_cancel),
        )
        .route(
            "/api/me/feishu-personal/register/{session_id}",
            get(feishu_personal_status),
        )
        .route("/api/me/feishu-personal", delete(feishu_personal_delete))
        .route("/api/me/bind-code", post(issue_bind_code))
        .route("/api/me", get(me).put(update_me))
        .route("/api/me/password", post(change_password))
        .route(
            "/api/me/webpush",
            post(subscribe_webpush).delete(unsubscribe_webpush),
        )
        .route("/api/my/feed", get(feed))
        .route("/api/catalog", get(catalog))
        .route("/api/recommendations", get(recommendations))
        .route("/api/categories", get(categories).post(add_category))
        .route(
            "/api/categories/{category_id}",
            put(rename_category).delete(remove_category),
        )
        .route("/api/posts", get(list_posts))
        .route("/api/push-logs", get(list_push_logs))
        .route("/api/tags", get(tags).put(save_tags))
        .route("/api/tags/maintain", post(maintain_tags))
        .route("/api/tags/backfill", post(backfill_tags))
        .route("/api/subscriptions", post(subscribe))
        .route(
            "/api/subscriptions/{kol_id}",
            put(set_sub_type).delete(unsubscribe),
        )
        .route("/api/subscriptions/{kol_id}/favorite", put(set_favorite))
        .route("/api/subscriptions/{kol_id}/secondary", put(set_secondary))
        .route(
            "/api/subscriptions/{kol_id}/hide-images",
            put(set_hide_images),
        )
        .route("/api/my/subscriptions", get(my_subscriptions))
        .route("/api/kol-requests", post(create_kol_request))
        .route("/api/my/kol-requests", get(my_kol_requests))
        .route("/api/admin/kol-requests", get(admin_kol_requests))
        .route(
            "/api/admin/kol-requests/{request_id}/approve",
            post(approve_kol_request),
        )
        .route(
            "/api/admin/kol-requests/{request_id}/reject",
            post(reject_kol_request),
        )
        .route(
            "/api/admin/imgbed",
            get(imgbed_status).put(save_imgbed).delete(clear_imgbed),
        )
        .route("/api/stats", get(stats))
        .route("/api/admin/dashboard", get(admin_dashboard))
        .route("/api/admin/logs", get(admin_logs))
        .route("/api/admin/error-logs", get(error_logs))
        .route("/api/admin/system-logs", get(system_logs))
        .route(
            "/api/admin/polling-config",
            get(polling_config).put(update_polling),
        )
        .route("/api/admin/xueqiu-cookie", post(save_xueqiu_cookie))
        .route("/api/admin/twitter-cookie", post(save_twitter_cookie))
        .route("/api/admin/zsxq-cookie", post(save_zsxq_cookie))
        .route("/api/admin/cookies/{kind}", delete(clear_cookie))
        .route("/api/admin/zsxq-cache/purge", post(purge_zsxq_cache))
        .route("/api/admin/backup", get(backup_status))
        .route("/api/admin/backup/webdav", put(backup_save))
        .route("/api/admin/backup/webdav/test", post(backup_test))
        .route("/api/admin/backup/download", get(backup_download))
        .route(
            "/api/admin/backup/restore/webdav",
            post(backup_restore_webdav),
        )
        .route(
            "/api/admin/backup/restore/upload",
            post(backup_restore_upload).layer(DefaultBodyLimit::max(200 * 1024 * 1024)),
        )
        .route(
            "/api/admin/turnstile",
            get(admin_turnstile).put(set_admin_turnstile),
        )
        .route(
            "/api/admin/register-codes",
            get(list_register_codes).post(generate_register_codes),
        )
        .route(
            "/api/admin/register-codes/batch",
            post(register_codes_batch),
        )
        .route(
            "/api/admin/register-codes/{code}/revoke",
            post(revoke_register_code),
        )
        .route(
            "/api/admin/register-code-batches/{batch_id}/revoke-unused",
            post(revoke_register_batch),
        )
        .route("/api/users", get(list_users))
        .route("/api/users/{user_id}", put(update_user).delete(delete_user))
        .route("/api/admin/users/{user_id}/ima-kb", put(set_user_ima_kb))
        .route("/api/admin/users/batch", post(users_batch))
        .route("/api/admin/test-push", post(test_push))
        .route(
            "/api/admin/inactive-users-policy",
            get(inactive_policy).put(save_inactive_policy),
        )
        .route(
            "/api/admin/plaza-sources",
            get(plaza_sources).put(save_plaza_sources),
        )
        .route(
            "/api/ima-documents/groups/{group_id}/subscribe",
            post(subscribe_ima_kb).delete(unsubscribe_ima_kb),
        )
        .route(
            "/api/admin/ima-credentials",
            get(ima_credentials).post(save_ima_credentials),
        )
        .route(
            "/api/admin/ima-local-libraries",
            get(ima_local_libraries).post(create_ima_local_library),
        )
        .route(
            "/api/admin/ima-local-libraries/scan",
            post(scan_ima_local_libraries),
        )
        .route(
            "/api/admin/ima-local-libraries/{slug}/enabled",
            put(set_ima_local_enabled),
        )
        .route(
            "/api/admin/ima-local-libraries/{slug}",
            put(update_ima_local_library),
        )
        .route(
            "/api/admin/feishu-documents",
            get(feishu_docs_admin).post(feishu_docs_add),
        )
        .route(
            "/api/admin/feishu-documents/config",
            put(feishu_docs_config),
        )
        .route(
            "/api/admin/feishu-documents/oauth/start",
            post(feishu_docs_oauth_start),
        )
        .route(
            "/api/admin/feishu-documents/oauth/callback",
            get(feishu_docs_oauth_callback),
        )
        .route(
            "/api/admin/feishu-documents/preview",
            post(feishu_docs_preview),
        )
        .route(
            "/api/admin/feishu-documents/{id}/sync",
            post(feishu_docs_sync),
        )
        .route(
            "/api/admin/feishu-documents/{id}",
            patch(feishu_docs_update).delete(feishu_docs_delete),
        )
        .route(
            "/api/admin/proxies",
            get(list_proxies).post(import_proxies_loose),
        )
        .route("/api/admin/proxies/{id}/test", post(test_proxy))
        .route("/api/admin/proxies/{id}", delete(delete_proxy))
        .route(
            "/api/admin/proxy-pools",
            get(list_proxy_pools).post(create_proxy_pool),
        )
        .route(
            "/api/admin/proxy-pools/{id}/import",
            post(import_proxy_pool),
        )
        .route(
            "/api/admin/proxy-pools/{id}/extract",
            post(extract_proxy_pool),
        )
        .route("/api/admin/proxy-pools/{id}", delete(delete_proxy_pool))
        .route(
            "/api/admin/proxy-routes",
            get(proxy_routes).put(save_proxy_routes),
        )
        .route(
            "/api/admin/ima-collector",
            get(ima_collector_status).put(save_ima_collector),
        )
        .route(
            "/api/admin/ima-collector/discover",
            post(discover_ima_groups),
        )
        .route("/api/admin/ima-collector/sync", post(sync_ima_collector))
        .route(
            "/api/admin/ima-collector/groups/{group_id}/folders",
            get(ima_group_folders),
        )
        .route(
            "/api/admin/ima-collector/groups/{group_id}/acl",
            put(set_ima_kb_acl),
        )
        .route("/api/ima-documents/catalog", get(feishu_catalog))
        .route("/api/ima-documents", get(feishu_documents))
        .route("/api/ima-documents/timeline/all", get(feishu_timeline))
        .route("/api/ima-documents/tickers/{code}", get(ima_ticker))
        .route(
            "/api/ima-documents/{media_id}/translate",
            post(ima_translate),
        )
        .route(
            "/api/ima-documents/{media_id}/assets/{asset_id}",
            get(feishu_asset),
        )
        .route("/api/ima-documents/{media_id}/pdf", get(ima_pdf))
        .route("/api/ima-documents/{media_id}/text", get(ima_text))
        .route("/api/ima-documents/{media_id}", get(feishu_document))
        .route("/api/xincai/ingest", post(xincai_ingest))
        .route("/api/news/sources", get(news_sources))
        .route("/api/news", get(news_list))
        .route("/api/news/seen", post(news_seen))
        .route("/api/news/read-all", post(news_read_all))
        .route("/api/news/read-all/undo", post(news_read_all_undo))
        .route("/api/news/magazine", get(news_magazine))
        .route("/api/news/{article_id}/images/{index}", get(news_image))
        .route("/api/media/zsxq-file/{file_id}", get(zsxq_file))
        .route("/api/news/{article_id}/read", post(news_read))
        .route("/api/news/{article_id}", get(news_article))
        .route(
            "/api/admin/news/settings",
            get(news_settings).patch(save_news_settings),
        )
        .route(
            "/api/admin/news/sources",
            get(admin_news_sources).post(create_news_source),
        )
        .route(
            "/api/admin/news/sources/{source_id}/feeds",
            post(create_news_feed),
        )
        .route(
            "/api/admin/news/sources/{source_id}/archive",
            post(archive_news_source),
        )
        .route(
            "/api/admin/news/sources/{source_id}/restore",
            post(restore_news_source),
        )
        .route(
            "/api/admin/news/sources/{source_id}/refresh",
            post(refresh_news_source),
        )
        .route(
            "/api/admin/news/sources/{source_id}",
            delete(delete_news_source).patch(update_news_source),
        )
        .route("/api/admin/news/feeds/validate", post(validate_news_feed))
        .route(
            "/api/admin/news/feeds/{feed_id}/archive",
            post(archive_news_feed),
        )
        .route(
            "/api/admin/news/feeds/{feed_id}/restore",
            post(restore_news_feed),
        )
        .route(
            "/api/admin/news/feeds/{feed_id}/refresh",
            post(refresh_news_feed),
        )
        .route(
            "/api/admin/news/feeds/{feed_id}",
            delete(delete_news_feed).patch(update_news_feed),
        )
        .route("/api/admin/news/refresh", post(refresh_news))
        .route(
            "/api/admin/news/articles/{article_id}",
            delete(delete_news_article),
        )
        .route("/api/admin/news/articles", get(admin_news_articles))
        .route("/api/admin/ima-arm", get(ima_arm))
        .route("/api/admin/weibo-qr/start", post(weibo_qr_start))
        .route("/api/admin/weibo-qr/status", get(weibo_qr_status))
        .route("/api/admin/cicc/status", get(cicc_status))
        .route("/api/admin/cicc/trigger", post(cicc_trigger))
        .route(
            "/api/admin/cicc/schedule",
            get(cicc_schedule).put(cicc_save_schedule),
        )
        .route(
            "/api/admin/ima-collector/cicc-categories",
            get(cicc_categories).put(cicc_save_categories),
        )
        .route("/api/kols", get(admin_kols).post(add_kol))
        .route("/api/kols/batch", post(batch_add_kols))
        .route("/api/admin/kols", get(list_admin_kols))
        .route("/api/admin/kols/batch", post(batch_kols))
        .route(
            "/api/kols/{kol_id}",
            get(kol_detail).put(update_kol).delete(delete_kol),
        )
        .route("/api/kols/{kol_id}/posts", get(kol_posts))
        .route("/api/kols/{kol_id}/holdings", get(kol_holdings))
        .route("/api/kols/{kol_id}/nav", get(kol_nav))
        .route("/api/img-proxy", get(img_proxy))
        .route("/api/live/wscn", get(live_wscn))
        .route("/api/market/indices", get(market_indices))
        .fallback(static_or_spa)
        .layer(middleware::from_fn(attach_ip))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version() -> Json<Value> {
    // ponytail: pinned to app.js APP_VERSION so the shell does not reload itself
    Json(json!({
        "current": APP_VERSION,
        "latest": "",
        "update_available": false,
        "url": "https://github.com/icekale/vpush/releases",
    }))
}

async fn turnstile(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let runtime = turnstile_runtime(&state.db).await?;
    let sitekey = if runtime.active {
        runtime.sitekey
    } else {
        String::new()
    };
    Ok(Json(json!({ "sitekey": sitekey })))
}

struct TurnstileRuntime {
    enabled: bool,
    active: bool,
    sitekey: String,
    secret: String,
    secret_set: bool,
    secret_from_env: bool,
    hostnames: String,
    hosts: Vec<String>,
}

async fn turnstile_runtime(db: &Db) -> Result<TurnstileRuntime, ApiError> {
    let stored_enabled = db
        .setting("turnstile_enabled")
        .await
        .map_err(db_err)?
        .unwrap_or_default();
    let stored_site = db
        .setting("turnstile_site_key")
        .await
        .map_err(db_err)?
        .unwrap_or_default();
    let stored_secret = db
        .setting("turnstile_secret")
        .await
        .map_err(db_err)?
        .unwrap_or_default();
    let stored_hosts = db
        .setting("turnstile_hostnames")
        .await
        .map_err(db_err)?
        .unwrap_or_default();
    let stored_site = stored_site.trim().to_string();
    let stored_secret = stored_secret.trim().to_string();
    let stored_hosts = stored_hosts.trim().to_string();
    let env_site = std::env::var("TURNSTILE_SITE_KEY").unwrap_or_default();
    let env_secret = std::env::var("TURNSTILE_SECRET").unwrap_or_default();
    let env_hosts = std::env::var("TURNSTILE_HOSTNAMES").unwrap_or_default();
    let sitekey = if stored_site.is_empty() {
        env_site.trim().to_string()
    } else {
        stored_site
    };
    let secret = if stored_secret.is_empty() {
        env_secret.trim().to_string()
    } else {
        stored_secret.clone()
    };
    let hostnames = if stored_hosts.is_empty() {
        env_hosts.trim().to_string()
    } else {
        stored_hosts
    };
    let enabled = if stored_enabled == "0" || stored_enabled == "1" {
        stored_enabled == "1"
    } else {
        !secret.is_empty()
    };
    let hosts = hostnames
        .split(',')
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    let active = enabled && !secret.is_empty() && !sitekey.is_empty();
    Ok(TurnstileRuntime {
        enabled,
        active,
        sitekey,
        secret_set: !secret.is_empty(),
        secret_from_env: !env_secret.trim().is_empty() && stored_secret.is_empty(),
        secret,
        hostnames,
        hosts,
    })
}

fn turnstile_admin_json(runtime: &TurnstileRuntime) -> Value {
    json!({
        "enabled": runtime.enabled,
        "active": runtime.active,
        "sitekey": runtime.sitekey,
        "secret_set": runtime.secret_set,
        "secret_from_env": runtime.secret_from_env,
        "hostnames": runtime.hostnames,
    })
}

fn parse_turnstile_hostnames(raw: &str) -> Result<String, ApiError> {
    let parts = raw
        .split(',')
        .map(|item| item.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "请填写允许域名"));
    }
    for host in &parts {
        let ok = host.len() <= 253
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        if !ok || host.contains('/') || host.contains(':') {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("域名无效: {host}"),
            ));
        }
    }
    Ok(parts.join(","))
}

async fn admin_turnstile(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(turnstile_admin_json(
        &turnstile_runtime(&state.db).await?,
    )))
}

async fn set_admin_turnstile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TurnstileIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let current = turnstile_runtime(&state.db).await?;
    let posted_site = body.sitekey.trim();
    let posted_secret = body.secret.trim();
    let posted_hosts = body.hostnames.trim();
    let sitekey = if posted_site.is_empty() {
        current.sitekey.clone()
    } else {
        posted_site.to_string()
    };
    let secret = if posted_secret.is_empty() {
        state
            .db
            .setting("turnstile_secret")
            .await
            .map_err(db_err)?
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        posted_secret.to_string()
    };
    let secret = if secret.is_empty() {
        std::env::var("TURNSTILE_SECRET")
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        secret
    };
    let hostnames = if body.enabled {
        parse_turnstile_hostnames(if posted_hosts.is_empty() {
            &current.hostnames
        } else {
            posted_hosts
        })?
    } else if !posted_hosts.is_empty() {
        parse_turnstile_hostnames(posted_hosts)?
    } else {
        current.hostnames.clone()
    };
    if body.enabled && sitekey.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "请填写站点密钥"));
    }
    if body.enabled && secret.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "请填写密钥"));
    }
    state
        .db
        .set_setting("turnstile_enabled", if body.enabled { "1" } else { "0" })
        .await
        .map_err(db_err)?;
    if !posted_site.is_empty() {
        state
            .db
            .set_setting("turnstile_site_key", &sitekey)
            .await
            .map_err(db_err)?;
    }
    if !posted_secret.is_empty() {
        state
            .db
            .set_setting("turnstile_secret", &secret)
            .await
            .map_err(db_err)?;
    }
    state
        .db
        .set_setting("turnstile_hostnames", &hostnames)
        .await
        .map_err(db_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "set_turnstile",
            "",
            &format!(
                "enabled={} sitekey={sitekey} hostnames={hostnames} secret={}",
                i32::from(body.enabled),
                if secret.is_empty() { "missing" } else { "set" }
            ),
        )
        .await;
    Ok(Json(turnstile_admin_json(
        &turnstile_runtime(&state.db).await?,
    )))
}

fn verify_turnstile(secret: &str, token: &str, action: &str, hosts: &[String], ip: &str) -> bool {
    if token.is_empty() || token.len() > 2048 {
        return false;
    }
    let mut fields = vec![("secret", secret), ("response", token)];
    if !ip.is_empty() && ip != "unknown" {
        fields.push(("remoteip", ip));
    }
    let response = ureq::post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .timeout(std::time::Duration::from_secs(10))
        .send_form(&fields);
    let Ok(response) = response else { return false };
    let Ok(text) = response.into_string() else {
        return false;
    };
    let Ok(result) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    result["success"] == true
        && result["action"].as_str() == Some(action)
        && hosts
            .iter()
            .any(|host| result["hostname"].as_str() == Some(host))
}

async fn require_turnstile(
    state: &AppState,
    token: &str,
    action: &str,
    ip: &str,
) -> Result<(), ApiError> {
    let runtime = turnstile_runtime(&state.db).await?;
    if !runtime.active {
        return Ok(());
    }
    let secret = runtime.secret.clone();
    let token = token.to_string();
    let action = action.to_string();
    let hosts = runtime.hosts.clone();
    let ip = ip.to_string();
    let ok = tokio::task::spawn_blocking(move || {
        verify_turnstile(&secret, &token, &action, &hosts, &ip)
    })
    .await
    .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(ApiError::new(StatusCode::FORBIDDEN, "人机验证失败，请重试"))
    }
}

async fn live_wscn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LiveQuery>,
) -> Result<Json<Value>, ApiError> {
    require_user(&state, &headers).await?;
    let cursor = q.cursor.unwrap_or_default();
    if !cursor.is_empty() && (cursor.len() > 16 || !cursor.bytes().all(|b| b.is_ascii_digit())) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "无效的 cursor"));
    }
    let limit = q.limit.unwrap_or(30).clamp(1, 50);
    let page = wscn::live(&cursor, limit, q.since_id.unwrap_or(0))
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_GATEWAY, err))?;
    Ok(Json(page))
}

#[derive(Deserialize)]
struct LiveQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    since_id: Option<i64>,
}

async fn market_indices(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<MarketQuery>,
) -> Result<Json<Value>, ApiError> {
    require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_else(|| "auto".into());
    if !matches!(group.as_str(), "auto" | "day" | "night") {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "无效的分组"));
    }
    Ok(Json(market::snapshot(&group).await))
}

#[derive(Deserialize)]
struct MarketQuery {
    group: Option<String>,
}

#[derive(Deserialize)]
struct CatalogQuery {
    platform: Option<String>,
    category_id: Option<i64>,
}

#[derive(Deserialize)]
struct FeedQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    platform: Option<String>,
    category_id: Option<i64>,
    q: Option<String>,
    favorite: Option<i64>,
    tag: Option<String>,
    include_secondary: Option<i64>,
    since_id: Option<i64>,
}

#[derive(Deserialize)]
struct TagMaintainBody {
    backfill: Option<String>,
}

#[derive(Deserialize)]
struct TagBackfillBody {
    mode: Option<String>,
}

#[derive(Deserialize)]
struct DownloadQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
struct FlagQuery {
    unsubscribed: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct SubIn {
    kol_id: i64,
    #[serde(default = "default_sub_type", rename = "type")]
    kind: String,
}

fn default_sub_type() -> String {
    "post".into()
}

#[derive(Deserialize)]
struct TypeIn {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct FavoriteIn {
    favorite: bool,
}

#[derive(Deserialize)]
struct SecondaryIn {
    secondary: bool,
}

#[derive(Deserialize)]
struct HideImagesIn {
    hide_images: bool,
}

#[derive(Deserialize)]
struct KolIn {
    platform: String,
    name: String,
    external_id: String,
    category_id: Option<i64>,
    #[serde(default)]
    priority: bool,
    #[serde(default)]
    secondary: bool,
    #[serde(default)]
    original_only: bool,
}

#[derive(Deserialize)]
struct CategoryBody {
    name: String,
}

#[derive(Deserialize)]
struct PostsQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    platform: Option<String>,
    kol_id: Option<i64>,
    q: Option<String>,
}

#[derive(Deserialize)]
struct PushLogsQuery {
    limit: Option<i64>,
    user_id: Option<i64>,
    channel: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct RegisterCodeGen {
    count: Option<i64>,
    note: Option<String>,
    expires_in_days: Option<i64>,
}

#[derive(Deserialize)]
struct RegisterCodeBatch {
    codes: Vec<String>,
    action: String,
}

#[derive(Deserialize)]
struct AuditQuery {
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct ErrorLogsQuery {
    limit: Option<i64>,
    level: Option<String>,
    q: Option<String>,
}

fn clamp_limit(value: Option<i64>, default: i64) -> i64 {
    value.unwrap_or(default).clamp(1, 100)
}

async fn catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CatalogQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let rows = state
        .db
        .catalog(
            user.id,
            user.is_admin,
            q.platform.as_deref().unwrap_or(""),
            q.category_id.unwrap_or(0),
        )
        .await
        .map_err(db_err)?;
    Ok(Json(without_hidden(
        rows,
        &state.db.plaza_sources().await.map_err(db_err)?,
    )))
}

async fn recommendations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FlagQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let rows = state
        .db
        .recommendations(user.id, user.is_admin, q.unsubscribed.unwrap_or(0) != 0)
        .await
        .map_err(db_err)?;
    Ok(Json(without_hidden(
        rows,
        &state.db.plaza_sources().await.map_err(db_err)?,
    )))
}

async fn categories(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_user(&state, &headers).await?;
    Ok(Json(state.db.categories().await.map_err(db_err)?))
}

async fn add_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CategoryBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let id = state
        .db
        .add_category(&body.name)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "id": id })))
}

async fn rename_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(category_id): UrlPath<i64>,
    Json(body): Json<CategoryBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .rename_category(category_id, &body.name)
            .await
            .map_err(catalog_err)?,
    ))
}

async fn remove_category(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(category_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    state
        .db
        .delete_category(category_id)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn list_posts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PostsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_posts(
                q.limit.unwrap_or(100).clamp(1, 500),
                q.offset.unwrap_or(0),
                q.platform.as_deref().unwrap_or(""),
                q.kol_id,
                q.q.as_deref().unwrap_or(""),
            )
            .await
            .map_err(db_err)?,
    ))
}

async fn list_push_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<PushLogsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_push_logs(
                q.limit.unwrap_or(100).clamp(1, 500),
                q.user_id,
                q.channel.as_deref().unwrap_or(""),
                q.status.as_deref().unwrap_or(""),
            )
            .await
            .map_err(db_err)?,
    ))
}

async fn tags(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    require_user(&state, &headers).await?;
    Ok(Json(tags::snapshot(&state.db).await.map_err(db_err)?))
}

async fn save_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = tags::save(&state.db, &body).await.map_err(catalog_err)?;
    state
        .db
        .add_admin_log(admin.id, "tags.update", "", "")
        .await
        .map_err(db_err)?;
    Ok(Json(saved))
}

async fn maintain_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TagMaintainBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let mode = body.backfill.unwrap_or_else(|| "none".into());
    let result = tags::maintain(&state.db, &mode)
        .await
        .map_err(catalog_err)?;
    state
        .db
        .add_admin_log(admin.id, "maintain_post_tags", "", &mode)
        .await
        .map_err(db_err)?;
    Ok(Json(result))
}

async fn backfill_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TagBackfillBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let mode = body.mode.unwrap_or_else(|| "pending".into());
    let result = tags::backfill(&state.db, &mode)
        .await
        .map_err(catalog_err)?;
    state
        .db
        .add_admin_log(admin.id, "backfill_post_tags", "", &mode)
        .await
        .map_err(db_err)?;
    Ok(Json(result))
}

async fn feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FeedQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let platform = q.platform.unwrap_or_default();
    let query = q.q.unwrap_or_default();
    let tag = q.tag.unwrap_or_default();
    let filter = FeedFilter {
        limit: clamp_limit(q.limit, 100),
        offset: q.offset.unwrap_or(0).max(0),
        platform: &platform,
        category_id: q.category_id.unwrap_or(0),
        q: &query,
        favorite: q.favorite.unwrap_or(0) != 0,
        tag: &tag,
        include_secondary: q.include_secondary.unwrap_or(0) != 0,
        since_id: q.since_id.unwrap_or(0),
    };
    let rows = state
        .db
        .feed(user.id, user.is_admin, &filter)
        .await
        .map_err(db_err)?;
    Ok(Json(without_hidden(
        rows,
        &state.db.plaza_sources().await.map_err(db_err)?,
    )))
}

async fn subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SubIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .subscribe(user.id, user.is_admin, body.kol_id, &body.kind)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn unsubscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .unsubscribe(user.id, kol_id)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn set_sub_type(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Json(body): Json<TypeIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .set_subscription_type(user.id, kol_id, &body.kind)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn set_favorite(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Json(body): Json<FavoriteIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .set_favorite(user.id, kol_id, body.favorite)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn set_secondary(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Json(body): Json<SecondaryIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .set_secondary(user.id, kol_id, body.secondary)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn set_hide_images(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Json(body): Json<HideImagesIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .set_hide_images(user.id, kol_id, body.hide_images)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn my_subscriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let rows = state.db.my_subscriptions(user.id).await.map_err(db_err)?;
    Ok(Json(without_hidden(
        rows,
        &state.db.plaza_sources().await.map_err(db_err)?,
    )))
}

fn without_hidden(rows: Vec<Value>, sources: &[Value]) -> Vec<Value> {
    rows.into_iter()
        .filter(|row| {
            let platform = row["platform"].as_str().unwrap_or("");
            !sources
                .iter()
                .any(|source| source["platform"] == platform && source["visible"] == false)
        })
        .collect()
}

#[derive(Deserialize)]
struct KolRequestIn {
    platform: String,
    external_id: String,
    #[serde(default)]
    name: String,
    category_id: Option<i64>,
}

#[derive(Deserialize)]
struct RequestQuery {
    status: Option<String>,
}

async fn create_kol_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KolRequestIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .add_kol_request(
            &body.platform,
            &body.external_id,
            user.id,
            &body.name,
            body.category_id,
        )
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn my_kol_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_kol_requests("", user.id)
            .await
            .map_err(db_err)?,
    ))
}

async fn admin_kol_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<RequestQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    let status = q.status.unwrap_or_default();
    Ok(Json(
        state
            .db
            .list_kol_requests(&status, 0)
            .await
            .map_err(db_err)?,
    ))
}

async fn approve_kol_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(request_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let kol_id = state
        .db
        .approve_kol_request(request_id)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true, "kol_id": kol_id })))
}

async fn reject_kol_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(request_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    state
        .db
        .reject_kol_request(request_id)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct ImgQuery {
    url: String,
}

async fn img_proxy(
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Query(q): Query<ImgQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let target = img_proxy::validate(&q.url).map_err(img_err)?;
    let host = target.host.clone();
    let ips = tokio::task::spawn_blocking(move || img_proxy::resolve(&host))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "图片源请求失败"))?;
    if !img_proxy::resolution_ok(&ips) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "不支持的图片地址"));
    }
    img_proxy::take_quota(target.video, &ip, now_secs()).map_err(img_err)?;
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let proxied = tokio::task::spawn_blocking(move || img_proxy::fetch(&target, range.as_deref()))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "图片源请求失败"))?
        .map_err(img_err)?;
    let mut response = Response::builder()
        .status(proxied.status)
        .header(header::CONTENT_TYPE, proxied.media_type);
    for (name, value) in proxied.headers {
        response = response.header(name, value);
    }
    response
        .body(Body::from(proxied.body))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "图片源请求失败"))
}

fn img_err(err: img_proxy::ImgError) -> ApiError {
    let mut response = ApiError::new(
        StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST),
        err.detail,
    );
    response.retry_after = err.retry_after;
    response
}

#[derive(Deserialize)]
struct ImgbedIn {
    base_url: Option<String>,
    token: Option<String>,
    channel_name: Option<String>,
    folder: Option<String>,
    retention_days: Option<i64>,
}

async fn imgbed_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(imgbed::status(&state.db).await.map_err(db_err)?))
}

async fn save_imgbed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ImgbedIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = imgbed::save(
        &state.db,
        imgbed::Input {
            base_url: body.base_url.as_deref().unwrap_or(""),
            token: body.token.as_deref().unwrap_or(""),
            channel_name: body.channel_name.as_deref().unwrap_or(""),
            folder: body.folder.as_deref().unwrap_or(""),
            retention_days: body.retention_days,
        },
        imgbed::probe,
    )
    .await
    .map_err(imgbed_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "set_imgbed",
            "",
            saved["base_url"].as_str().unwrap_or(""),
        )
        .await;
    Ok(Json(saved))
}

async fn clear_imgbed(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = imgbed::clear(&state.db).await.map_err(db_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "clear_imgbed", "", "")
        .await;
    Ok(Json(saved))
}

fn imgbed_err(err: imgbed::ImgbedError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST),
        err.detail,
    )
}

async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(state.db.admin_stats().await.map_err(db_err)?))
}

async fn admin_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(state.db.dashboard().await.map_err(db_err)?))
}

async fn admin_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_admin_logs(q.limit.unwrap_or(100))
            .await
            .map_err(db_err)?,
    ))
}

async fn error_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ErrorLogsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_error_logs(
                q.limit.unwrap_or(200),
                q.level.as_deref().unwrap_or(""),
                q.q.as_deref().unwrap_or(""),
            )
            .await
            .map_err(catalog_err)?,
    ))
}

async fn system_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ErrorLogsQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let lines = syslogs::recent(
        q.limit.unwrap_or(200),
        q.level.as_deref().unwrap_or(""),
        q.q.as_deref().unwrap_or("").trim(),
    )
    .map_err(|detail| ApiError::new(StatusCode::BAD_REQUEST, detail))?;
    Ok(Json(json!({"lines": lines})))
}

async fn polling_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(state.db.polling_config().await.map_err(db_err)?))
}

async fn update_polling(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state.db.update_polling(&body).await.map_err(catalog_err)?,
    ))
}

#[derive(Deserialize)]
struct CookieIn {
    cookie: String,
}

async fn save_xueqiu_cookie(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CookieIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let cookie = body.cookie.trim();
    if cookie.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "cookie 不能为空"));
    }
    state
        .db
        .save_cookie("xueqiu_cookie", cookie)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn save_twitter_cookie(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CookieIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let cookie = body.cookie.trim();
    if cookie.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "cookie 不能为空"));
    }
    if !cookie.contains("auth_token=") || !cookie.contains("ct0=") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "X Cookie 需包含 auth_token 与 ct0",
        ));
    }
    state
        .db
        .save_cookie("twitter_cookie", cookie)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn save_zsxq_cookie(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CookieIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let cookie = db::zsxq_cookie_value(&body.cookie);
    if cookie.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "cookie 不能为空"));
    }
    state
        .db
        .save_cookie("zsxq_cookie", &cookie)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn clear_cookie(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    state.db.clear_cookie(&kind).await.map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn list_register_codes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(state.db.list_register_codes().await.map_err(db_err)?))
}

async fn generate_register_codes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterCodeGen>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let made = state
        .db
        .generate_register_codes(
            body.count.unwrap_or(5),
            body.note.as_deref().unwrap_or(""),
            body.expires_in_days,
            admin.id,
        )
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "generate_register_codes",
            made["batch_id"].as_str().unwrap_or(""),
            &made["count"].to_string(),
        )
        .await;
    Ok(Json(made))
}

async fn revoke_register_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(code): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    state
        .db
        .revoke_register_code(&code)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "revoke_register_code", &code, "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn revoke_register_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(batch_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let count = state
        .db
        .revoke_unused_batch(&batch_id)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "revoke_register_batch",
            &batch_id,
            &count.to_string(),
        )
        .await;
    Ok(Json(json!({"ok": true, "count": count})))
}

async fn register_codes_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RegisterCodeBatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let (count, skipped) = state
        .db
        .register_codes_batch(&body.action, &body.codes)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "register_codes_batch",
            &body.action,
            &count.to_string(),
        )
        .await;
    Ok(Json(
        json!({"ok": true, "count": count, "skipped": skipped}),
    ))
}

async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(state.db.list_admin_users().await.map_err(db_err)?))
}

#[derive(Deserialize)]
struct UserUpdate {
    username: Option<String>,
    password: Option<String>,
    is_admin: Option<bool>,
}

async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(user_id): UrlPath<i64>,
    Json(body): Json<UserUpdate>,
) -> Result<Json<Value>, ApiError> {
    let actor = require_admin(&state, &headers).await?;
    let username = match body.username {
        Some(name) => Some(
            auth::validate_username(&name)
                .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?,
        ),
        None => None,
    };
    let password_hash = match body.password {
        Some(password) if password.len() < auth::PASSWORD_MIN => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("密码至少{}位", auth::PASSWORD_MIN),
            ))
        }
        Some(password) if password.len() > auth::PASSWORD_MAX => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("密码最长{}位", auth::PASSWORD_MAX),
            ))
        }
        Some(password) => Some(auth::hash_password(&password, None)),
        None => None,
    };
    state
        .db
        .update_admin_user(
            actor.id,
            user_id,
            username.as_deref(),
            password_hash.as_deref(),
            body.is_admin,
        )
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(user_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let actor = require_admin(&state, &headers).await?;
    state
        .db
        .delete_user(actor.id, user_id)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct BatchIn {
    action: String,
    ids: Vec<i64>,
}

async fn users_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BatchIn>,
) -> Result<Json<Value>, ApiError> {
    let actor = require_admin(&state, &headers).await?;
    if body.ids.is_empty() {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "请先选择用户"));
    }
    let (count, skipped) = match body.action.as_str() {
        "enable_notify" | "disable_notify" => {
            let changed = state
                .db
                .set_notify_many(&body.ids, body.action == "enable_notify")
                .await
                .map_err(db_err)?;
            (changed, (body.ids.len() as i64 - changed).max(0))
        }
        "delete" => {
            let mut count = 0;
            let mut skipped = 0;
            for id in body.ids {
                match state.db.delete_user(actor.id, id).await {
                    Ok(()) => count += 1,
                    Err(_) => skipped += 1,
                }
            }
            (count, skipped)
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("不支持的操作: {}", body.action),
            ))
        }
    };
    Ok(Json(
        json!({ "ok": true, "count": count, "skipped": skipped }),
    ))
}

#[derive(Deserialize)]
struct TestPushIn {
    user_id: i64,
    #[serde(default)]
    message: String,
}

async fn test_push(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TestPushIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let user = state
        .db
        .user_by_id(body.user_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "用户不存在"))?;
    let message = if body.message.trim().is_empty() {
        "这是一条测试推送"
    } else {
        body.message.trim()
    };
    let results = push::test_user(&state.db, &user, message)
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    Ok(Json(json!({"results": results})))
}

#[derive(Deserialize)]
struct InactiveQuery {
    inactive_after_days: Option<i64>,
    inactive_purge_after_days: Option<i64>,
}

#[derive(Deserialize)]
struct InactiveIn {
    inactive_after_days: i64,
    inactive_purge_after_days: i64,
}

async fn inactive_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<InactiveQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .inactive_policy(q.inactive_after_days, q.inactive_purge_after_days)
            .await
            .map_err(catalog_err)?,
    ))
}

async fn save_inactive_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InactiveIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .set_inactive_policy(body.inactive_after_days, body.inactive_purge_after_days)
            .await
            .map_err(catalog_err)?,
    ))
}

async fn plaza_sources(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        json!({"sources": state.db.plaza_sources().await.map_err(db_err)?}),
    ))
}

#[derive(Deserialize)]
struct PlazaIn {
    #[serde(default)]
    visibility: Map<String, Value>,
}

async fn save_plaza_sources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PlazaIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let sources = state
        .db
        .set_plaza_visibility(&body.visibility)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({"sources": sources})))
}

#[derive(Deserialize)]
struct FeishuAssetQuery {
    group: Option<String>,
}

#[derive(Deserialize)]
struct ImaListQuery {
    q: Option<String>,
    day: Option<String>,
    group: Option<String>,
    tag: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    facets_only: Option<i64>,
}

async fn feishu_documents(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ImaListQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_default();
    let query = q.q.unwrap_or_default();
    let day = q.day.unwrap_or_default();
    let tag = q.tag.unwrap_or_default();
    if group.len() > 128 || query.len() > 200 || day.len() > 64 || tag.len() > 64 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    match feishu_docs::list_documents(
        &state.db,
        user.id,
        user.is_admin,
        &group,
        &query,
        &day,
        &tag,
        q.limit.unwrap_or(50),
        q.offset.unwrap_or(0),
        q.facets_only.unwrap_or(0) == 1,
    )
    .await
    {
        Ok(value) => Ok(Json(value)),
        Err("读取文档失败") => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "读取文档失败",
        )),
        Err(msg) => Err(ApiError::new(StatusCode::NOT_FOUND, msg)),
    }
}

fn ima_group_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b':' | b'-'))
}

async fn subscribe_ima_kb(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(group_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if !ima_group_id(&group_id) || !state.db.ima_group_known(&group_id).await.map_err(db_err)? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "知识库不存在"));
    }
    if !user.is_admin && !group_id.starts_with("feishu-") {
        let (acl, _) = state.db.ima_access(user.id).await.map_err(db_err)?;
        if !acl.contains(&group_id) {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "知识库不存在"));
        }
    }
    state
        .db
        .ima_kb_subscribe(user.id, &group_id)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn unsubscribe_ima_kb(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(group_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if ima_group_id(&group_id) {
        state
            .db
            .ima_kb_unsubscribe(user.id, &group_id)
            .await
            .map_err(db_err)?;
    }
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct ImaAclBody {
    usernames: Vec<String>,
}

async fn set_ima_kb_acl(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(group_id): UrlPath<String>,
    Json(body): Json<ImaAclBody>,
) -> Result<Json<Value>, ApiError> {
    let _admin = require_admin(&state, &headers).await?;
    if !ima_group_id(&group_id) || !state.db.ima_group_known(&group_id).await.map_err(db_err)? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "知识库不存在"));
    }
    if body.usernames.len() > 200 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "用户过多"));
    }
    let mut ids = Vec::new();
    for name in &body.usernames {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "用户不存在"));
        }
        let Some(user) = state.db.user_by_username(name).await.map_err(db_err)? else {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("用户不存在: {name}"),
            ));
        };
        if !ids.contains(&user.id) {
            ids.push(user.id);
        }
    }
    state
        .db
        .set_ima_kb_acl(&group_id, &ids)
        .await
        .map_err(db_err)?;
    let names = state
        .db
        .ima_kb_acl_usernames(&group_id)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({"ok": true, "acl_usernames": names})))
}

#[derive(Deserialize)]
struct ImaUserKbBody {
    group_ids: Vec<String>,
}

async fn set_user_ima_kb(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(user_id): UrlPath<i64>,
    Json(body): Json<ImaUserKbBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let Some(target) = state.db.user_by_id(user_id).await.map_err(db_err)? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "用户不存在"));
    };
    if target.is_admin {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "管理员可直接打开全部知识库",
        ));
    }
    if body.group_ids.len() > 200 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "知识库过多"));
    }
    let mut ids = Vec::new();
    for raw in &body.group_ids {
        let value = raw.trim();
        if value.is_empty()
            || value.starts_with("feishu-")
            || ids.iter().any(|item: &String| item == value)
        {
            continue;
        }
        if !ima_group_id(value) || !state.db.ima_group_known(value).await.map_err(db_err)? {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "知识库不存在"));
        }
        ids.push(value.to_string());
    }
    state
        .db
        .set_ima_kb_acl_for_user(user_id, &ids)
        .await
        .map_err(db_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "set_user_ima_kb",
            &user_id.to_string(),
            &ids.join(","),
        )
        .await;
    Ok(Json(json!({
        "ok": true,
        "ima_kb_groups": state.db.ima_kb_group_ids_for_user(user_id).await.map_err(db_err)?,
        "ima_kb_subscribed": state.db.ima_kb_subscribed_group_ids_for_user(user_id).await.map_err(db_err)?,
    })))
}

async fn feishu_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    match feishu_docs::catalog(&state.db, user.id, user.is_admin).await {
        Ok(value) => Ok(Json(value)),
        Err(msg) => Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, msg)),
    }
}

#[derive(Deserialize)]
struct ImaTickerQuery {
    group: Option<String>,
    limit: Option<i64>,
}

async fn ima_ticker(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(code): UrlPath<String>,
    Query(q): Query<ImaTickerQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let requested = q.group.unwrap_or_default();
    if requested.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    let readable = ticker_groups(&state, user.id, user.is_admin, &requested).await?;
    state
        .db
        .ima_ticker_page(&code, &readable, q.limit.unwrap_or(100))
        .await
        .map(Json)
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "读取文档失败"))
}

async fn ticker_groups(
    state: &AppState,
    user_id: i64,
    is_admin: bool,
    requested: &str,
) -> Result<Vec<String>, ApiError> {
    let configured =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT group_id FROM ima_document_index")
            .fetch_all(state.db.pool())
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "读取文档失败"))?;
    let readable = if is_admin {
        configured
    } else {
        let (acl, subscribed) = state
            .db
            .ima_access(user_id)
            .await
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "读取文档失败"))?;
        configured
            .into_iter()
            .filter(|group| acl.contains(group) && subscribed.contains(group))
            .collect()
    };
    if !requested.is_empty() && !readable.iter().any(|group| group == requested) {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "知识库不存在"));
    }
    Ok(readable)
}

async fn ima_translate(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(media_id): UrlPath<String>,
    Query(q): Query<ImaFileQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_default();
    if group.len() > 128 || media_id.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    match feishu_docs::translate_document(
        &state.db,
        user.id,
        user.is_admin,
        &media_id,
        &group,
        &archive_root(),
        feishu_docs::my_memory,
    )
    .await
    {
        Ok(value) => Ok(Json(value)),
        Err("读取文档失败") => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "读取文档失败",
        )),
        Err(msg) => Err(ApiError::new(StatusCode::NOT_FOUND, msg)),
    }
}

async fn feishu_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(media_id): UrlPath<String>,
    Query(q): Query<FeishuAssetQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_default();
    if group.len() > 128 || media_id.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    let root = archive_root();
    match feishu_docs::document_meta(&state.db, user.id, user.is_admin, &media_id, &group, &root)
        .await
    {
        Ok(value) => Ok(Json(value)),
        Err("读取文档失败") => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "读取文档失败",
        )),
        Err(msg) => Err(ApiError::new(StatusCode::NOT_FOUND, msg)),
    }
}

#[derive(Deserialize)]
struct ImaFileQuery {
    group: Option<String>,
    download: Option<i64>,
}

fn archive_root() -> PathBuf {
    std::env::var("IMA_ARCHIVE_ROOT")
        .or_else(|_| std::env::var("FEISHU_ARCHIVE_ROOT"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn content_disposition(download: bool, name: &str) -> String {
    let kind = if download { "attachment" } else { "inline" };
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("{kind}; filename=\"document\"; filename*=UTF-8''{encoded}")
}

async fn ima_pdf(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(media_id): UrlPath<String>,
    Query(q): Query<ImaFileQuery>,
) -> Result<Response, ApiError> {
    serve_ima_file(
        &state,
        &headers,
        &media_id,
        q.group.unwrap_or_default(),
        "pdf",
        q.download.unwrap_or(0) == 1,
    )
    .await
}

async fn ima_text(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(media_id): UrlPath<String>,
    Query(q): Query<ImaFileQuery>,
) -> Result<Response, ApiError> {
    serve_ima_file(
        &state,
        &headers,
        &media_id,
        q.group.unwrap_or_default(),
        "txt",
        false,
    )
    .await
}

async fn serve_ima_file(
    state: &AppState,
    headers: &HeaderMap,
    media_id: &str,
    group: String,
    kind: &str,
    download: bool,
) -> Result<Response, ApiError> {
    let user = require_user(state, headers).await?;
    if group.len() > 128 || media_id.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    let (path, name) = match feishu_docs::archive_file(
        &state.db,
        &archive_root(),
        user.id,
        user.is_admin,
        media_id,
        &group,
        kind,
    )
    .await
    {
        Ok(file) => file,
        Err("读取文档失败") => {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "读取文档失败",
            ))
        }
        Err(msg) => return Err(ApiError::new(StatusCode::NOT_FOUND, msg)),
    };
    let bytes = std::fs::read(&path).map_err(|_| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            if kind == "pdf" {
                "PDF 文件不存在"
            } else {
                "TXT 文件不存在"
            },
        )
    })?;
    let mime = if kind == "pdf" {
        "application/pdf"
    } else {
        "text/plain; charset=utf-8"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(
            header::CONTENT_DISPOSITION,
            content_disposition(download, &name),
        )
        .body(Body::from(bytes))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "读取文档失败"))
}

async fn feishu_asset(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((media_id, asset_id)): UrlPath<(String, String)>,
    Query(q): Query<FeishuAssetQuery>,
) -> Result<Response, ApiError> {
    let user = require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_default();
    if group.len() > 128 || media_id.len() > 128 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    let root = archive_root();
    let path = match feishu_docs::asset_file(
        &state.db,
        &root,
        user.id,
        user.is_admin,
        &media_id,
        &group,
        &asset_id,
    )
    .await
    {
        Ok(path) => path,
        Err("读取文档失败") => {
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "读取文档失败",
            ))
        }
        Err(msg) => return Err(ApiError::new(StatusCode::NOT_FOUND, msg)),
    };
    let bytes = std::fs::read(&path)
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "飞书文档资源不存在"))?;
    Response::builder()
        .header(header::CONTENT_TYPE, feishu_docs::asset_type(&path))
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from(bytes))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "读取文档失败"))
}

#[derive(Deserialize)]
struct FeishuTimelineQuery {
    order: Option<String>,
    group: Option<String>,
    window_days: Option<i64>,
    before: Option<String>,
}

async fn feishu_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FeishuTimelineQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let group = q.group.unwrap_or_default();
    let before = q.before.unwrap_or_default();
    if group.len() > 128 || before.len() > 512 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "参数过长"));
    }
    let root = archive_root();
    match feishu_docs::timeline_all(
        &state.db,
        &root,
        user.id,
        user.is_admin,
        &group,
        q.order.as_deref().unwrap_or("latest"),
        q.window_days,
        &before,
    )
    .await
    {
        Ok(value) => Ok(Json(value)),
        Err("文档不存在") => Err(ApiError::new(StatusCode::NOT_FOUND, "文档不存在")),
        Err("读取文档失败") => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "读取文档失败",
        )),
        Err(msg) => Err(ApiError::new(StatusCode::BAD_REQUEST, msg)),
    }
}

#[derive(Deserialize)]
struct CiccTriggerBody {
    mode: String,
}

#[derive(Deserialize)]
struct CiccScheduleBody {
    enabled: bool,
    time: Option<String>,
}

#[derive(Deserialize)]
struct CiccCategoriesBody {
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

#[derive(Deserialize)]
struct NewsQuery {
    source_id: Option<i64>,
    q: Option<String>,
    unread: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn xincai_ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let expected = std::env::var("XINCAI_INGEST_TOKEN").unwrap_or_default();
    let header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    match xincai::authorize(header, &expected) {
        Err("未配置 XINCAI_INGEST_TOKEN") => {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "未配置 XINCAI_INGEST_TOKEN",
            ))
        }
        Err(msg) => return Err(ApiError::new(StatusCode::UNAUTHORIZED, msg)),
        Ok(()) => {}
    }
    match xincai::ingest(&state.db, &body).await {
        Ok(value) => Ok(Json(value)),
        Err(CatalogError::Bad(msg)) => Err(ApiError::new(StatusCode::BAD_REQUEST, msg)),
        Err(CatalogError::Invalid(msg)) => Err(ApiError::new(StatusCode::BAD_REQUEST, msg)),
        Err(CatalogError::Missing(msg)) => Err(ApiError::new(StatusCode::BAD_REQUEST, msg)),
        Err(CatalogError::Limited(msg)) => Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, msg)),
        Err(CatalogError::Conflict(_)) => {
            Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "入库失败"))
        }
        Err(CatalogError::Db(err)) => {
            tracing::warn!("心裁入库失败: {err}");
            Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "入库失败"))
        }
    }
}

fn cicc_control() -> Result<cicc::Control, ApiError> {
    cicc::from_env()
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "当前部署未挂载存储归档"))
}

fn cicc_err(err: cicc::CiccError) -> ApiError {
    match err {
        cicc::CiccError::Isolated => {
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "知识库存储暂不可用")
        }
        cicc::CiccError::Bad(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
        cicc::CiccError::Invalid(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
    }
}

#[derive(Deserialize)]
struct ImaCredentialIn {
    cookie: Option<String>,
    openapi_clientid: Option<String>,
    openapi_apikey: Option<String>,
}

#[derive(Deserialize)]
struct LocalLibraryIn {
    slug: Option<String>,
    name: Option<String>,
    tags: Option<Vec<String>>,
    enabled: Option<bool>,
}

async fn ima_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        ima_admin::credentials(&state.db).await.map_err(db_err)?,
    ))
}

async fn save_ima_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ImaCredentialIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    ima_admin::save_credentials(
        &state.db,
        body.cookie.as_deref().unwrap_or(""),
        body.openapi_clientid.as_deref().unwrap_or(""),
        body.openapi_apikey.as_deref().unwrap_or(""),
    )
    .await
    .map_err(ima_admin_err)?;
    let openapi = !body
        .openapi_clientid
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty();
    let cookie = !body.cookie.as_deref().unwrap_or("").trim().is_empty();
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "set_ima_credentials",
            "",
            &format!("cookie={cookie} openapi={openapi}"),
        )
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn ima_local_libraries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        ima_admin::libraries(&state.db)
            .await
            .map_err(ima_admin_err)?,
    ))
}

async fn scan_ima_local_libraries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let root = ima_admin::archive_root().map_err(ima_admin_err)?;
    let result = ima_admin::scan(&state.db, &root)
        .await
        .map_err(ima_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "scan_ima_local_libraries",
            "",
            result["status"].as_str().unwrap_or(""),
        )
        .await;
    Ok(Json(result))
}

async fn create_ima_local_library(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LocalLibraryIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let root = ima_admin::archive_root().map_err(ima_admin_err)?;
    let tags = body.tags.unwrap_or_default();
    let result = ima_admin::create(
        &state.db,
        &root,
        body.slug.as_deref().unwrap_or(""),
        body.name.as_deref().unwrap_or(""),
        &tags,
    )
    .await
    .map_err(ima_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "create_ima_local_library",
            body.slug.as_deref().unwrap_or(""),
            "",
        )
        .await;
    Ok(Json(result))
}

async fn set_ima_local_enabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(slug): UrlPath<String>,
    Json(body): Json<LocalLibraryIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let enabled = body
        .enabled
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "enabled 必填"))?;
    let root = ima_admin::archive_root().map_err(ima_admin_err)?;
    let result = ima_admin::set_enabled(&state.db, &root, &slug, enabled)
        .await
        .map_err(ima_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "set_ima_local_library_enabled",
            &slug,
            if enabled { "enabled" } else { "disabled" },
        )
        .await;
    Ok(Json(result))
}

async fn update_ima_local_library(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(slug): UrlPath<String>,
    Json(body): Json<LocalLibraryIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let root = ima_admin::archive_root().map_err(ima_admin_err)?;
    let result = ima_admin::update(
        &state.db,
        &root,
        &slug,
        body.name.as_deref(),
        body.tags.as_deref(),
    )
    .await
    .map_err(ima_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "update_ima_local_library", &slug, "")
        .await;
    Ok(Json(result))
}

fn ima_admin_err(err: ima_admin::AdminError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST),
        err.detail,
    )
}

async fn ima_collector_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    ima_collector::status(&state.db)
        .await
        .map(Json)
        .map_err(|err| {
            tracing::warn!("IMA 采集状态读取失败: {err}");
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "IMA 采集状态读取失败")
        })
}

async fn save_ima_collector(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ima_collector::SaveBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    ima_collector::save(&state.db, &body)
        .await
        .map(Json)
        .map_err(collector_err)
}

async fn discover_ima_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    ima_collector::discover(&state.db)
        .await
        .map(Json)
        .map_err(collector_err)
}

async fn sync_ima_collector(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ima_collector::SyncBody>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    ima_collector::sync(&state.db, &body)
        .await
        .map(Json)
        .map_err(collector_err)
}

fn collector_err(err: ima_collector::CollectorError) -> ApiError {
    match err {
        ima_collector::CollectorError::Bad(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
        ima_collector::CollectorError::NotFound(msg) => ApiError::new(StatusCode::NOT_FOUND, msg),
        ima_collector::CollectorError::Conflict(msg) => ApiError::new(StatusCode::CONFLICT, msg),
        ima_collector::CollectorError::TooSoon => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "距离上次同步时间太短，请稍后再试",
        ),
        ima_collector::CollectorError::Unavailable(msg) => {
            ApiError::new(StatusCode::SERVICE_UNAVAILABLE, msg)
        }
        ima_collector::CollectorError::Upstream(msg) => ApiError::new(StatusCode::BAD_GATEWAY, msg),
    }
}

#[derive(Deserialize)]
struct ImaFolderQuery {
    parent_id: Option<String>,
}

async fn ima_group_folders(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(group_id): UrlPath<String>,
    Query(q): Query<ImaFolderQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    ima_collector::folders(&state.db, &group_id, q.parent_id.as_deref().unwrap_or(""))
        .await
        .map(Json)
        .map_err(collector_err)
}

async fn ima_arm(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    arm::admin_status(&state.db).await.map(Json).map_err(|err| {
        tracing::warn!("ARM 状态读取失败: {err}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "ARM 状态读取失败")
    })
}

#[derive(Deserialize)]
struct QridQuery {
    qrid: Option<String>,
}

async fn weibo_qr_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    weibo_qr::start().await.map(Json).map_err(weibo_qr_err)
}

async fn weibo_qr_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<QridQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    weibo_qr::status(&state.db, query.qrid.as_deref().unwrap_or(""))
        .await
        .map(Json)
        .map_err(weibo_qr_err)
}

fn weibo_qr_err(err: weibo_qr::QrFail) -> ApiError {
    match err {
        weibo_qr::QrFail::Missing => {
            ApiError::new(StatusCode::NOT_FOUND, "二维码已过期，请重新生成")
        }
        weibo_qr::QrFail::Bad(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
        weibo_qr::QrFail::Detail(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
    }
}

async fn cicc_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(cicc_control()?.status()))
}

async fn cicc_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CiccTriggerBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    cicc_control()?
        .trigger(&body.mode, &admin.username, None)
        .map(Json)
        .map_err(cicc_err)
}

async fn cicc_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(cicc_control()?.read_schedule()))
}

async fn cicc_save_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CiccScheduleBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    if body
        .time
        .as_deref()
        .is_some_and(|time| !cicc::time_ok(time))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "时间格式应为 HH:mm（00:00-23:59）",
        ));
    }
    let ctl = cicc_control()?;
    let mut result = ctl.set_schedule(body.enabled).map_err(cicc_err)?;
    if let Some(time) = body.time.as_deref() {
        let queued = ctl
            .set_schedule_time(time, &admin.username)
            .map_err(cicc_err)?;
        if let (Some(result), Some(queued)) = (result.as_object_mut(), queued.as_object()) {
            for (key, value) in queued {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(Json(result))
}

async fn cicc_categories(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let status = cicc_control()?.status();
    let categories = if status["cicc_settings"]["categories"].is_array() {
        status["cicc_settings"]["categories"].clone()
    } else {
        serde_json::from_str(
            &state
                .db
                .setting("cicc_category_settings")
                .await
                .map_err(db_err)?
                .unwrap_or_default(),
        )
        .unwrap_or(json!([]))
    };
    let keywords = serde_json::from_str(
        &state
            .db
            .setting("cicc_keywords_key")
            .await
            .map_err(db_err)?
            .unwrap_or_default(),
    )
    .unwrap_or(json!([]));
    Ok(Json(
        json!({"categories": categories, "keywords": keywords}),
    ))
}

async fn cicc_save_categories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CiccCategoriesBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let (categories, keywords) =
        cicc::clean_lists(&body.categories, &body.keywords).map_err(cicc_err)?;
    cicc_control()?
        .set_settings(&categories, &keywords, &admin.username)
        .map_err(cicc_err)?;
    state
        .db
        .set_setting(
            "cicc_category_settings",
            &serde_json::to_string(&categories).unwrap_or_else(|_| "[]".into()),
        )
        .await
        .map_err(db_err)?;
    state
        .db
        .set_setting(
            "cicc_keywords_key",
            &serde_json::to_string(&keywords).unwrap_or_else(|_| "[]".into()),
        )
        .await
        .map_err(db_err)?;
    Ok(Json(
        json!({"categories": categories, "keywords": keywords}),
    ))
}

async fn news_sources(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        state.db.user_news_sources(user.id).await.map_err(db_err)?,
    ))
}

async fn news_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<NewsQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);
    Ok(Json(
        state
            .db
            .list_news(
                user.id,
                q.source_id.unwrap_or(0),
                q.q.as_deref().unwrap_or(""),
                q.unread.unwrap_or(0) != 0,
                limit,
                offset,
            )
            .await
            .map_err(db_err)?,
    ))
}

#[derive(Deserialize)]
struct MagazineQuery {
    source_id: i64,
}

async fn news_magazine(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<MagazineQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .magazine(user.id, q.source_id)
            .await
            .map_err(catalog_err)?,
    ))
}

async fn zsxq_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(file_id): UrlPath<String>,
    Query(q): Query<DownloadQuery>,
) -> Result<Response, ApiError> {
    let user = require_download_user(&state, &headers, q.token.as_deref()).await?;
    let file = zsxq_file::read_file(&state.db, user.id, user.is_admin, &file_id)
        .await
        .map_err(|err| {
            ApiError::new(
                StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_GATEWAY),
                err.detail,
            )
        })?;
    let mut response = Response::new(Body::from(file.bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(disposition) = header::HeaderValue::from_str(&file.disposition) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, disposition);
    }
    Ok(response)
}

#[derive(Deserialize)]
struct BackupBody {
    url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    path: Option<String>,
    hour: Option<i64>,
    keep: Option<i64>,
}

fn backup_json(body: BackupBody) -> Value {
    let mut value = json!({});
    if let Some(url) = body.url {
        value["url"] = json!(url);
    }
    if let Some(username) = body.username {
        value["username"] = json!(username);
    }
    if let Some(password) = body.password {
        value["password"] = json!(password);
    }
    if let Some(path) = body.path {
        value["path"] = json!(path);
    }
    if let Some(hour) = body.hour {
        value["hour"] = json!(hour);
    }
    if let Some(keep) = body.keep {
        value["keep"] = json!(keep);
    }
    value
}

fn backup_err(err: backup::BackupError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST),
        err.detail,
    )
}

async fn backup_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    backup::status(&state.db)
        .await
        .map(Json)
        .map_err(backup_err)
}

async fn backup_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BackupBody>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let body = backup_json(body);
    let status = backup::save(&state.db, &body).await.map_err(backup_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "backup_webdav_save",
            body["url"].as_str().unwrap_or(""),
            "",
        )
        .await;
    Ok(Json(status))
}

async fn backup_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<BackupBody>>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let value = body.map(|Json(body)| backup_json(body));
    backup::test_webdav(&state.db, value.as_ref())
        .await
        .map_err(backup_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn backup_download(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_admin(&state, &headers).await?;
    let path = backup::snapshot(&state.db).await.map_err(backup_err)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "备份校验失败，请稍后重试",
        )
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("dav-backup.db");
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(disposition) =
        header::HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
    {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, disposition);
    }
    Ok(response)
}

async fn backup_restore_webdav(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    backup::restore_webdav(&state.db)
        .await
        .map_err(backup_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "backup_restore", "webdav", "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn backup_restore_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let mut data = Vec::new();
    let mut name = String::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "请上传有效的 .db 备份文件"))?
    {
        if field.name() != Some("file") {
            continue;
        }
        name = field.file_name().unwrap_or("").to_string();
        data = field
            .bytes()
            .await
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "请上传有效的 .db 备份文件"))?
            .to_vec();
        break;
    }
    if !name.to_ascii_lowercase().ends_with(".db") {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "请上传有效的 .db 备份文件",
        ));
    }
    backup::restore_bytes(&state.db, &data)
        .await
        .map_err(backup_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "backup_restore", "upload", &name)
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn purge_zsxq_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let result = zsxq_file::purge(&state.db).await.map_err(|err| {
        ApiError::new(
            StatusCode::from_u16(err.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            err.detail,
        )
    })?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "purge_zsxq_cache",
            "",
            &format!("deleted={}", result["deleted"].as_i64().unwrap_or(0)),
        )
        .await;
    Ok(Json(result))
}

async fn news_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((article_id, index)): UrlPath<(i64, i64)>,
) -> Result<Response, ApiError> {
    let user = require_user(&state, &headers).await?;
    let stored = state
        .db
        .news_image_url(user.id, article_id, index)
        .await
        .map_err(catalog_err)?;
    let target = news::image_target(&stored)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "图片地址不安全或类型不受支持"))?;
    let host = target.host.clone();
    let ips = tokio::task::spawn_blocking(move || img_proxy::resolve(&host))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "图片暂时无法加载"))?;
    if !img_proxy::resolution_ok(&ips) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "图片地址不安全或类型不受支持",
        ));
    }
    let image = tokio::task::spawn_blocking(move || news::fetch_image(&target.url))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "图片暂时无法加载"))?
        .map_err(|err| match err {
            news::ImageFail::Unsafe => {
                ApiError::new(StatusCode::BAD_REQUEST, "图片地址不安全或类型不受支持")
            }
            news::ImageFail::Upstream => ApiError::new(StatusCode::BAD_GATEWAY, "图片暂时无法加载"),
        })?;
    let mut response = Response::builder()
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .header(header::ETAG, &image.etag)
        .header("x-content-type-options", "nosniff");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim() == image.etag)
    {
        response = response.status(StatusCode::NOT_MODIFIED);
        return response
            .body(Body::empty())
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "图片暂时无法加载"));
    }
    response = response
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, image.media_type);
    response
        .body(Body::from(image.body))
        .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "图片暂时无法加载"))
}

async fn news_article(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(article_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let article = state
        .db
        .news_article(user.id, article_id)
        .await
        .map_err(db_err)?;
    article
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "文章不存在"))
        .map(Json)
}

async fn news_read(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(article_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if !state
        .db
        .mark_news_read(user.id, article_id)
        .await
        .map_err(db_err)?
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "文章不存在"));
    }
    Ok(Json(json!({"ok": true})))
}

async fn news_read_all(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let (now, previous) = state.db.mark_news_seen_now(user.id).await.map_err(db_err)?;
    Ok(Json(
        json!({"ok": true, "read_all_seen_at": now, "previous_seen_at": previous}),
    ))
}

#[derive(Deserialize)]
struct SeenUndo {
    read_all_seen_at: String,
    #[serde(default)]
    previous_seen_at: String,
}

async fn news_read_all_undo(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SeenUndo>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let current = state.db.news_seen(user.id).await.map_err(db_err)?;
    if current != body.read_all_seen_at {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "已读状态已变化，请刷新后重试",
        ));
    }
    state
        .db
        .set_news_seen(user.id, &body.previous_seen_at)
        .await
        .map_err(db_err)?;
    Ok(Json(
        json!({"ok": true, "news_last_seen_at": body.previous_seen_at}),
    ))
}

#[derive(Deserialize)]
struct SeenIn {
    #[serde(default)]
    view_started_at: String,
}

async fn news_seen(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SeenIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let stamp = if body.view_started_at.trim().is_empty() {
        state
            .db
            .mark_news_seen_now(user.id)
            .await
            .map_err(db_err)?
            .0
    } else {
        state
            .db
            .set_news_seen(user.id, body.view_started_at.trim())
            .await
            .map_err(db_err)?;
        body.view_started_at.trim().to_string()
    };
    Ok(Json(json!({"ok": true, "news_last_seen_at": stamp})))
}

async fn news_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let minutes = state
        .db
        .setting("news_refresh_minutes")
        .await
        .map_err(db_err)?
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(30);
    Ok(Json(json!({
        "enabled": state.db.news_flag("news_enabled", false).await.map_err(db_err)?,
        "visible": state.db.setting("news_visible").await.map_err(db_err)?.map(|v| v != "0").unwrap_or(true),
        "refresh_interval_minutes": minutes,
    })))
}

#[derive(Deserialize)]
struct NewsSettingsIn {
    enabled: Option<bool>,
    visible: Option<bool>,
    refresh_interval_minutes: Option<i64>,
}

async fn save_news_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewsSettingsIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    if let Some(on) = body.enabled {
        state
            .db
            .set_news_flag("news_enabled", on)
            .await
            .map_err(db_err)?;
    }
    if let Some(on) = body.visible {
        state
            .db
            .set_news_flag("news_visible", on)
            .await
            .map_err(db_err)?;
    }
    if let Some(minutes) = body.refresh_interval_minutes {
        if !(5..=1440).contains(&minutes) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "刷新间隔需在 5-1440 分钟",
            ));
        }
        state
            .db
            .set_setting("news_refresh_minutes", &minutes.to_string())
            .await
            .map_err(db_err)?;
    }
    news_settings(State(state), headers).await
}

#[derive(Deserialize)]
struct ArchivedQuery {
    include_archived: Option<String>,
}

async fn admin_news_sources(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ArchivedQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let include = matches!(q.include_archived.as_deref(), Some("1") | Some("true"));
    Ok(Json(
        json!({"items": state.db.admin_news_sources_all(include).await.map_err(db_err)?}),
    ))
}

#[derive(Deserialize)]
struct NewsSourceIn {
    name: String,
    #[serde(default)]
    group_name: String,
}

async fn create_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewsSourceIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let id = state
        .db
        .add_news_source(&body.name, &body.group_name)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({"id": id})))
}

#[derive(Deserialize)]
struct NewsFeedIn {
    name: String,
    url: String,
}

async fn validate_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<NewsFeedIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let parsed = news::fetch_feed(body.url.trim())
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let entries = parsed
        .entries
        .iter()
        .take(3)
        .map(|entry| {
            json!({
                "title": entry.title,
                "published_at": entry.published_at,
                "text": entry.summary,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"format": parsed.format, "title": parsed.title, "entries": entries}),
    ))
}

async fn create_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
    Json(body): Json<NewsFeedIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let url = body.url.trim();
    let parsed = news::fetch_feed(url)
        .await
        .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
    let feed_id = state
        .db
        .add_news_feed(source_id, &body.name, url)
        .await
        .map_err(catalog_err)?;
    let added = state
        .db
        .save_news_entries(source_id, feed_id, &parsed.entries)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({"id": feed_id, "added": added})))
}

async fn refresh_news(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let ids = state.db.enabled_feed_ids(None).await.map_err(db_err)?;
    let result = news::refresh_ids(&state.db, &ids)
        .await
        .map_err(news_refresh_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_refresh", "", &result.to_string())
        .await;
    Ok(Json(result))
}

#[derive(Deserialize, Default)]
struct NewsSourcePatch {
    name: Option<String>,
    group_name: Option<String>,
    enabled: Option<bool>,
}

async fn update_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
    Json(body): Json<NewsSourcePatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = state
        .db
        .update_news_source(
            source_id,
            body.name.as_deref(),
            body.group_name.as_deref(),
            body.enabled,
        )
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_source_update", &source_id.to_string(), "")
        .await;
    Ok(Json(saved))
}

async fn archive_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    state
        .db
        .set_news_source_archived(source_id, true)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_source_archive", &source_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn restore_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    state
        .db
        .set_news_source_archived(source_id, false)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_source_restore", &source_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn delete_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    if !state
        .db
        .delete_news_source(source_id)
        .await
        .map_err(db_err)?
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "媒体不存在"));
    }
    let _ = state
        .db
        .add_admin_log(admin.id, "news_source_delete", &source_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn refresh_news_source(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(source_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let source = state
        .db
        .admin_news_source(source_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "媒体不存在"))?;
    if source["archived_at"].is_string() || source["enabled"] != true {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "媒体已停用或归档"));
    }
    let ids = state
        .db
        .enabled_feed_ids(Some(source_id))
        .await
        .map_err(db_err)?;
    let result = news::refresh_ids(&state.db, &ids)
        .await
        .map_err(news_refresh_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "news_source_refresh",
            &source_id.to_string(),
            &result.to_string(),
        )
        .await;
    Ok(Json(result))
}

#[derive(Deserialize, Default)]
struct NewsFeedPatch {
    name: Option<String>,
    url: Option<String>,
    enabled: Option<bool>,
}

async fn update_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(feed_id): UrlPath<i64>,
    Json(body): Json<NewsFeedPatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let Some((_, current, _, _)) = state.db.news_feed_target(feed_id).await.map_err(db_err)? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Feed 不存在"));
    };
    if let Some(url) = body.url.as_deref().map(str::trim) {
        if url != current {
            news::fetch_feed(url)
                .await
                .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, err))?;
        }
    }
    let saved = state
        .db
        .update_news_feed(
            feed_id,
            body.name.as_deref(),
            body.url.as_deref(),
            body.enabled,
        )
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_feed_update", &feed_id.to_string(), "")
        .await;
    Ok(Json(saved))
}

async fn archive_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(feed_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    state
        .db
        .set_news_feed_archived(feed_id, true)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_feed_archive", &feed_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn restore_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(feed_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    state
        .db
        .set_news_feed_archived(feed_id, false)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "news_feed_restore", &feed_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn delete_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(feed_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    if !state.db.delete_news_feed(feed_id).await.map_err(db_err)? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Feed 不存在"));
    }
    let _ = state
        .db
        .add_admin_log(admin.id, "news_feed_delete", &feed_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn refresh_news_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(feed_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let Some((source_id, _url, enabled, archived)) =
        state.db.news_feed_target(feed_id).await.map_err(db_err)?
    else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "Feed 不存在"));
    };
    let source = state
        .db
        .admin_news_source(source_id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "媒体不存在"))?;
    if archived || !enabled || source["archived_at"].is_string() || source["enabled"] != true {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "Feed 已停用或归档"));
    }
    let result = news::refresh_ids(&state.db, &[feed_id])
        .await
        .map_err(news_refresh_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "news_feed_refresh",
            &feed_id.to_string(),
            &result.to_string(),
        )
        .await;
    Ok(Json(result))
}

async fn delete_news_article(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(article_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    if !state
        .db
        .delete_news_article(article_id)
        .await
        .map_err(db_err)?
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "文章不存在"));
    }
    let _ = state
        .db
        .add_admin_log(admin.id, "news_article_delete", &article_id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

fn news_refresh_err(err: String) -> ApiError {
    if err == "财经资讯采集已关闭" {
        ApiError::new(StatusCode::CONFLICT, err)
    } else {
        ApiError::new(StatusCode::BAD_GATEWAY, err)
    }
}

async fn admin_news_articles(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<NewsQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .list_news(
                0,
                q.source_id.unwrap_or(0),
                "",
                false,
                q.limit.unwrap_or(50).clamp(1, 100),
                0,
            )
            .await
            .map_err(db_err)?,
    ))
}

#[derive(Deserialize)]
struct AdminKolQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    platform: Option<String>,
    category_id: Option<i64>,
    q: Option<String>,
    status: Option<i64>,
}

async fn list_admin_kols(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AdminKolQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    if q.status.is_some_and(|status| status != 0 && status != 1) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "status 需为 0 或 1"));
    }
    let platform = q.platform.unwrap_or_default();
    let query = q.q.unwrap_or_default();
    if platform.len() > 32 || query.chars().count() > 80 {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "筛选条件过长"));
    }
    let page = state
        .db
        .admin_kol_page(
            &platform,
            q.category_id.unwrap_or(0),
            query.trim(),
            q.status,
            q.limit.unwrap_or(50).clamp(1, 200),
            q.offset.unwrap_or(0).max(0),
        )
        .await
        .map_err(db_err)?;
    Ok(Json(page))
}

async fn admin_kols(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_admin(&state, &headers).await?;
    Ok(Json(
        state
            .db
            .catalog(user.id, true, "", 0)
            .await
            .map_err(db_err)?,
    ))
}

async fn add_kol(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KolIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let id = state
        .db
        .add_kol(
            &body.platform,
            &body.name,
            &body.external_id,
            body.category_id,
            body.priority,
            body.secondary,
            body.original_only,
        )
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct KolBatchIn {
    lines: String,
    category_id: Option<i64>,
}

async fn batch_add_kols(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KolBatchIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = state
        .db
        .batch_add_kols(&body.lines, body.category_id)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "batch_add_kols",
            "",
            &format!("ok={}/{}", saved["ok"], saved["total"]),
        )
        .await;
    Ok(Json(saved))
}

#[derive(Deserialize)]
struct KolBatchAction {
    ids: Vec<i64>,
    action: String,
    value: Option<Value>,
}

async fn batch_kols(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KolBatchAction>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let count = state
        .db
        .batch_kols(&body.ids, &body.action, body.value.as_ref())
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            &format!("batch_{}", body.action),
            &count.to_string(),
            "",
        )
        .await;
    Ok(Json(json!({"ok": true, "count": count})))
}

async fn delete_kol(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    state.db.delete_kol(kol_id).await.map_err(catalog_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn update_kol(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let name = optional_text(&body, "name")?;
    let external_id = optional_text(&body, "external_id")?;
    let enabled = optional_bool(&body, "enabled")?;
    let priority = optional_bool(&body, "priority")?;
    let secondary = optional_bool(&body, "secondary")?;
    let is_private = optional_bool(&body, "is_private")?;
    let original_only = optional_bool(&body, "original_only")?;
    let category_id = if body.get("category_id").is_none() {
        None
    } else if body["category_id"].is_null() {
        Some(None)
    } else {
        Some(Some(body["category_id"].as_i64().ok_or_else(|| {
            ApiError::new(StatusCode::BAD_REQUEST, "分类不存在")
        })?))
    };
    let recommend_weight = if body.get("recommend_weight").is_none() {
        None
    } else {
        Some(
            body["recommend_weight"]
                .as_i64()
                .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "推荐权重需为数字"))?
                .max(0),
        )
    };
    let users = if body.get("visible_users").is_none() {
        None
    } else {
        let list = body["visible_users"]
            .as_array()
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "白名单需为用户名列表"))?;
        let mut names = Vec::new();
        for item in list {
            let Some(name) = item.as_str() else {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "白名单需为用户名列表",
                ));
            };
            names.push(name.to_string());
        }
        Some(names)
    };
    let patch = KolPatch {
        name: name.as_deref(),
        external_id: external_id.as_deref(),
        enabled,
        category_id,
        priority,
        secondary,
        is_private,
        original_only,
        recommend_weight,
        visible_users: users.as_deref(),
    };
    state
        .db
        .patch_kol(kol_id, patch)
        .await
        .map_err(catalog_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "update_kol", &kol_id.to_string(), "")
        .await;
    state
        .db
        .kol_for(admin.id, true, kol_id)
        .await
        .map_err(db_err)?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "大V不存在"))
}

fn optional_text(body: &Value, key: &str) -> Result<Option<String>, ApiError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(ApiError::new(StatusCode::BAD_REQUEST, "字段格式不正确")),
    }
}

fn optional_bool(body: &Value, key: &str) -> Result<Option<bool>, ApiError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(ApiError::new(StatusCode::BAD_REQUEST, "字段格式不正确")),
    }
}

async fn kol_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .kol_for(user.id, user.is_admin, kol_id)
        .await
        .map_err(db_err)?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "大V不存在"))
}

async fn kol_posts(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
    Query(q): Query<FlagQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .kol_posts(user.id, user.is_admin, kol_id, clamp_limit(q.limit, 50))
        .await
        .map_err(db_err)?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "大V不存在"))
}

async fn kol_holdings(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if state
        .db
        .kol_for(user.id, user.is_admin, kol_id)
        .await
        .map_err(db_err)?
        .is_none()
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "大V不存在"));
    }
    let snap = state
        .db
        .cube_snapshot(kol_id, "holdings")
        .await
        .map_err(db_err)?;
    Ok(Json(combination::holdings_response(snap)))
}

async fn kol_nav(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(kol_id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if state
        .db
        .kol_for(user.id, user.is_admin, kol_id)
        .await
        .map_err(db_err)?
        .is_none()
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "大V不存在"));
    }
    let snap = state
        .db
        .cube_snapshot(kol_id, "nav")
        .await
        .map_err(db_err)?;
    Ok(Json(combination::nav_response(snap)))
}

async fn login(
    State(state): State<AppState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Json(body): Json<LoginIn>,
) -> Result<Json<Value>, ApiError> {
    check_login_limit(&state, &ip)?;
    require_turnstile(&state, &body.turnstile, "login", &ip).await?;
    let username = body.username.trim();
    let user = state.db.user_by_username(username).await.map_err(db_err)?;
    let ok = match &user {
        Some(user) if !user.password_hash.is_empty() => {
            auth::verify_password(&body.password, &user.password_hash)
        }
        _ => {
            auth::verify_password(&body.password, dummy_password_hash());
            false
        }
    };
    if !ok || body.password.is_empty() || body.password.len() > auth::PASSWORD_MAX {
        record_login_failure(&state, &ip);
        let status = if body.password.len() > auth::PASSWORD_MAX {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::UNAUTHORIZED
        };
        let detail = if body.password.len() > auth::PASSWORD_MAX {
            format!("密码长度需在 1-{} 位之间", auth::PASSWORD_MAX)
        } else {
            "用户名或密码错误".into()
        };
        return Err(ApiError::new(status, detail));
    }
    let user = user.expect("checked");
    clear_login_failures(&state, &ip);
    state.db.touch_login(user.id).await.map_err(db_err)?;
    Ok(Json(session_payload(&state, &user).await?))
}

async fn register(
    State(state): State<AppState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Json(body): Json<RegisterIn>,
) -> Result<Json<Value>, ApiError> {
    if !state.allow_register {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "暂未开放注册"));
    }
    check_login_limit(&state, &ip)?;
    require_turnstile(&state, &body.turnstile, "register", &ip).await?;
    let username = match auth::validate_username(&body.username) {
        Ok(name) => name,
        Err(msg) => {
            record_login_failure(&state, &ip);
            return Err(ApiError::new(StatusCode::BAD_REQUEST, msg));
        }
    };
    if body.password.len() < auth::PASSWORD_MIN {
        record_login_failure(&state, &ip);
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("密码至少{}位", auth::PASSWORD_MIN),
        ));
    }
    if body.password.len() > auth::PASSWORD_MAX {
        record_login_failure(&state, &ip);
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("密码最长{}位", auth::PASSWORD_MAX),
        ));
    }
    if body.code.trim().is_empty() {
        record_login_failure(&state, &ip);
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "注册需要邀请码，请向管理员索取",
        ));
    }
    let hash = auth::hash_password(&body.password, None);
    let id = match state.db.register(&body.code, &username, &hash).await {
        Ok(id) => id,
        Err(RegisterError::Rejected(msg)) => {
            record_login_failure(&state, &ip);
            return Err(ApiError::new(StatusCode::BAD_REQUEST, msg));
        }
        Err(RegisterError::Db(err)) => return Err(db_err(err)),
    };
    let user = state
        .db
        .user_by_id(id)
        .await
        .map_err(db_err)?
        .expect("user");
    Ok(Json(session_payload(&state, &user).await?))
}

async fn wechat_login(
    State(state): State<AppState>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    Json(body): Json<WechatIn>,
) -> Result<Json<Value>, ApiError> {
    let app_id = std::env::var("WECHAT_APP_ID").unwrap_or_default();
    let secret = std::env::var("WECHAT_APP_SECRET").unwrap_or_default();
    if app_id.trim().is_empty() || secret.trim().is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "未配置微信小程序 app_id/app_secret",
        ));
    }
    check_login_limit(&state, &ip)?;
    let openid = match wechat::exchange(&body.code, &app_id, &secret).await {
        Ok(openid) => openid,
        Err(err) => {
            record_login_failure(&state, &ip);
            return Err(wechat_err(err));
        }
    };
    let id =
        match wechat::account(&state.db, state.allow_register, &openid, &body.invite_code).await {
            Ok(id) => id,
            Err(err) => {
                record_login_failure(&state, &ip);
                return Err(wechat_err(err));
            }
        };
    clear_login_failures(&state, &ip);
    state.db.touch_login(id).await.map_err(db_err)?;
    let user = state
        .db
        .user_by_id(id)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "微信登录失败"))?;
    Ok(Json(session_payload(&state, &user).await?))
}

fn wechat_err(err: wechat::WechatFail) -> ApiError {
    match err {
        wechat::WechatFail::Config => ApiError::new(
            StatusCode::BAD_REQUEST,
            "未配置微信小程序 app_id/app_secret",
        ),
        wechat::WechatFail::Closed => ApiError::new(StatusCode::FORBIDDEN, "暂未开放注册"),
        wechat::WechatFail::NeedInvite => {
            ApiError::new(StatusCode::BAD_REQUEST, "注册需要邀请码，请向管理员索取")
        }
        wechat::WechatFail::Bad(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
        wechat::WechatFail::Msg(msg) => ApiError::new(StatusCode::BAD_REQUEST, msg),
    }
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state.db.bump_token_version(user.id).await.map_err(db_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state.db.touch_login(user.id).await.map_err(db_err)?;
    let mut profile = enrich_user(&state.db, &user).await.map_err(db_err)?;
    let news_visible = state
        .db
        .setting("news_visible")
        .await
        .map_err(db_err)?
        .map(|value| value != "0")
        .unwrap_or(true);
    profile["news_visible"] = json!(news_visible);
    profile["subscription_count"] =
        json!(state.db.subscription_count(user.id).await.map_err(db_err)?);
    profile["push_guide"] = json!({
        "telegram_bot_username": "",
        "feishu_bot_name": "",
    });
    let bot = state.db.feishu_bot(user.id).await.map_err(db_err)?;
    profile["feishu_personal"] = json!({
        "available": feishu_personal::enabled(),
        "status": bot.as_ref().map(|item| item.status.as_str()).unwrap_or(""),
        "app_id_masked": bot.as_ref().map(|item| feishu_personal::mask_app_id(&item.app_id)).unwrap_or_default(),
    });
    let (public_key, count) = crate::webpush::profile(&state.db, user.id)
        .await
        .map_err(|err| {
            tracing::warn!("浏览器推送密钥不可用: {err}");
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "浏览器推送密钥不可用")
        })?;
    profile["vapid_public_key"] = json!(public_key);
    profile["webpush_count"] = json!(count);
    profile["webpush_bound"] = json!(count > 0);
    profile["android_device_count"] = json!(0);
    Ok(Json(profile))
}

#[derive(Deserialize)]
struct LlmForm {
    llm_api_base: Option<String>,
    llm_api_key: Option<String>,
    llm_model: Option<String>,
    llm_api_format: Option<String>,
}

async fn llm_models(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LlmForm>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let cfg = llm_config(&user, &body)?;
    let key = cfg.key.clone();
    let exchange = tokio::task::spawn_blocking(move || {
        llm::prepare(&cfg, "models", "GET", None, llm::resolve_host)
    })
    .await
    .map_err(|_| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "无法获取模型列表，请检查地址和 Key",
        )
    })?
    .map_err(llm_err)?;
    let reply = llm::send(&exchange, &key).await.map_err(llm_err)?;
    let models = llm::models_from(&reply).map_err(llm_err)?;
    Ok(Json(json!({"models": models})))
}

async fn llm_test(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LlmForm>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let started = std::time::Instant::now();
    let cfg = llm_config(&user, &body)?;
    let key = cfg.key.clone();
    let payload = llm::probe_body(&cfg);
    let suffix = llm::probe_suffix(&cfg);
    let prepared = tokio::task::spawn_blocking(move || {
        llm::prepare(&cfg, suffix, "POST", Some(payload), llm::resolve_host)
    })
    .await
    .map_err(|_| llm::LlmError::Failed);
    let reply = match prepared {
        Ok(Ok(exchange)) => llm::send(&exchange, &key).await,
        Ok(Err(err @ llm::LlmError::Bad(_))) => return Err(llm_err(err)),
        _ => Err(llm::LlmError::Failed),
    };
    let latency = started.elapsed().as_millis();
    let result = match reply {
        Ok(reply) => llm::probe_result(&reply, &llm_config(&user, &body)?, latency),
        Err(llm::LlmError::Bad(message)) => {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, message))
        }
        Err(llm::LlmError::Failed) => {
            json!({"ok": false, "latency_ms": latency, "error": "无响应或地址/Key/模型不正确"})
        }
    };
    Ok(Json(result))
}

fn llm_config(user: &User, body: &LlmForm) -> Result<llm::Config, ApiError> {
    llm::runtime(
        body.llm_api_base.as_deref(),
        body.llm_api_key.as_deref(),
        &user.llm_api_base,
        &user.llm_api_key,
        body.llm_model.as_deref(),
        &user.llm_model,
        body.llm_api_format.as_deref(),
        &user.llm_api_format,
        user.is_admin,
    )
    .map_err(llm_err)
}

fn llm_err(err: llm::LlmError) -> ApiError {
    match err {
        llm::LlmError::Bad(message) => ApiError::new(StatusCode::BAD_REQUEST, message),
        llm::LlmError::Failed => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "无法获取模型列表，请检查地址和 Key",
        ),
    }
}

async fn feishu_personal_register(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        feishu_personal::begin(&state.db, user.id, now_secs() as i64)
            .await
            .map_err(personal_err)?,
    ))
}

async fn feishu_personal_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(session_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        feishu_personal::status(&state.db, user.id, &session_id, now_secs() as i64)
            .await
            .map_err(personal_err)?,
    ))
}

async fn feishu_personal_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(session_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    Ok(Json(
        feishu_personal::refresh_code(&state.db, user.id, &session_id, now_secs() as i64)
            .await
            .map_err(personal_err)?,
    ))
}

async fn feishu_personal_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(session_id): UrlPath<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    feishu_personal::cancel(&state.db, user.id, &session_id)
        .await
        .map_err(personal_err)?;
    Ok(Json(json!({"ok": true})))
}

async fn feishu_personal_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    feishu_personal::disable(&state.db, user.id)
        .await
        .map_err(personal_err)?;
    Ok(Json(json!({"ok": true})))
}

fn personal_err(err: feishu_personal::PersonalError) -> ApiError {
    match err {
        feishu_personal::PersonalError::Disabled => ApiError::new(
            StatusCode::BAD_REQUEST,
            "个人机器人功能未启用（服务端未配置 FEISHU_CREDENTIAL_KEY）",
        ),
        feishu_personal::PersonalError::Missing => {
            ApiError::new(StatusCode::NOT_FOUND, "注册会话不存在")
        }
        feishu_personal::PersonalError::Bad(message) => {
            ApiError::new(StatusCode::BAD_REQUEST, message)
        }
        feishu_personal::PersonalError::Upstream(message) => {
            ApiError::new(StatusCode::BAD_GATEWAY, message)
        }
        feishu_personal::PersonalError::Db(err) => {
            tracing::warn!("飞书个人机器人数据库错误: {err}");
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "数据库错误")
        }
    }
}

async fn issue_bind_code(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let (code, expires) = state
        .db
        .issue_bind_code(user.id, now_secs() as i64)
        .await
        .map_err(catalog_err)?;
    Ok(Json(json!({"code": code, "expires_in_seconds": expires})))
}

async fn update_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MeUpdate>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    apply_profile(&state, &user, &body).await?;
    let fresh = state
        .db
        .user_by_id(user.id)
        .await
        .map_err(db_err)?
        .expect("user");
    Ok(Json(enrich_user(&state.db, &fresh).await.map_err(db_err)?))
}

async fn apply_profile(state: &AppState, user: &User, body: &MeUpdate) -> Result<(), ApiError> {
    if let Some(value) = &body.telegram_chat_id {
        state
            .db
            .set_user_text(user.id, "telegram_chat_id", value.trim())
            .await
            .map_err(db_err)?;
    }
    if let Some(value) = &body.telegram_bot_token {
        let value = value.trim();
        if !masked_secret(value) {
            if value.is_empty() {
                state
                    .db
                    .set_user_text(user.id, "telegram_bot_token", "")
                    .await
                    .map_err(db_err)?;
                state
                    .db
                    .set_user_text(user.id, "telegram_chat_id", "")
                    .await
                    .map_err(db_err)?;
            } else {
                if state
                    .db
                    .other_user_has("telegram_bot_token", value, user.id)
                    .await
                    .map_err(db_err)?
                {
                    return Err(ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "该机器人 token 已被其他账号使用",
                    ));
                }
                let (_username, chat_id) =
                    crate::push::resolve_telegram_bot(value)
                        .await
                        .map_err(|err| {
                            ApiError::new(
                                StatusCode::BAD_REQUEST,
                                format!("自建机器人绑定失败：{err}"),
                            )
                        })?;
                if state
                    .db
                    .other_user_has("telegram_chat_id", &chat_id, user.id)
                    .await
                    .map_err(db_err)?
                {
                    return Err(ApiError::new(
                        StatusCode::BAD_REQUEST,
                        "该 Telegram 已绑定其他账号",
                    ));
                }
                state
                    .db
                    .set_user_text(user.id, "telegram_bot_token", value)
                    .await
                    .map_err(db_err)?;
                state
                    .db
                    .set_user_text(user.id, "telegram_chat_id", &chat_id)
                    .await
                    .map_err(db_err)?;
            }
        }
    }
    for (field, column) in [
        (&body.feishu_open_id, "feishu_open_id"),
        (&body.feishu_chat_id, "feishu_chat_id"),
    ] {
        if let Some(value) = field {
            state
                .db
                .set_user_text(user.id, column, value.trim())
                .await
                .map_err(db_err)?;
        }
    }
    if let Some(value) = &body.wecom_webhook {
        let value = value.trim();
        if !masked_secret(value) {
            if !value.is_empty()
                && !value.starts_with("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=")
            {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "企业微信 webhook 地址无效，应为 https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=... 格式",
                ));
            }
            state
                .db
                .set_user_text(user.id, "wecom_webhook", value)
                .await
                .map_err(db_err)?;
        }
    }
    if let Some(value) = &body.bark_key {
        let value = value.trim();
        if !masked_secret(value) {
            if !value.is_empty() && !valid_bark_key(value) {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "Bark key 无效：应为手机 Bark App 里的推送 key（形如 AaBbCcDdEeFf...）",
                ));
            }
            state
                .db
                .set_user_text(user.id, "bark_key", value)
                .await
                .map_err(db_err)?;
        }
    }
    if let Some(value) = &body.push_channels {
        let channels: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect();
        let allowed = ["telegram", "feishu", "wecom", "bark", "webpush"];
        let invalid: Vec<&str> = channels
            .iter()
            .copied()
            .filter(|item| !allowed.contains(item))
            .collect();
        if !invalid.is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("无效的推送渠道: {}", invalid.join(", ")),
            ));
        }
        state
            .db
            .set_user_text(user.id, "push_channels", &channels.join(","))
            .await
            .map_err(db_err)?;
    }
    for (field, column, label) in [
        (&body.dnd_start, "dnd_start", "开始"),
        (&body.dnd_end, "dnd_end", "结束"),
    ] {
        if let Some(value) = field {
            let value = value.trim();
            if !value.is_empty() && !valid_clock(value) {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("免打扰{label}时间需为 HH:MM 格式（00:00-23:59）"),
                ));
            }
            state
                .db
                .set_user_text(user.id, column, value)
                .await
                .map_err(db_err)?;
        }
    }
    if let Some(keywords) = &body.keywords {
        let keywords: Vec<&str> = keywords
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .collect();
        if keywords.len() > 20 {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "关键词最多 20 个"));
        }
        if let Some(too_long) = keywords.iter().find(|item| item.chars().count() > 50) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("单个关键词最长 50 字：{too_long}"),
            ));
        }
        let encoded = serde_json::to_string(&keywords).unwrap_or_else(|_| "[]".into());
        state
            .db
            .set_user_text(user.id, "keywords", &encoded)
            .await
            .map_err(db_err)?;
    }
    if let Some(size) = &body.news_font_size {
        if !matches!(size.as_str(), "" | "small" | "large") {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "字号只支持空(标准)/small/large",
            ));
        }
        state
            .db
            .set_user_text(user.id, "news_font_size", size)
            .await
            .map_err(db_err)?;
    }
    if let Some(value) = &body.llm_api_base {
        let value = value.trim();
        if !(value.is_empty() || value.starts_with("https://") || value.starts_with("http://")) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "LLM 地址须为 http(s) URL",
            ));
        }
        state
            .db
            .set_user_text(user.id, "llm_api_base", value)
            .await
            .map_err(db_err)?;
    }
    if let Some(value) = &body.llm_api_key {
        let value = value.trim();
        if !masked_secret(value) {
            state
                .db
                .set_user_text(user.id, "llm_api_key", value)
                .await
                .map_err(db_err)?;
        }
    }
    if let Some(value) = &body.llm_model {
        state
            .db
            .set_user_text(user.id, "llm_model", value.trim())
            .await
            .map_err(db_err)?;
    }
    if let Some(value) = &body.llm_api_format {
        let format = llm::normalize_format(value);
        state
            .db
            .set_user_text(user.id, "llm_api_format", &format)
            .await
            .map_err(db_err)?;
    }
    for (field, column) in [
        (body.notify_enabled, "notify_enabled"),
        (body.daily_report_enabled, "daily_report"),
        (body.translate_twitter, "translate_twitter"),
        (body.dnd_allow_favorite, "dnd_allow_favorite"),
    ] {
        if let Some(value) = field {
            state
                .db
                .set_user_flag(user.id, column, value)
                .await
                .map_err(db_err)?;
        }
    }
    for (field, column, since) in [
        (
            body.keywords_match_reports,
            "keywords_match_reports",
            "keywords_match_reports_since",
        ),
        (
            body.keywords_match_news,
            "keywords_match_news",
            "keywords_match_news_since",
        ),
    ] {
        if let Some(value) = field {
            state
                .db
                .set_user_flag(user.id, column, value)
                .await
                .map_err(db_err)?;
            if value {
                state
                    .db
                    .stamp_keyword_since(user.id, since)
                    .await
                    .map_err(db_err)?;
            }
        }
    }
    if let Some(ids) = &body.news_source_ids {
        state
            .db
            .set_user_news_sources(user.id, ids)
            .await
            .map_err(catalog_err)?;
    }
    Ok(())
}

async fn subscribe_webpush(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WebPushIn>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    let endpoint = body.endpoint.trim();
    let p256dh = body.keys.p256dh.trim();
    let auth = body.keys.auth.trim();
    if !crate::webpush::endpoint_ok(endpoint) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "推送端点无效"));
    }
    if !crate::webpush::keys_ok(p256dh, auth) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "推送密钥无效"));
    }
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let ua: String = ua.chars().take(200).collect();
    state
        .db
        .upsert_webpush(user.id, endpoint, p256dh, auth, &ua)
        .await
        .map_err(db_err)?;
    let count = state.db.webpush_count(user.id).await.map_err(db_err)?;
    Ok(Json(
        json!({"ok": true, "webpush_bound": true, "webpush_count": count}),
    ))
}

async fn unsubscribe_webpush(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    state
        .db
        .delete_webpush_user(user.id)
        .await
        .map_err(db_err)?;
    Ok(Json(
        json!({"ok": true, "webpush_bound": false, "webpush_count": 0}),
    ))
}

async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasswordChange>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&state, &headers).await?;
    if body.new_password.len() < auth::PASSWORD_MIN {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "新密码至少10位"));
    }
    if body.new_password.len() > auth::PASSWORD_MAX {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "新密码最长128位"));
    }
    if !user.password_hash.is_empty()
        && !auth::verify_password(&body.old_password, &user.password_hash)
    {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "原密码错误"));
    }
    let hash = auth::hash_password(&body.new_password, None);
    state
        .db
        .set_password(user.id, &hash)
        .await
        .map_err(db_err)?;
    let fresh = state
        .db
        .user_by_id(user.id)
        .await
        .map_err(db_err)?
        .expect("user");
    Ok(Json(json!({
        "ok": true,
        "token": auth::create_token(fresh.id, &fresh.username, &state.secret, fresh.token_version, now_secs()),
    })))
}

fn valid_clock(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.bytes().all(|b| b.is_ascii_digit())
        && minute.bytes().all(|b| b.is_ascii_digit())
        && hour.parse::<u32>().ok().is_some_and(|h| h <= 23)
        && minute.parse::<u32>().ok().is_some_and(|m| m <= 59)
}

fn valid_bark_key(key: &str) -> bool {
    (10..=100).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn session_payload(state: &AppState, user: &User) -> Result<Value, ApiError> {
    let now = now_secs();
    Ok(json!({
        "token": auth::create_token(user.id, &user.username, &state.secret, user.token_version, now),
        "user": enrich_user(&state.db, user).await.map_err(db_err)?,
    }))
}

async fn enrich_user(db: &db::Db, user: &User) -> Result<Value, sqlx::Error> {
    let mut profile = public_user(user);
    let sources = db.plaza_sources().await?;
    let visible = sources
        .iter()
        .filter(|row| row["visible"] == true)
        .filter_map(|row| row["platform"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let subscribed = db.subscribed_platforms(user.id).await?;
    let timeline = visible
        .iter()
        .filter(|platform| subscribed.iter().any(|item| item == *platform))
        .cloned()
        .collect::<Vec<_>>();
    profile["plaza_platforms"] = json!(visible);
    profile["timeline_platforms"] = json!(timeline);
    Ok(profile)
}

fn public_user(user: &User) -> Value {
    let keywords = serde_json::from_str::<Vec<String>>(&user.keywords).unwrap_or_default();
    json!({
        "id": user.id,
        "username": user.username,
        "is_admin": user.is_admin,
        "telegram_chat_id": user.telegram_chat_id,
        "custom_telegram_bot": !user.telegram_bot_token.is_empty(),
        "feishu_open_id": user.feishu_open_id,
        "feishu_chat_id": user.feishu_chat_id,
        "wecom_webhook": mask_secret(&user.wecom_webhook),
        "bark_key": mask_secret(&user.bark_key),
        "notify_enabled": user.notify_enabled,
        "daily_report_enabled": user.daily_report,
        "translate_twitter": user.translate_twitter,
        "push_channels": user.push_channels,
        "dnd_start": user.dnd_start,
        "dnd_end": user.dnd_end,
        "dnd_allow_favorite": user.dnd_allow_favorite,
        "keywords": keywords,
        "keywords_match_reports": user.keywords_match_reports,
        "keywords_match_news": user.keywords_match_news,
        "news_font_size": user.news_font_size,
        "llm_api_base": user.llm_api_base,
        "llm_api_key": mask_secret(&user.llm_api_key),
        "llm_model": user.llm_model,
        "llm_api_format": if user.llm_api_format.is_empty() { "chat".to_string() } else { user.llm_api_format.clone() },
        "llm_last_status": "",
        "created_at": user.created_at,
    })
}

fn mask_secret(value: &str) -> String {
    let value = value.trim();
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 8 {
        return "…………".into();
    }
    format!(
        "{}……{}",
        chars[..4].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

fn masked_secret(value: &str) -> bool {
    value.contains("……")
}

#[derive(Deserialize)]
struct FeishuConfigIn {
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    app_secret: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    scopes: Option<String>,
    #[serde(default)]
    interval_seconds: Option<i64>,
}

#[derive(Deserialize)]
struct FeishuUrlIn {
    url: String,
}

#[derive(Deserialize)]
struct FeishuSourcePatch {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    display_mode: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct FeishuOauthQuery {
    #[serde(default)]
    state: String,
    #[serde(default)]
    code: String,
}

async fn feishu_docs_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    feishu_admin::overview(&state.db)
        .await
        .map(Json)
        .map_err(feishu_admin_err)
}

async fn feishu_docs_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeishuConfigIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = feishu_admin::save_config(
        &state.db,
        feishu_admin::Save {
            app_id: body.app_id.as_deref(),
            app_secret: body.app_secret.as_deref(),
            redirect_uri: body.redirect_uri.as_deref(),
            scopes: body.scopes.as_deref(),
            interval_seconds: body.interval_seconds,
        },
    )
    .await
    .map_err(feishu_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "update_feishu_documents_config", "", "")
        .await;
    Ok(Json(saved))
}

async fn feishu_docs_oauth_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let (url, hash) = feishu_admin::begin_oauth(&state.db, admin.id)
        .await
        .map_err(feishu_admin_err)?;
    let mut response = Json(json!({"url": url})).into_response();
    let cookie = format!("feishu_oauth={hash}; Max-Age=300; HttpOnly; SameSite=Lax; Path=/api/admin/feishu-documents/oauth/callback");
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "无法开始飞书授权"))?,
    );
    let _ = state
        .db
        .add_admin_log(admin.id, "start_feishu_documents_oauth", "", "")
        .await;
    Ok(response)
}

async fn feishu_docs_oauth_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FeishuOauthQuery>,
) -> Response {
    let cookie = cookie_value(&headers, "feishu_oauth");
    let failed = Redirect::to("/admin/knowledge?tab=feishu&oauth=failed");
    let verifier = match feishu_admin::take_session(&state.db, &q.state, &cookie).await {
        Ok(verifier) => verifier,
        Err(_) => return failed.into_response(),
    };
    let cfg = match feishu_admin::overview(&state.db).await {
        Ok(value) => value,
        Err(_) => return failed.into_response(),
    };
    let app_id = cfg["config"]["app_id"].as_str().unwrap_or("");
    let redirect = cfg["config"]["redirect_uri"].as_str().unwrap_or("");
    let secret = std::env::var("FEISHU_DOCS_APP_SECRET").unwrap_or_default();
    let secret = if secret.is_empty() {
        match feishu_secret(&state.db).await {
            Ok(secret) => secret,
            Err(_) => return failed.into_response(),
        }
    } else {
        secret
    };
    let body = match feishu_admin::exchange_code(app_id, &secret, redirect, &q.code, &verifier) {
        Ok(body) => body,
        Err(_) => return failed.into_response(),
    };
    if feishu_admin::save_token(&state.db, &body).await.is_err() {
        return failed.into_response();
    }
    Redirect::to("/admin/knowledge?tab=feishu&oauth=success").into_response()
}

async fn feishu_secret(db: &db::Db) -> Result<String, ApiError> {
    let stored = db
        .setting("feishu_docs_app_secret")
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "读取飞书文档配置失败"))?
        .unwrap_or_default();
    let key = feishu_personal::credential_key()
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "未配置 FEISHU_CREDENTIAL_KEY"))?;
    feishu_personal::open_app_secret(&key, &stored)
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "App Secret 无法解密"))
}

async fn feishu_docs_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeishuUrlIn>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    feishu_admin::preview_url(&state.db, &body.url)
        .await
        .map(Json)
        .map_err(feishu_admin_err)
}

async fn feishu_docs_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeishuUrlIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let source = feishu_admin::add_source(&state.db, &body.url)
        .await
        .map_err(feishu_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "add_feishu_document_source",
            &source["id"].to_string(),
            "",
        )
        .await;
    Ok(Json(source))
}

async fn feishu_docs_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
    Json(body): Json<FeishuSourcePatch>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let source = feishu_admin::update_source(
        &state.db,
        id,
        body.enabled,
        body.display_mode.as_deref(),
        body.display_name.as_deref(),
    )
    .await
    .map_err(feishu_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "update_feishu_document_source",
            &id.to_string(),
            "",
        )
        .await;
    Ok(Json(source))
}

async fn feishu_docs_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    feishu_admin::remove_source(&state.db, id)
        .await
        .map_err(feishu_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "delete_feishu_document_source",
            &id.to_string(),
            "",
        )
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn feishu_docs_sync(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let queued = feishu_admin::sync_source(&state.db, id)
        .await
        .map_err(feishu_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "sync_feishu_document_source", &id.to_string(), "")
        .await;
    Ok(Json(queued))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .find_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            (key == name).then(|| value.to_string())
        })
        .unwrap_or_default()
}

fn feishu_admin_err(err: feishu_admin::Fail) -> ApiError {
    let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST);
    ApiError::new(status, err.detail)
}

async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<User, ApiError> {
    let user = require_user(state, headers).await?;
    if !user.is_admin {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "需要管理员权限"));
    }
    Ok(user)
}

async fn require_download_user(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<User, ApiError> {
    let header_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let token = header_token.or(query_token).unwrap_or("");
    user_from_token(state, token).await
}

async fn require_user(state: &AppState, headers: &HeaderMap) -> Result<User, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    user_from_token(state, token).await
}

async fn user_from_token(state: &AppState, token: &str) -> Result<User, ApiError> {
    let claims = auth::verify_token(token, &state.secret, now_secs())
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "未登录"))?;
    let user = state
        .db
        .user_by_id(claims.uid)
        .await
        .map_err(db_err)?
        .filter(|user| user.token_version == claims.ver && user.username == claims.name)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "未登录"))?;
    Ok(user)
}

async fn attach_ip(mut req: Request<Body>, next: Next) -> Response {
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip().to_string())
        .unwrap_or_else(|| "local".into());
    req.extensions_mut().insert(ClientIp(ip));
    next.run(req).await
}

fn dummy_password_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        auth::hash_password("dummy-password", Some("00112233445566778899aabbccddeeff"))
    })
}

fn check_login_limit(state: &AppState, ip: &str) -> Result<(), ApiError> {
    let now = now_secs();
    let map = state.fails.lock().expect("login limit");
    let count = map
        .get(ip)
        .map(|hits| {
            hits.iter()
                .filter(|t| now.saturating_sub(**t) < LOGIN_WINDOW_SECS)
                .count()
        })
        .unwrap_or(0);
    if count >= LOGIN_MAX_FAILURES {
        Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "尝试次数过多，请 5 分钟后再试",
        ))
    } else {
        Ok(())
    }
}

fn record_login_failure(state: &AppState, ip: &str) {
    let now = now_secs();
    let mut map = state.fails.lock().expect("login limit");
    let hits = map.entry(ip.to_string()).or_default();
    hits.retain(|t| now.saturating_sub(*t) < LOGIN_WINDOW_SECS);
    hits.push(now);
}

fn clear_login_failures(state: &AppState, ip: &str) {
    state.fails.lock().expect("login limit").remove(ip);
}

fn db_err(err: sqlx::Error) -> ApiError {
    tracing::error!("{err}");
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "服务器错误")
}

fn catalog_err(err: CatalogError) -> ApiError {
    match err {
        CatalogError::Missing(detail) => ApiError::new(StatusCode::NOT_FOUND, detail),
        CatalogError::Bad(detail) => ApiError::new(StatusCode::BAD_REQUEST, detail),
        CatalogError::Invalid(detail) => ApiError::new(StatusCode::BAD_REQUEST, detail),
        CatalogError::Limited(detail) => ApiError::new(StatusCode::TOO_MANY_REQUESTS, detail),
        CatalogError::Conflict(detail) => ApiError::new(StatusCode::CONFLICT, detail),
        CatalogError::Db(err) => db_err(err),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        "default-src 'self'; script-src 'self' 'unsafe-inline' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; connect-src 'self' https://challenges.cloudflare.com; frame-src 'self' blob: https://challenges.cloudflare.com; worker-src 'self'; manifest-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    if path == "/news"
        || path.starts_with("/news/")
        || path == "/api/news"
        || path.starts_with("/api/news/")
    {
        headers.insert(
            HeaderName::from_static("x-robots-tag"),
            "noindex, nofollow".parse().unwrap(),
        );
    }
    res
}

use axum::http::HeaderName;

async fn static_or_spa(State(state): State<AppState>, req: Request<Body>) -> Response {
    if req.method() != axum::http::Method::GET && req.method() != axum::http::Method::HEAD {
        return StatusCode::NOT_FOUND.into_response();
    }
    let rel = req.uri().path().trim_start_matches('/');
    if rel.split('/').any(|part| part == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some((path, fingerprinted)) = resolve_asset(&state.static_dir, rel) {
        return file_response(
            &path,
            fingerprinted,
            req.method() == axum::http::Method::HEAD,
        );
    }
    let first = rel.split('/').next().unwrap_or("");
    if rel.is_empty() || SPA.contains(&first) {
        let index = state.static_dir.join("index.html");
        if index.is_file() {
            return file_response(&index, false, req.method() == axum::http::Method::HEAD);
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

fn resolve_asset(root: &Path, rel: &str) -> Option<(PathBuf, bool)> {
    if rel.is_empty() {
        return None;
    }
    let direct = root.join(rel);
    if direct.is_file() && inside(root, &direct) {
        return Some((direct, false));
    }
    let logical = unhash(rel)?;
    let path = root.join(&logical);
    if !path.is_file() || !inside(root, &path) {
        return None;
    }
    let digest = hex::encode(Sha256::digest(std::fs::read(&path).ok()?));
    let hashed = rel.rsplit_once('.')?.0.rsplit_once('.')?.1;
    if !digest.starts_with(hashed) {
        return None;
    }
    Some((path, true))
}

fn unhash(rel: &str) -> Option<String> {
    let (rest, ext) = rel.rsplit_once('.')?;
    if ext != "js" && ext != "css" {
        return None;
    }
    let (stem, digest) = rest.rsplit_once('.')?;
    if digest.len() != 12 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{stem}.{ext}"))
}

fn inside(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

fn file_response(path: &Path, fingerprinted: bool, head: bool) -> Response {
    let name = path.to_string_lossy();
    let body = if head {
        Body::empty()
    } else {
        match std::fs::read(path) {
            Ok(bytes) => Body::from(bytes),
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    };
    let cache = if fingerprinted {
        "public, max-age=31536000, immutable"
    } else if should_revalidate(&name) {
        "no-cache"
    } else {
        "public, max-age=86400"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, content_type(&name))
        .header(header::CACHE_CONTROL, cache)
        .body(body)
        .unwrap()
}

fn should_revalidate(path: &str) -> bool {
    let path = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    path.ends_with(".html")
        || path.ends_with(".js")
        || path.ends_with(".css")
        || path.ends_with(".webmanifest")
        || path.ends_with(".json")
}

#[derive(Deserialize)]
struct ProxyPoolIn {
    name: String,
    kind: Option<String>,
    protocol: Option<String>,
    extract_url: Option<String>,
    expire_seconds: Option<i64>,
    refresh_interval_seconds: Option<i64>,
}

#[derive(Deserialize)]
struct ProxyImportIn {
    text: Option<String>,
    protocol: Option<String>,
    pool_id: Option<i64>,
}

async fn list_proxy_pools(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(proxy_admin::pools(&state.db).await.map_err(db_err)?))
}

async fn create_proxy_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProxyPoolIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let pool = proxy_admin::create_pool(
        &state.db,
        &body.name,
        body.kind.as_deref().unwrap_or("static"),
        body.protocol.as_deref().unwrap_or("http"),
        body.extract_url.as_deref().unwrap_or(""),
        body.expire_seconds.unwrap_or(0),
        body.refresh_interval_seconds.unwrap_or(0),
    )
    .await
    .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "create_proxy_pool",
            &pool["id"].to_string(),
            &body.name,
        )
        .await;
    Ok(Json(pool))
}

async fn delete_proxy_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    proxy_admin::delete_pool(&state.db, id)
        .await
        .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "delete_proxy_pool", &id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn import_proxy_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
    Json(body): Json<ProxyImportIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let result = proxy_admin::import_text(
        &state.db,
        id,
        body.text.as_deref().unwrap_or(""),
        body.protocol.as_deref(),
    )
    .await
    .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "import_proxies",
            &id.to_string(),
            &format!("imported={}", result["imported"]),
        )
        .await;
    Ok(Json(result))
}

async fn import_proxies_loose(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProxyImportIn>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let pool_id = body
        .pool_id
        .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "代理池不存在"))?;
    let result = proxy_admin::import_text(
        &state.db,
        pool_id,
        body.text.as_deref().unwrap_or(""),
        body.protocol.as_deref(),
    )
    .await
    .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "add_proxy",
            &pool_id.to_string(),
            &format!("imported={}", result["imported"]),
        )
        .await;
    Ok(Json(result))
}

async fn extract_proxy_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let url = proxy_admin::extract_target(&state.db, id)
        .await
        .map_err(proxy_admin_err)?;
    let fetched = tokio::task::spawn_blocking(move || proxy_admin::live_get(&url))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "提取失败"))?;
    let result = match fetched {
        Ok(body) => proxy_admin::store_extracted(&state.db, id, &body, now_secs() as i64)
            .await
            .map_err(proxy_admin_err)?,
        Err(err) => {
            proxy_admin::note_extract_error(&state.db, id, &err).await;
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("提取失败: {err}"),
            ));
        }
    };
    let _ = state
        .db
        .add_admin_log(
            admin.id,
            "extract_proxy_pool",
            &id.to_string(),
            &format!("imported={}", result["imported"]),
        )
        .await;
    Ok(Json(result))
}

async fn list_proxies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(proxy_admin::proxies(&state.db).await.map_err(db_err)?))
}

async fn delete_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    proxy_admin::delete_proxy(&state.db, id)
        .await
        .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "delete_proxy", &id.to_string(), "")
        .await;
    Ok(Json(json!({"ok": true})))
}

async fn test_proxy(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    let endpoint = proxy_admin::proxy_endpoint(&state.db, id)
        .await
        .map_err(proxy_admin_err)?;
    let probed = tokio::task::spawn_blocking(move || proxy_admin::live_probe(&endpoint))
        .await
        .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "测试失败"))?;
    let result = match probed {
        Ok(code) if code == 204 || (200..400).contains(&code) => {
            proxy_admin::finish_probe(&state.db, id, true, Some(code), "").await
        }
        Ok(code) => {
            proxy_admin::finish_probe(&state.db, id, false, Some(code), &format!("HTTP {code}"))
                .await
        }
        Err(err) if err.starts_with("status:") => {
            let code = err.trim_start_matches("status:").parse().unwrap_or(0);
            proxy_admin::finish_probe(&state.db, id, false, Some(code), &format!("HTTP {code}"))
                .await
        }
        Err(err) => proxy_admin::finish_probe(&state.db, id, false, None, &err).await,
    }
    .map_err(proxy_admin_err)?;
    Ok(Json(result))
}

async fn proxy_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    require_admin(&state, &headers).await?;
    Ok(Json(proxy_admin::routes(&state.db).await.map_err(db_err)?))
}

async fn save_proxy_routes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let admin = require_admin(&state, &headers).await?;
    let saved = proxy_admin::save_routes(&state.db, &body)
        .await
        .map_err(proxy_admin_err)?;
    let _ = state
        .db
        .add_admin_log(admin.id, "set_proxy_routes", "", "")
        .await;
    Ok(Json(saved))
}

fn proxy_admin_err(err: proxy_admin::AdminError) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_REQUEST),
        err.detail,
    )
}

fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "webmanifest" => "application/manifest+json",
        "json" => "application/json",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn login_me_and_shell() {
        let dir = std::env::temp_dir().join(format!("vpush-{}-{}", std::process::id(), now_secs()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("static")).unwrap();
        std::fs::write(dir.join("static/index.html"), b"<title>VPush</title>").unwrap();
        std::fs::write(dir.join("static/app.js"), b"console.log(1)").unwrap();
        let db = Db::open(&dir.join("vpush.db")).await.unwrap();
        db.ensure_admin(&auth::hash_password("password1234", None))
            .await
            .unwrap();
        let state = AppState {
            db,
            secret: "test-secret".into(),
            allow_register: true,
            static_dir: dir.join("static"),
            fails: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/timeline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let html = to_bytes(res.into_body(), 1024).await.unwrap();
        assert!(html.windows(5).any(|w| w == b"<titl") || html.starts_with(b"<title"));

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/app.aaaaaaaaaaaa.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let digest = hex::encode(Sha256::digest(b"console.log(1)"));
        let hashed = format!("/app.{}.js", &digest[..12]);
        let res = app
            .clone()
            .oneshot(Request::builder().uri(&hashed).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );

        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"Admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(login.into_body(), 8192).await.unwrap()).unwrap();
        let token = body["token"].as_str().unwrap();
        assert!(body["user"]["is_admin"].as_bool().unwrap());

        let me = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);

        let feed = app
            .oneshot(
                Request::builder()
                    .uri("/api/my/feed")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(feed.status(), StatusCode::OK);
        let feed_body = to_bytes(feed.into_body(), 64).await.unwrap();
        assert_eq!(&feed_body[..], b"[]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn settings_round_trip_and_password() {
        let dir =
            std::env::temp_dir().join(format!("vpush-set-{}-{}", std::process::id(), now_secs()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("static")).unwrap();
        let db = Db::open(&dir.join("vpush.db")).await.unwrap();
        db.ensure_admin(&auth::hash_password("password1234", None))
            .await
            .unwrap();
        let state = AppState {
            db,
            secret: "test-secret".into(),
            allow_register: false,
            static_dir: dir.join("static"),
            fails: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(login.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        let token = parsed["token"].as_str().unwrap().to_string();
        let saved = app.clone().oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"notify_enabled":false,"push_channels":"wecom,bark","keywords":["宏观"],"bark_key":"AbcdEfghij"}"#))
                .unwrap(),
        ).await.unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved_body = to_bytes(saved.into_body(), 4096).await.unwrap();
        let saved_json: Value = serde_json::from_slice(&saved_body).unwrap();
        assert_eq!(saved_json["notify_enabled"], false);
        assert_eq!(saved_json["push_channels"], "wecom,bark");
        assert_eq!(saved_json["keywords"][0], "宏观");
        assert!(saved_json["bark_key"].as_str().unwrap().contains('…'));
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/me")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"wecom_webhook":"https://evil.example/hook"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let changed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/me/password")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"old_password":"password1234","new_password":"newpassword1"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(changed.status(), StatusCode::OK);
        let stale = app
            .oneshot(
                Request::builder()
                    .uri("/api/me")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn admin_turnstile_saves_keys_and_hides_secret() {
        let dir = std::env::temp_dir().join(format!(
            "vpush-turnstile-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("static")).unwrap();
        let db = Db::open(&dir.join("vpush.db")).await.unwrap();
        db.ensure_admin(&auth::hash_password("password1234", None))
            .await
            .unwrap();
        sqlx::query("INSERT INTO users (username, password_hash, is_admin) VALUES ('plain', ?, 0)")
            .bind(auth::hash_password("password1234", None))
            .execute(db.pool())
            .await
            .unwrap();
        let state = AppState {
            db,
            secret: "test-secret".into(),
            allow_register: false,
            static_dir: dir.join("static"),
            fails: Arc::new(Mutex::new(HashMap::new())),
        };
        let app = router(state);
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let token =
            serde_json::from_slice::<Value>(&to_bytes(login.into_body(), 4096).await.unwrap())
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
        let user_login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"plain","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let user_token =
            serde_json::from_slice::<Value>(&to_bytes(user_login.into_body(), 4096).await.unwrap())
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/turnstile")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let missing = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/turnstile")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"enabled":true,"sitekey":"0xabc","secret":"sec"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
        let bad_host = app.clone().oneshot(Request::builder().method("PUT").uri("/api/admin/turnstile").header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, "application/json").body(Body::from(r#"{"enabled":true,"sitekey":"0xabc","secret":"sec","hostnames":"https://vpush.net"}"#)).unwrap()).await.unwrap();
        assert_eq!(bad_host.status(), StatusCode::BAD_REQUEST);
        let saved = app.clone().oneshot(Request::builder().method("PUT").uri("/api/admin/turnstile").header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, "application/json").body(Body::from(r#"{"enabled":true,"sitekey":"0xabc","secret":"sec","hostnames":"vpush.net"}"#)).unwrap()).await.unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let saved_json: Value =
            serde_json::from_slice(&to_bytes(saved.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(saved_json["active"], true);
        assert_eq!(saved_json["secret_set"], true);
        assert!(saved_json.get("secret").is_none());
        let public = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/turnstile")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let public_json: Value =
            serde_json::from_slice(&to_bytes(public.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(public_json["sitekey"], "0xabc");
        let blocked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
        let kept = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/turnstile")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"enabled":true,"sitekey":"0xabc","secret":"","hostnames":"vpush.net"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let kept_json: Value =
            serde_json::from_slice(&to_bytes(kept.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(kept_json["secret_set"], true);
        let off = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/turnstile")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"enabled":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(off.status(), StatusCode::OK);
        let hidden = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/turnstile")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let hidden_json: Value =
            serde_json::from_slice(&to_bytes(hidden.into_body(), 1024).await.unwrap()).unwrap();
        assert_eq!(hidden_json["sitekey"], "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn system_logs_filters_for_admins() {
        let dir = std::env::temp_dir().join(format!(
            "vpush-syslog-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("static")).unwrap();
        let db = Db::open(&dir.join("vpush.db")).await.unwrap();
        db.ensure_admin(&auth::hash_password("password1234", None))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin) VALUES ('plainlog', ?, 0)",
        )
        .bind(auth::hash_password("password1234", None))
        .execute(db.pool())
        .await
        .unwrap();
        let app = router(AppState {
            db,
            secret: "test-secret".into(),
            allow_register: false,
            static_dir: dir.join("static"),
            fails: Arc::new(Mutex::new(HashMap::new())),
        });
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let token =
            serde_json::from_slice::<Value>(&to_bytes(login.into_body(), 4096).await.unwrap())
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
        let user_login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"plainlog","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let user_token =
            serde_json::from_slice::<Value>(&to_bytes(user_login.into_body(), 4096).await.unwrap())
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
        let denied = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/admin/system-logs")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        syslogs::record("2026-08-11 10:00:03.000 ERROR app.d [t] syslog-marker failed");
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/admin/system-logs?level=ERROR&q=syslog-marker&limit=50")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(ok.into_body(), 8192).await.unwrap()).unwrap();
        assert!(body["lines"][0].as_str().unwrap().contains("syslog-marker"));
        let bogus = app
            .oneshot(
                Request::builder()
                    .uri("/api/admin/system-logs?level=BOGUS")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bogus.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn admin_sets_user_knowledge_access() {
        let dir =
            std::env::temp_dir().join(format!("vpush-imakb-{}-{}", std::process::id(), now_secs()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("static")).unwrap();
        let db = Db::open(&dir.join("vpush.db")).await.unwrap();
        db.ensure_admin(&auth::hash_password("password1234", None))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO users (username, password_hash, is_admin) VALUES ('readerkb', ?, 0)",
        )
        .bind(auth::hash_password("password1234", None))
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO ima_document_index (group_id, media_id, name) VALUES ('reports', 'm1', 'a.pdf')")
            .execute(db.pool())
            .await
            .unwrap();
        let reader = db.user_by_username("readerkb").await.unwrap().unwrap();
        let app = router(AppState {
            db,
            secret: "test-secret".into(),
            allow_register: false,
            static_dir: dir.join("static"),
            fails: Arc::new(Mutex::new(HashMap::new())),
        });
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"password1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let token =
            serde_json::from_slice::<Value>(&to_bytes(login.into_body(), 4096).await.unwrap())
                .unwrap()["token"]
                .as_str()
                .unwrap()
                .to_string();
        let saved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/admin/users/{}/ima-kb", reader.id))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"group_ids":["reports","feishu-open","missing"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::NOT_FOUND);
        let saved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/api/admin/users/{}/ima-kb", reader.id))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"group_ids":["reports","feishu-open"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(saved.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["ima_kb_groups"], json!(["reports"]));
        assert_eq!(body["ima_kb_subscribed"], json!(["reports"]));
        let admin_user = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/admin/users/1/ima-kb")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"group_ids":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_user.status(), StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
