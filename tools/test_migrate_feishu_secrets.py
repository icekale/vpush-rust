import sqlite3
import tempfile
import unittest
from pathlib import Path

from cryptography.fernet import Fernet

from migrate_feishu_secrets import convert_database, open_sealed
from migrate_telegram_secrets import _key_from_base64


class FeishuSecretMigrationTest(unittest.TestCase):
    def test_converts_fernet_and_enc1_without_touching_source(self):
        key = Fernet.generate_key().decode()
        raw = _key_from_base64(key)
        fernet = Fernet(key.encode())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            source = root / "source.db"
            output = root / "out.db"
            db = sqlite3.connect(source)
            db.executescript(
                """
                CREATE TABLE feishu_personal_bots (
                    user_id INTEGER PRIMARY KEY,
                    app_secret_ciphertext TEXT NOT NULL
                );
                CREATE TABLE feishu_registration_sessions (
                    session_id TEXT PRIMARY KEY,
                    device_code_ciphertext TEXT NOT NULL,
                    candidate_app_secret_ciphertext TEXT NOT NULL
                );
                """
            )
            secret = fernet.encrypt(b"personal-secret").decode()
            device = "enc1:" + fernet.encrypt(b"device-code").decode()
            db.execute("INSERT INTO feishu_personal_bots VALUES (7, ?)", (secret,))
            db.execute(
                "INSERT INTO feishu_registration_sessions VALUES ('s', ?, '')",
                (device,),
            )
            db.commit()
            db.close()
            converted, unchanged = convert_database(source, output, key, key)
            self.assertEqual((converted, unchanged), (2, 1))
            out = sqlite3.connect(output)
            stored = out.execute(
                "SELECT app_secret_ciphertext FROM feishu_personal_bots"
            ).fetchone()[0]
            self.assertEqual(open_sealed(stored, raw), "personal-secret")
            stored = out.execute(
                "SELECT device_code_ciphertext FROM feishu_registration_sessions"
            ).fetchone()[0]
            self.assertEqual(open_sealed(stored, raw), "device-code")
            original = sqlite3.connect(source).execute(
                "SELECT app_secret_ciphertext FROM feishu_personal_bots"
            ).fetchone()[0]
            self.assertTrue(original.startswith("gAAAA"))


if __name__ == "__main__":
    unittest.main()
