import base64
import contextlib
import io
import os
import sqlite3
import tempfile
import unittest
from pathlib import Path

from cryptography.fernet import Fernet
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from tools import migrate_telegram_secrets


VALID_TOKEN = "123456:ABCDEFGHIJKLMNOPQRST"
OTHER_VALID_TOKEN = "654321:ZYXWVUTSRQPONMLKJIHG"
_DEFAULT_OUTPUT = object()


def seal(key, plaintext):
    nonce = os.urandom(12)
    return "enc2:" + base64.urlsafe_b64encode(
        nonce + AESGCM(key).encrypt(nonce, plaintext.encode("utf-8"), None)
    ).decode("ascii")


class TelegramSecretMigrationTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.input_path = self.root / "input.sqlite3"
        self.output_path = self.root / "output.sqlite3"
        self.fernet_key = Fernet.generate_key()
        self.aes_key = os.urandom(32)
        self.aes_key_b64 = base64.urlsafe_b64encode(self.aes_key).decode("ascii")
        self._create_database(
            self.input_path,
            [
                "enc1:" + Fernet(self.fernet_key).encrypt(VALID_TOKEN.encode()).decode(),
                OTHER_VALID_TOKEN,
                "",
                seal(self.aes_key, OTHER_VALID_TOKEN),
            ],
        )

    def tearDown(self):
        self.tempdir.cleanup()

    def _create_database(self, path, tokens):
        with sqlite3.connect(path) as connection:
            connection.execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, telegram_bot_token TEXT NOT NULL)"
            )
            connection.executemany(
                "INSERT INTO users (telegram_bot_token) VALUES (?)",
                [(token,) for token in tokens],
            )

    def _tokens(self, path):
        with sqlite3.connect(path) as connection:
            return connection.execute(
                "SELECT telegram_bot_token FROM users ORDER BY id"
            ).fetchall()

    def _file_snapshot(self, path):
        return {
            candidate.name: candidate.read_bytes()
            for candidate in path.parent.glob(path.name + "*")
            if candidate.is_file() and not candidate.name.endswith("-shm")
        }

    def _run(
        self,
        input_path=None,
        output_path=_DEFAULT_OUTPUT,
        credential_key=None,
        **kwargs,
    ):
        if output_path is _DEFAULT_OUTPUT:
            output_path = self.output_path
        return migrate_telegram_secrets.convert_database(
            input_path or self.input_path,
            output_path,
            self.fernet_key,
            credential_key or self.aes_key_b64,
            **kwargs,
        )

    def test_dry_run_reads_input_only_and_counts_enc1_rows(self):
        before = self._file_snapshot(self.input_path)

        result = self._run(dry_run=True)

        self.assertEqual(result.converted, 1)
        self.assertEqual(result.unchanged, 3)
        self.assertTrue(result.dry_run)
        self.assertFalse(self.output_path.exists())
        self.assertEqual(self._file_snapshot(self.input_path), before)

    def test_apply_backs_up_and_converts_without_changing_input(self):
        before = self._file_snapshot(self.input_path)

        result = self._run()

        self.assertEqual(result.converted, 1)
        self.assertEqual(result.unchanged, 3)
        converted = self._tokens(self.output_path)[0][0]
        packed = base64.urlsafe_b64decode(converted[5:])
        self.assertEqual(
            AESGCM(self.aes_key).decrypt(packed[:12], packed[12:], None).decode(),
            VALID_TOKEN,
        )
        output_tokens = self._tokens(self.output_path)
        self.assertEqual(output_tokens[1:3], [
            (OTHER_VALID_TOKEN,),
            ("",),
        ])
        existing_packed = base64.urlsafe_b64decode(output_tokens[3][0][5:])
        self.assertEqual(
            AESGCM(self.aes_key)
            .decrypt(existing_packed[:12], existing_packed[12:], None)
            .decode(),
            OTHER_VALID_TOKEN,
        )
        self.assertEqual(self._file_snapshot(self.input_path), before)
        with sqlite3.connect(self.output_path) as connection:
            self.assertEqual(connection.execute("PRAGMA journal_mode").fetchone()[0], "delete")
            self.assertEqual(connection.execute("PRAGMA quick_check").fetchone()[0], "ok")

    def test_second_conversion_to_new_output_is_idempotent(self):
        self._run()
        second_path = self.root / "second.sqlite3"

        result = self._run(input_path=self.output_path, output_path=second_path)

        self.assertEqual(result.converted, 0)
        self.assertEqual(result.unchanged, 4)
        self.assertEqual(self._tokens(second_path), self._tokens(self.output_path))

    def test_apply_requires_new_distinct_output(self):
        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            self._run(output_path=None)
        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            self._run(output_path=self.input_path)
        self.output_path.touch()
        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            self._run()

    def test_wrong_fernet_key_leaves_input_and_no_partial_output(self):
        before = self._file_snapshot(self.input_path)

        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            migrate_telegram_secrets.convert_database(
                self.input_path,
                self.output_path,
                Fernet.generate_key(),
                self.aes_key_b64,
            )

        self.assertFalse(
            any(
                path.exists()
                for path in self.output_path.parent.glob("output.sqlite3*")
            )
        )
        self.assertEqual(self._file_snapshot(self.input_path), before)

    def test_invalid_decrypted_token_leaves_input_and_no_partial_output(self):
        invalid = "123:too-short"
        with sqlite3.connect(self.input_path) as connection:
            connection.execute(
                "UPDATE users SET telegram_bot_token = ? WHERE id = 1",
                ("enc1:" + Fernet(self.fernet_key).encrypt(invalid.encode()).decode(),),
            )
        before = self._file_snapshot(self.input_path)

        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            self._run()

        self.assertFalse(
            any(
                path.exists()
                for path in self.output_path.parent.glob("output.sqlite3*")
            )
        )
        self.assertEqual(self._file_snapshot(self.input_path), before)

    def test_malformed_or_invalid_utf8_enc1_leaves_input_and_no_partial_output(self):
        for index, stored in enumerate(
            (
                "enc1:not-a-fernet-token",
                "enc1:" + Fernet(self.fernet_key).encrypt(b"\xff").decode(),
            )
        ):
            with self.subTest(index=index):
                with sqlite3.connect(self.input_path) as connection:
                    connection.execute(
                        "UPDATE users SET telegram_bot_token = ? WHERE id = 1",
                        (stored,),
                    )
                before = self._file_snapshot(self.input_path)

                with self.assertRaises(migrate_telegram_secrets.MigrationError):
                    self._run()

                self.assertFalse(
                    any(
                        path.exists()
                        for path in self.output_path.parent.glob("output.sqlite3*")
                    )
                )
                self.assertEqual(self._file_snapshot(self.input_path), before)

    def test_wrong_aes_key_is_rejected_when_existing_enc2_is_present(self):
        before = self._file_snapshot(self.input_path)

        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            migrate_telegram_secrets.convert_database(
                self.input_path,
                self.output_path,
                self.fernet_key,
                base64.urlsafe_b64encode(os.urandom(32)).decode("ascii"),
            )

        self.assertFalse(self.output_path.exists())
        self.assertEqual(self._file_snapshot(self.input_path), before)

    def test_wal_committed_rows_are_included_and_input_is_unchanged(self):
        connection = sqlite3.connect(self.input_path)
        try:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute(
                "INSERT INTO users (telegram_bot_token) VALUES (?)",
                ("enc1:" + Fernet(self.fernet_key).encrypt(VALID_TOKEN.encode()).decode(),),
            )
            connection.commit()
            before = self._file_snapshot(self.input_path)

            result = self._run()
            self.assertEqual(self._file_snapshot(self.input_path), before)
        finally:
            connection.close()

        self.assertEqual(result.converted, 2)
        self.assertEqual(len(self._tokens(self.output_path)), 5)

    def test_cli_uses_env_keys_and_does_not_accept_keys_in_argv(self):
        environment = {
            "TELEGRAM_FERNET_KEY": self.fernet_key.decode("ascii"),
            "FEISHU_CREDENTIAL_KEY": self.aes_key_b64,
        }
        old = {name: os.environ.get(name) for name in environment}
        try:
            os.environ.update(environment)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    migrate_telegram_secrets.main(
                        [str(self.input_path), "--dry-run"]
                    ),
                    0,
                )
        finally:
            for name, value in old.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value

        self.assertIn("converted=1", output.getvalue())
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()):
            migrate_telegram_secrets.build_parser().parse_args(
                [str(self.input_path), str(self.output_path), "--fernet-key", "sentinel"]
            )

    def test_cli_reads_keys_from_protected_file_descriptors(self):
        fernet_read, fernet_write = os.pipe()
        aes_read, aes_write = os.pipe()
        try:
            os.write(fernet_write, self.fernet_key + b"\n")
            os.write(aes_write, self.aes_key_b64.encode("ascii") + b"\n")
        finally:
            os.close(fernet_write)
            os.close(aes_write)

        names = (
            "TELEGRAM_FERNET_KEY",
            "FEISHU_CREDENTIAL_KEY",
            "TELEGRAM_FERNET_KEY_FD",
            "FEISHU_CREDENTIAL_KEY_FD",
        )
        old = {name: os.environ.get(name) for name in names}
        try:
            for name in names:
                os.environ.pop(name, None)
            os.environ["TELEGRAM_FERNET_KEY_FD"] = str(fernet_read)
            os.environ["FEISHU_CREDENTIAL_KEY_FD"] = str(aes_read)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    migrate_telegram_secrets.main(
                        [str(self.input_path), "--dry-run"]
                    ),
                    0,
                )
        finally:
            os.close(fernet_read)
            os.close(aes_read)
            for name, value in old.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value

        self.assertIn("converted=1", output.getvalue())

    def test_aes_key_must_be_canonical_padded_urlsafe_base64(self):
        for index, key in enumerate(
            (
                base64.urlsafe_b64encode(self.aes_key).decode("ascii").rstrip("="),
                "+" + self.aes_key_b64[1:],
                self.aes_key_b64.lower(),
            )
        ):
            with self.subTest(index=index):
                with self.assertRaises(migrate_telegram_secrets.MigrationError):
                    self._run(
                        output_path=self.root / ("bad-" + str(len(key)) + ".sqlite3"),
                        credential_key=key,
                    )


if __name__ == "__main__":
    unittest.main()
