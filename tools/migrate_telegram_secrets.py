#!/usr/bin/env python3
"""Convert legacy Telegram Fernet secrets from an offline SQLite copy.

The AES credential key must be the same canonical, padded URL-safe base64 key
configured by the Rust runtime. When an input has no enc2 value, this tool
cannot independently prove that key matches the runtime; verify the output on
staging before using it.
"""

import argparse
import base64
import os
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence
from urllib.parse import quote

from cryptography.exceptions import InvalidTag
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
        if not value or len(value) % 4 or any(
            char not in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_="
            for char in value
        ):
            raise ValueError
        decoded = base64.b64decode(value.encode("ascii"), altchars=b"-_", validate=True)
    except (UnicodeEncodeError, ValueError, base64.binascii.Error) as exc:
        raise MigrationError("invalid credential key") from exc
    if len(decoded) != 32 or base64.urlsafe_b64encode(decoded).decode("ascii") != value:
        raise MigrationError("invalid credential key")
    return decoded


def _telegram_token_ok(token: str) -> bool:
    try:
        token_bytes = token.encode("ascii")
    except UnicodeEncodeError:
        return False
    identifier, separator, secret = token_bytes.partition(b":")
    return (
        separator == b":"
        and 6 <= len(identifier) <= 16
        and identifier.isdigit()
        and 20 <= len(secret) <= 128
        and all(
            byte >= 65 and byte <= 90
            or byte >= 97 and byte <= 122
            or byte >= 48 and byte <= 57
            or byte in (95, 45)
            for byte in secret
        )
    )


def _encrypt(value: str, key: bytes) -> str:
    nonce = os.urandom(12)
    encrypted = AESGCM(key).encrypt(nonce, value.encode("utf-8"), None)
    return "enc2:" + base64.urlsafe_b64encode(nonce + encrypted).decode("ascii")


def _decrypt_enc2(stored: str, key: bytes) -> str:
    encoded = stored[5:]
    try:
        packed = base64.b64decode(encoded.encode("ascii"), altchars=b"-_", validate=True)
        if base64.urlsafe_b64encode(packed).decode("ascii") != encoded:
            raise ValueError
        if len(packed) < 28:
            raise ValueError
        plaintext = AESGCM(key).decrypt(packed[:12], packed[12:], None)
        value = plaintext.decode("utf-8")
    except (
        InvalidTag,
        UnicodeEncodeError,
        ValueError,
        base64.binascii.Error,
        UnicodeDecodeError,
    ) as exc:
        raise MigrationError("encrypted secret preflight failed") from exc
    if not _telegram_token_ok(value):
        raise MigrationError("encrypted secret token is invalid")
    return value


def _decrypt_enc1(stored: str, fernet: Fernet) -> str:
    try:
        plaintext = fernet.decrypt(stored[5:].encode("ascii"))
        value = plaintext.decode("utf-8")
    except (InvalidToken, UnicodeDecodeError, UnicodeEncodeError, ValueError, TypeError) as exc:
        raise MigrationError("legacy secret preflight failed") from exc
    if not _telegram_token_ok(value):
        raise MigrationError("legacy secret token is invalid")
    return value


def _readonly_connection(path: Path) -> sqlite3.Connection:
    uri = "file:" + quote(str(path), safe="/") + "?mode=ro"
    return sqlite3.connect(uri, uri=True)


def _quick_check(connection: sqlite3.Connection) -> None:
    checks = connection.execute("PRAGMA quick_check").fetchall()
    if not checks or any(row[0] != "ok" for row in checks):
        raise MigrationError("database quick_check failed")


def _output_paths(path: Path):
    return (
        path,
        Path(str(path) + "-wal"),
        Path(str(path) + "-shm"),
        Path(str(path) + "-journal"),
    )


def _exists(path: Path) -> bool:
    return os.path.lexists(path)


def _validate_paths(input_path: os.PathLike[str] | str, output_path: os.PathLike[str] | str | None):
    source = Path(input_path).expanduser()
    if not source.is_file():
        raise MigrationError("input database does not exist")
    source = source.resolve()
    if output_path is None:
        return source, None
    output = Path(output_path).expanduser().resolve()
    if output == source:
        raise MigrationError("output database must differ from input")
    if any(_exists(candidate) for candidate in _output_paths(output)):
        raise MigrationError("output database already exists")
    return source, output


def convert_database(
    input_path: os.PathLike[str] | str,
    output_path: os.PathLike[str] | str | None,
    fernet_key: str | bytes,
    credential_key: str,
    *,
    dry_run: bool = False,
) -> MigrationResult:
    """Convert enc1 rows from a read-only input into a new output copy."""
    source, output = _validate_paths(input_path, output_path)
    if not dry_run and output is None:
        raise MigrationError("output database is required unless using dry-run")
    try:
        fernet = Fernet(fernet_key)
    except (TypeError, ValueError) as exc:
        raise MigrationError("invalid Fernet key") from exc
    aes_key = _key_from_base64(credential_key)

    source_connection = None
    output_connection = None
    output_created = False
    succeeded = False
    try:
        source_connection = _readonly_connection(source)
        if dry_run:
            _quick_check(source_connection)
            rows = source_connection.execute(
                "SELECT id, telegram_bot_token FROM users"
            ).fetchall()
        else:
            fd = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            os.close(fd)
            output_created = True
            output_connection = sqlite3.connect(output)
            source_connection.backup(output_connection)
            output_connection.execute("PRAGMA journal_mode=DELETE")
            output_connection.execute("BEGIN IMMEDIATE")
            _quick_check(output_connection)
            rows = output_connection.execute(
                "SELECT id, telegram_bot_token FROM users"
            ).fetchall()

        replacements = {}
        for rowid, stored in rows:
            if not isinstance(stored, str):
                continue
            if stored.startswith("enc1:"):
                replacements[rowid] = _encrypt(_decrypt_enc1(stored, fernet), aes_key)
            elif stored.startswith("enc2:"):
                _decrypt_enc2(stored, aes_key)

        result = MigrationResult(
            converted=len(replacements),
            unchanged=len(rows) - len(replacements),
            dry_run=dry_run,
        )
        if dry_run:
            return result

        for rowid, replacement in replacements.items():
            output_connection.execute(
                "UPDATE users SET telegram_bot_token = ? WHERE id = ?",
                (replacement, rowid),
            )
        _quick_check(output_connection)
        output_connection.commit()
        succeeded = True
        return result
    except (OSError, sqlite3.Error) as exc:
        if output_connection is not None:
            output_connection.rollback()
        raise MigrationError("database conversion failed") from exc
    except MigrationError:
        if output_connection is not None:
            output_connection.rollback()
        raise
    finally:
        if output_connection is not None:
            output_connection.close()
        if source_connection is not None:
            source_connection.close()
        if output_created and output is not None and not succeeded:
            for candidate in _output_paths(output):
                try:
                    candidate.unlink()
                except FileNotFoundError:
                    pass


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
        description="Convert Telegram enc1 secrets from an offline SQLite copy"
    )
    parser.add_argument("input", help="path to an offline database copy")
    parser.add_argument("output", nargs="?", help="new output database path")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="preflight and count conversions without writing an output",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not args.dry_run and args.output is None:
        print("status=failed")
        return 1
    try:
        fernet_key = _read_key("TELEGRAM_FERNET_KEY", "TELEGRAM_FERNET_KEY_FD")
        credential_key = _read_key(
            "FEISHU_CREDENTIAL_KEY", "FEISHU_CREDENTIAL_KEY_FD"
        )
        result = convert_database(
            args.input, args.output, fernet_key, credential_key, dry_run=args.dry_run
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
