# IMA Storage Rust Migration Implementation Plan

> **For agentic workers:** Execute this plan task-by-task. Keep each checkbox current and run the listed verification before moving on.

**Goal:** Implement the eight remaining IMA storage routes in Rust with safe archive behavior and no Python-only cutover routes.

**Architecture:** Keep HTTP/auth adapters in `src/main.rs`; add `src/ima_storage.rs` for archive operations and alert settings. Use SQLite settings for persisted reports, requests, and alert thresholds. Treat external backup/remote refresh as unavailable unless an explicit local capability exists; never return fake success.

**Tech Stack:** Rust, Axum 0.8, Tokio, SQLx SQLite, serde_json, SHA-256, standard filesystem APIs.

---

### Task 1: Add storage state and pure archive helpers

**Files:**
- Create: `src/ima_storage.rs`
- Modify: `src/main.rs:1-40`

- [x] **Step 1: Write failing unit tests**

Add tests in `src/ima_storage.rs` for:

```rust
#[test]
fn archive_paths_reject_symlinks_and_escape() {}

#[tokio::test]
async fn consistency_reports_missing_and_orphan_files() {}
```

The first test must create a root, a regular child, and a symlink to a file outside the root; the symlink must be rejected. The second must create one referenced file, omit one referenced file, and create one unreferenced file; the report must contain each category.

- [x] **Step 2: Run the focused tests and confirm failure**

Run:

```bash
cargo test ima_storage::tests::archive_paths_reject_symlinks_and_escape -- --nocapture
cargo test ima_storage::tests::consistency_reports_missing_and_orphan_files -- --nocapture
```

Expected: compile failure because `ima_storage` and its helpers do not exist.

- [x] **Step 3: Implement the module skeleton**

Add `mod ima_storage;` and implement:

```rust
pub const REPORT_KEY: &str = "ima_storage_consistency_report";
pub const REFRESH_KEY: &str = "ima_storage_refresh_requested_at";
pub const ALERTS_KEY: &str = "ima_storage_alert_settings";

pub fn archive_root() -> Result<PathBuf, StorageError>;
pub fn health(root: &Path) -> Result<Value, StorageError>;
pub fn safe_child(root: &Path, relative: &str) -> Result<PathBuf, StorageError>;
```

`safe_child` must reject absolute paths, `..`, symlink components, and paths that do not remain under `root`. `health` must report `available`, `readable`, `writable`, byte capacity, and regular-file count without following symlink directories.

- [x] **Step 4: Run the focused tests and commit**

Run the two focused tests; expected PASS. Then:

```bash
cargo fmt --all
cargo test ima_storage::tests
 git add src/main.rs src/ima_storage.rs
 git commit -m "新增 IMA 存储安全归档基础"
```

---

### Task 2: Implement consistency scan and safe deduplication

**Files:**
- Modify: `src/ima_storage.rs`
- Modify: `src/db.rs` only if a small query helper is required

- [x] **Step 1: Add failing tests**

Test that:

```rust
assert_eq!(report["missing"], json!(["local/a.pdf"]));
assert_eq!(report["orphan"], json!(["local/orphan.pdf"]));
assert_eq!(dedup["removed"], json!(1));
```

Create duplicate content under two unreferenced paths and assert that referenced files and symlinks remain untouched.

- [x] **Step 2: Implement reference collection**

Read referenced relative paths from `ima_document_index.pdf_path`, `ima_document_index.txt_path`, and `feishu_document_sources.timeline_path`/`asset_root`. Normalize to archive-relative paths and ignore empty values. Scan only regular files beneath the archive, excluding symlinks and known request/report metadata files.

- [x] **Step 3: Implement persistence and deduplication**

Implement:

```rust
pub async fn consistency(db: &Db, root: &Path) -> Result<Value, StorageError>;
pub async fn run_consistency(db: &Db, root: &Path) -> Result<Value, StorageError>;
pub async fn dedup(db: &Db, root: &Path) -> Result<Value, StorageError>;
```

Persist the report under `REPORT_KEY`. Dedup by SHA-256; retain the lexicographically first unreferenced file per digest and delete later unreferenced regular files only after rechecking the root boundary.

- [x] **Step 4: Verify and commit**

Run:

```bash
cargo test ima_storage::tests
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS. Commit:

```bash
git add src/ima_storage.rs src/db.rs
git commit -m "实现 IMA 归档一致性和安全去重"
```

---

### Task 3: Implement refresh, backup capability, and alerts

**Files:**
- Modify: `src/ima_storage.rs`
- Modify: `src/main.rs` route handlers

- [x] **Step 1: Add failing tests**

Cover:

```rust
assert_eq!(validate_alerts(&json!({"min_free_bytes": -1})).unwrap_err(), "阈值无效");
assert_eq!(refresh_status["status"], "requested");
assert_eq!(backup_status.unwrap_err().status, 503);
```

- [x] **Step 2: Implement state operations**

Implement:

```rust
pub async fn refresh(db: &Db, root: &Path) -> Result<Value, StorageError>;
pub async fn backup(db: &Db, root: &Path) -> Result<Value, StorageError>;
pub async fn alert_settings(db: &Db) -> Result<Value, StorageError>;
pub async fn save_alert_settings(db: &Db, body: &Value) -> Result<Value, StorageError>;
```

`refresh` writes a timestamped local request marker only if the archive is available and returns `status: requested`. `backup` returns a 503 `StorageUnavailable` error until an existing local backup target is configured; it must not claim `started`. Alert settings accept only bounded numeric thresholds and booleans, reject unknown/invalid values, and persist JSON through `Db::set_setting`.

- [x] **Step 3: Add admin routes**

Add to `src/main.rs`:

```rust
.route("/api/admin/ima-storage/health", get(ima_storage_health))
.route("/api/admin/ima-storage/consistency", get(ima_storage_consistency))
.route("/api/admin/ima-storage/consistency/run", post(ima_storage_consistency_run))
.route("/api/admin/ima-storage/dedup", post(ima_storage_dedup))
.route("/api/admin/ima-storage/refresh", post(ima_storage_refresh))
.route("/api/admin/ima-storage/backup", post(ima_storage_backup))
.route("/api/admin/ima-storage/alerts", get(ima_storage_alerts).put(save_ima_storage_alerts))
```

Every handler must call `require_admin`, map `StorageUnavailable` to 503, map invalid input to 400, and write an admin log for mutating actions.

- [x] **Step 4: Verify and commit**

Run:

```bash
cargo fmt --all
cargo test ima_storage::tests
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: PASS. Commit:

```bash
git add src/main.rs src/ima_storage.rs
 git commit -m "补齐 IMA 存储管理接口"
```

---

### Task 4: Contract and route acceptance

**Files:**
- Modify: `tools/route_inventory.py` only if parser coverage is incomplete
- Modify: `docs/superpowers/plans/2026-09-27-vpush-rust-cutover-plan.md`

- [x] **Step 1: Run route inventory**

```bash
python3 tools/route_inventory.py --python /tmp/vpush-production-api.py --rust src/main.rs --output /tmp/route-inventory-final.md
```

Expected: `Python-only: 0` for the route set currently present in the production source.

- [x] **Step 2: Run all quality gates**

```bash
cargo fmt --all --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

- [x] **Step 3: Audit changed behavior**

Confirm that no test or implementation writes SSH config, firewall rules, OCI security lists, production database paths, or deployment credentials. Confirm dedup tests prove referenced files survive.

- [x] **Step 4: Mark the plan and commit**

Mark completed checkboxes in this plan and the cutover plan. Commit:

```bash
git add docs/superpowers/plans/2026-09-27-ima-storage-rust-plan.md docs/superpowers/plans/2026-09-27-vpush-rust-cutover-plan.md
 git commit -m "完成 IMA 存储迁移验收"
```

Final report must include route counts, test count, any external backup limitation, and the exact remaining cutover prerequisite.
