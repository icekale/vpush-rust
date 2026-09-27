#!/usr/bin/env python3
"""Convert legacy Telegram Fernet secrets in an offline SQLite copy."""

import argparse
import base64
import os
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from cryptography.fernet import Fernet, InvalidToken
from cryptography.hazmat.primitives.ciphers.aead import AESGCM


class _ArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        self.exit(2, "error: invalid arguments\n")


class MigrationError(Exception):
    """Raised when the database cannot be safely converted."""


@dataclass(frozen=True)
class MigrationResult:
    converted: int
    unchanged: int
    dry_run: bool


def _key_from_base64(value: str) -> bytes:
    try:
        encoded = value.strip().encode("ascii")
        decoded = base64.b64decode(
            encoded + b"=" * (-len(encoded) % 4),
            altchars=b"-_",
            validate=True,
        )
    except (UnicodeEncodeError, ValueError, base64.binascii.Error) as exc:
        raise MigrationError("invalid credential key") from exc
    if len(decoded) != 32:
        raise MigrationError("invalid credential key")
    return decoded


def _encrypt(value: str, key: bytes) -> str:
    nonce = os.urandom(12)
    encrypted = AESGCM(key).encrypt(nonce, value.encode("utf-8"), None)
    return "enc2:" + base64.urlsafe_b64encode(nonce + encrypted).decode("ascii")


def _quick_check(connection: sqlite3.Connection) -> None:
    checks = connection.execute("PRAGMA quick_check").fetchall()
    if not checks or any(row[0] != "ok" for row in checks):
        raise MigrationError("database quick_check failed")


def convert_database(
    database: os.PathLike[str] | str,
    fernet_key: str | bytes,
    credential_key: str,
    *,
    dry_run: bool = False,
) -> MigrationResult:
    """Convert every enc1 Telegram token, without changing other token values."""
    try:
        fernet = Fernet(fernet_key)
    except (TypeError, ValueError) as exc:
        raise MigrationError("invalid Fernet key") from exc
    aes_key = _key_from_base64(credential_key)

    connection = sqlite3.connect(Path(database))
    try:
        _quick_check(connection)
        rows = connection.execute(
            "SELECT id, telegram_bot_token FROM users"
        ).fetchall()
        replacements = {}
        for rowid, stored in rows:
            if not isinstance(stored, str) or not stored.startswith("enc1:"):
                continue
            try:
                plaintext = fernet.decrypt(stored[5:].encode("ascii"))
                decoded = plaintext.decode("utf-8")
            except (InvalidToken, UnicodeDecodeError, UnicodeEncodeError, ValueError, TypeError) as exc:
                raise MigrationError("legacy secret preflight failed") from exc
            replacements[rowid] = _encrypt(decoded, aes_key)

        result = MigrationResult(
            converted=len(replacements),
            unchanged=len(rows) - len(replacements),
            dry_run=dry_run,
        )
        if dry_run or not replacements:
            return result

        try:
            connection.execute("BEGIN IMMEDIATE")
            for rowid, replacement in replacements.items():
                connection.execute(
                    "UPDATE users SET telegram_bot_token = ? WHERE id = ?",
                    (replacement, rowid),
                )
            _quick_check(connection)
            connection.commit()
        except (MigrationError, sqlite3.Error) as exc:
            connection.rollback()
            if isinstance(exc, MigrationError):
                raise
            raise MigrationError("database conversion failed") from exc
        return result
    except sqlite3.Error as exc:
        raise MigrationError("database read failed") from exc
    finally:
        connection.close()


def _read_key(environment_name: str, descriptor_name: str) -> str:
    descriptor = os.environ.get(descriptor_name, "").strip()
    if descriptor:
        try:
            fd = int(descriptor)
            with os.fdopen(os.dup(fd), "rb") as stream:
                value = stream.read().strip().decode("ascii")
        except (OSError, ValueError, UnicodeDecodeError):
            raise MigrationError("invalid protected key input") from None
        if value:
            return value
        raise MigrationError("empty protected key input")

    value = os.environ.get(environment_name, "").strip()
    if value:
        return value
    raise MigrationError("missing key input")


def build_parser() -> argparse.ArgumentParser:
    parser = _ArgumentParser(
        description="Convert Telegram enc1 secrets in an offline SQLite copy"
    )
    parser.add_argument("database", help="path to an offline database copy")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="preflight and count conversions without writing",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        fernet_key = _read_key("TELEGRAM_FERNET_KEY", "TELEGRAM_FERNET_KEY_FD")
        credential_key = _read_key(
            "FEISHU_CREDENTIAL_KEY", "FEISHU_CREDENTIAL_KEY_FD"
        )
        result = convert_database(
            args.database, fernet_key, credential_key, dry_run=args.dry_run
        )
    except (MigrationError, OSError, sqlite3.Error):
        print("status=failed")
        return 1

    print(
        "status=ok"
        f" dry_run={int(result.dry_run)}"
        f" converted={result.converted}"
        f" unchanged={result.unchanged}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
