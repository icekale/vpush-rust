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


class TelegramSecretMigrationTests(unittest.TestCase):
    def setUp(self):
        self.tempdir = tempfile.TemporaryDirectory()
        self.db_path = Path(self.tempdir.name) / "offline-copy.sqlite3"
        self.fernet_key = Fernet.generate_key()
        self.aes_key = os.urandom(32)
        self.aes_key_b64 = base64.urlsafe_b64encode(self.aes_key).decode("ascii")
        self.legacy_plaintext = "123456:ABCDEFGHIJKLMNOPQRST"
        self._create_database(
            [
                "enc1:" + Fernet(self.fernet_key).encrypt(self.legacy_plaintext.encode()).decode(),
                "bare-legacy-token",
                "",
                "enc2:already-converted",
            ]
        )

    def tearDown(self):
        self.tempdir.cleanup()

    def _create_database(self, tokens):
        with sqlite3.connect(self.db_path) as connection:
            connection.execute(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, telegram_bot_token TEXT NOT NULL)"
            )
            connection.executemany(
                "INSERT INTO users (telegram_bot_token) VALUES (?)",
                [(token,) for token in tokens],
            )

    def _tokens(self):
        with sqlite3.connect(self.db_path) as connection:
            return connection.execute(
                "SELECT telegram_bot_token FROM users ORDER BY id"
            ).fetchall()

    def test_dry_run_counts_enc1_without_writing(self):
        before = self.db_path.read_bytes()

        result = migrate_telegram_secrets.convert_database(
            self.db_path, self.fernet_key, self.aes_key_b64, dry_run=True
        )

        self.assertEqual(result.converted, 1)
        self.assertEqual(result.unchanged, 3)
        self.assertTrue(result.dry_run)
        self.assertEqual(self.db_path.read_bytes(), before)
        self.assertEqual(self._tokens()[0][0][:5], "enc1:")

    def test_conversion_matches_rust_aes_gcm_and_is_idempotent(self):
        result = migrate_telegram_secrets.convert_database(
            self.db_path, self.fernet_key, self.aes_key_b64
        )

        self.assertEqual(result.converted, 1)
        converted = self._tokens()[0][0]
        self.assertTrue(converted.startswith("enc2:"))
        packed = base64.urlsafe_b64decode(converted[5:])
        self.assertEqual(
            AESGCM(self.aes_key).decrypt(packed[:12], packed[12:], None).decode(),
            self.legacy_plaintext,
        )
        self.assertEqual(self._tokens()[1:], [
            ("bare-legacy-token",),
            ("",),
            ("enc2:already-converted",),
        ])

        after_first_run = self.db_path.read_bytes()
        second = migrate_telegram_secrets.convert_database(
            self.db_path, self.fernet_key, self.aes_key_b64
        )
        self.assertEqual(second.converted, 0)
        self.assertEqual(second.unchanged, 4)
        self.assertEqual(self.db_path.read_bytes(), after_first_run)

    def test_wrong_fernet_key_leaves_database_byte_for_byte_unchanged(self):
        before = self.db_path.read_bytes()

        with self.assertRaises(migrate_telegram_secrets.MigrationError):
            migrate_telegram_secrets.convert_database(
                self.db_path, Fernet.generate_key(), self.aes_key_b64
            )

        self.assertEqual(self.db_path.read_bytes(), before)
        self.assertTrue(self._tokens()[0][0].startswith("enc1:"))

    def test_malformed_ciphertext_and_invalid_utf8_fail_before_any_write(self):
        for bad_value in (
            "enc1:not-a-fernet-token",
            "enc1:" + Fernet(self.fernet_key).encrypt(b"\xff").decode(),
        ):
            with self.subTest(bad_value=bad_value):
                with sqlite3.connect(self.db_path) as connection:
                    connection.execute(
                        "UPDATE users SET telegram_bot_token = ? WHERE id = 2",
                        (bad_value,),
                    )
                before = self.db_path.read_bytes()

                with self.assertRaises(migrate_telegram_secrets.MigrationError):
                    migrate_telegram_secrets.convert_database(
                        self.db_path, self.fernet_key, self.aes_key_b64
                    )

                self.assertEqual(self.db_path.read_bytes(), before)
                self.assertTrue(self._tokens()[0][0].startswith("enc1:"))
                with sqlite3.connect(self.db_path) as connection:
                    self.assertEqual(
                        connection.execute(
                            "SELECT telegram_bot_token FROM users WHERE id = 2"
                        ).fetchone()[0],
                        bad_value,
                    )
                with sqlite3.connect(self.db_path) as connection:
                    connection.execute(
                        "UPDATE users SET telegram_bot_token = ? WHERE id = 2",
                        ("bare-legacy-token",),
                    )


    def test_cli_reads_keys_from_environment_and_does_not_accept_key_argument(self):
        environment = {
            "TELEGRAM_FERNET_KEY": self.fernet_key.decode("ascii"),
            "FEISHU_CREDENTIAL_KEY": self.aes_key_b64,
        }
        before = self.db_path.read_bytes()
        with contextlib.ExitStack() as stack:
            old = {name: os.environ.get(name) for name in environment}
            stack.callback(
                lambda: os.environ.update(
                    {
                        name: value
                        for name, value in old.items()
                        if value is not None
                    }
                )
            )
            stack.callback(
                lambda: [
                    os.environ.pop(name, None)
                    for name, value in old.items()
                    if value is None
                ]
            )
            os.environ.update(environment)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    migrate_telegram_secrets.main(
                        [str(self.db_path), "--dry-run"]
                    ),
                    0,
                )

        self.assertIn("dry_run=1", output.getvalue())
        self.assertIn("converted=1", output.getvalue())
        self.assertEqual(self.db_path.read_bytes(), before)
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()) as error:
            migrate_telegram_secrets.build_parser().parse_args(
                [str(self.db_path), "--fernet-key", "sentinel-key-not-accepted"]
            )
        self.assertNotIn("sentinel-key-not-accepted", error.getvalue())

    def test_cli_reads_keys_from_protected_file_descriptors(self):
        fernet_read, fernet_write = os.pipe()
        aes_read, aes_write = os.pipe()
        try:
            os.write(fernet_write, self.fernet_key + b"\n")
            os.write(aes_write, self.aes_key_b64.encode("ascii") + b"\n")
        finally:
            os.close(fernet_write)
            os.close(aes_write)

        old_environment = {
            name: os.environ.get(name)
            for name in (
                "TELEGRAM_FERNET_KEY",
                "FEISHU_CREDENTIAL_KEY",
                "TELEGRAM_FERNET_KEY_FD",
                "FEISHU_CREDENTIAL_KEY_FD",
            )
        }
        try:
            for name in old_environment:
                os.environ.pop(name, None)
            os.environ["TELEGRAM_FERNET_KEY_FD"] = str(fernet_read)
            os.environ["FEISHU_CREDENTIAL_KEY_FD"] = str(aes_read)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(
                    migrate_telegram_secrets.main([str(self.db_path), "--dry-run"]), 0
                )
        finally:
            os.close(fernet_read)
            os.close(aes_read)
            for name, value in old_environment.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value

        self.assertIn("status=ok", output.getvalue())
        self.assertIn("converted=1", output.getvalue())


if __name__ == "__main__":
    unittest.main()
