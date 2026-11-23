"""Keep reviewed Rust literal-pool matches distinct from real credentials."""
from pathlib import Path
import importlib.util
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("create-release.py")
SPEC = importlib.util.spec_from_file_location("release_privacy", SCRIPT)
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)
REPO = SCRIPT.resolve().parents[3]
EXECUTABLE = RELEASE.BUNDLE_NAME + "/ARTEX.exe"


class NativeLiteralPrivacyTests(unittest.TestCase):
    def setUp(self):
        self.span = b"".join(value.encode("ascii") for _, value in
                             RELEASE.REVIEWED_NATIVE_LITERAL_SPANS[0]["segments"])
        # Generated fixture only: no actual credential is stored in source.
        self.credential = b"sk-" + b"Z" * 40

    def check(self, data, *, name=EXECUTABLE, repo=REPO):
        return RELEASE.check_privacy(data, name, reviewed_native_source=repo)

    def test_reviewed_literal_pool_requires_exact_public_source_provenance(self):
        self.assertEqual(RELEASE.privacy_kinds(self.span), ["openai-token"])
        review = self.check(b"MZ\x00" + self.span + b"\x00")
        self.assertEqual(review["classification"], "compiler-coalesced-public-static-identifiers")

    def test_actual_credential_is_rejected_even_beside_approved_span(self):
        for data in (self.credential, self.span + b"\x00" + self.credential,
                     self.span + self.credential):
            with self.subTest(size=len(data)), self.assertRaises(RELEASE.ReleaseError):
                self.check(data)

    def test_changed_span_and_other_payload_entries_still_fail(self):
        with self.assertRaises(RELEASE.ReleaseError):
            self.check(self.span.replace(b"native.secret.set", b"native.secret.get"))
        with self.assertRaises(RELEASE.ReleaseError):
            self.check(self.span, name=RELEASE.BUNDLE_NAME + "/README.md")
        with self.assertRaises(RELEASE.ReleaseError):
            RELEASE.check_privacy(self.span, EXECUTABLE)

    def test_utf16_credential_detection_is_never_waived(self):
        with self.assertRaises(RELEASE.ReleaseError):
            self.check(self.span + b"\x00" + self.credential.decode().encode("utf-16-le"))

    def test_missing_or_edited_source_rejects_reviewed_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(RELEASE.ReleaseError):
                self.check(self.span, repo=root)
            for relative, value in RELEASE.REVIEWED_NATIVE_LITERAL_SPANS[0]["segments"]:
                source = root / relative
                source.parent.mkdir(parents=True, exist_ok=True)
                with source.open("ab") as stream:
                    stream.write(b'"' + value.encode() + b'"\n')
            self.assertIsNotNone(self.check(self.span, repo=root))
            (root / "desktop/native/src/model.rs").write_bytes(b'"different_collection"')
            with self.assertRaises(RELEASE.ReleaseError):
                self.check(self.span, repo=root)

    def test_other_secret_classes_are_rejected(self):
        with self.assertRaises(RELEASE.ReleaseError):
            self.check(self.span + b"\x00" + b"ghp_" + b"Y" * 35)


if __name__ == "__main__":
    unittest.main()
