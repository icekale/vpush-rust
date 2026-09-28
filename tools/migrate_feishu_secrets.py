#!/usr/bin/env python3
"""Convert legacy Feishu Fernet secrets on an offline SQLite copy.

Rust stores AES-GCM as URL-safe base64 of nonce + ciphertext, with no prefix.
The same 32-byte FEISHU_CREDENTIAL_KEY is the Fernet key and the AES key.
"""

import base64
import os
import sqlite3
import sys
from pathlib import Path

from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from migrate_telegram_secrets import MigrationError, _key_from_base64, _validate_paths

TARGETS = (
    ("feishu_personal_bots", "user_id", "app_secret_ciphertext"),
    ("feishu_registration_sessions", "session_id", "device_code_ciphertext"),
    ("feishu_registration_sessions", "session_id", "candidate_app_secret_ciphertext"),
)


def seal(value: str, key: bytes) -> str:
    nonce = os.urandom(12)
    encrypted = AESGCM(key).encrypt(nonce, value.encode("utf-8"), None)
    return base64.urlsafe_b64encode(nonce + encrypted).decode("ascii")


def open_sealed(stored: str, key: bytes) -> str:
    packed = base64.b64decode(stored.encode("ascii"), altchars=b"-_")
    return AESGCM(key).decrypt(packed[:12], packed[12:], None).decode("utf-8")


def legacy_plain(stored: str, fernet: Fernet) -> str:
    token = stored[5:] if stored.startswith("enc1:") else stored
    try:
        return fernet.decrypt(token.encode("ascii")).decode("utf-8")
    except (InvalidToken, UnicodeError, ValueError, TypeError) as exc:
        raise MigrationError("legacy feishu secret failed") from exc


def needs_conversion(stored: str) -> bool:
    return stored.startswith("gAAAA") or stored.startswith("enc1:")


def convert_database(source: Path, output: Path, fernet_key: str, aes_key: str) -> tuple[int, int]:
    key = _key_from_base64(aes_key)
    if fernet_key != aes_key:
        raise MigrationError("feishu fernet key and aes key must be the same value")
    fernet = Fernet(fernet_key.encode("ascii"))
    _validate_paths(source, output)
    if output.exists():
        raise MigrationError("output database must not exist")
    src = sqlite3.connect(f"file:{source}?mode=ro", uri=True)
    dst = sqlite3.connect(output)
    try:
        with dst:
            src.backup(dst)
        converted = unchanged = 0
        for table, key_column, column in TARGETS:
            rows = dst.execute(f"SELECT {key_column}, {column} FROM {table}").fetchall()
            for row_id, stored in rows:
                if not stored or not needs_conversion(stored):
                    unchanged += 1
                    continue
                plain = legacy_plain(stored, fernet)
                if not plain or any(ord(ch) < 32 for ch in plain):
                    raise MigrationError("legacy feishu secret is empty")
                sealed = seal(plain, key)
                if open_sealed(sealed, key) != plain:
                    raise MigrationError("converted feishu secret does not round-trip")
                dst.execute(
                    f"UPDATE {table} SET {column} = ? WHERE {key_column} = ?",
                    (sealed, row_id),
                )
                converted += 1
        dst.commit()
        check = dst.execute("PRAGMA quick_check").fetchone()
        if not check or check[0] != "ok":
            raise MigrationError("database quick_check failed")
        return converted, unchanged
    except Exception:
        dst.close()
        output.unlink(missing_ok=True)
        raise
    finally:
        src.close()
        if dst:
            try:
                dst.close()
            except sqlite3.ProgrammingError:
                pass


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("output")
    args = parser.parse_args(argv)
    key = os.environ.get("FEISHU_CREDENTIAL_KEY", "")
    try:
        converted, unchanged = convert_database(Path(args.source), Path(args.output), key, key)
    except MigrationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(f"converted={converted} unchanged={unchanged}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
