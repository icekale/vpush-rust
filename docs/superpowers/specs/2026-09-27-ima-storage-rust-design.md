# IMA Storage Rust Migration Design

**Status:** Approved for implementation

## Goal

Move the remaining Python IMA storage administration behavior into Rust so the cutover does not leave Python-only storage routes.

## Architecture

`src/main.rs` remains the authenticated HTTP adapter. New `src/ima_storage.rs` owns archive inspection, consistency reporting, orphan deduplication, refresh requests, backup dispatch, and alert settings. All filesystem operations are rooted at `IMA_ARCHIVE_ROOT`, reject missing/unusable roots, and do not follow symlinks outside the archive.

The Rust implementation preserves the Python API's important safety semantics: unavailable remote storage returns an explicit 503-equivalent error, consistency checks report without destructive cleanup, and backup/refresh never claim that an external operation completed when only a local request was recorded.

## Endpoint behavior

- `GET /api/admin/ima-storage/health`: archive capability and storage statistics.
- `GET /api/admin/ima-storage/consistency`: last consistency report, or a fresh report when no report exists.
- `POST /api/admin/ima-storage/consistency/run`: scan database references and archive files, persist and return the report.
- `POST /api/admin/ima-storage/dedup`: remove only unreferenced duplicate files, then return the report.
- `POST /api/admin/ima-storage/refresh`: record a local refresh request with timestamp and return current status.
- `POST /api/admin/ima-storage/backup`: use the existing configured backup path/mechanism; otherwise return unavailable rather than fake success.
- `GET/PUT /api/admin/ima-storage/alerts`: persist validated alert thresholds in SQLite and return masked/public settings.

## Safety rules

- Never delete a file referenced by `ima_document_index` or `feishu_document_sources`.
- Never follow symlinks while scanning or deleting.
- Never delete outside `IMA_ARCHIVE_ROOT`.
- Deduplication is content-hash based and only removes unreferenced duplicates.
- All mutating endpoints require the existing admin guard and emit an admin log.

## Verification

Unit tests cover archive health, reference/missing/orphan detection, symlink refusal, duplicate cleanup, alert validation, and unavailable backup/refresh behavior. The final gate is `cargo fmt --all --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings`, route inventory with zero Python-only routes, and `git diff --check`.
