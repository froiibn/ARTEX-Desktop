"""Exercise real local publication with isolated Cargo-output fixtures."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("native_builder", Path(__file__).with_name("build.py"))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.source = self.repo / "desktop/target/release/artex-native.exe"
        self.source.parent.mkdir(parents=True)
        self.source.write_bytes(b"first Cargo artifact")
        self.output = self.repo / "desktop/dist/ARTEX-Windows/ARTEX.exe"
        for name in ["LICENSE", "ARTEX_NOTICE.md", "ARTEX_NOTICE.en.md", "ARTEX_NOTICE.zh-CN.md"]:
            (self.repo / name).write_text(name, encoding="utf-8")
        self.event = {"reason": "compiler-artifact", "target": {"name": "artex-native", "kind": ["bin"]},
                      "executable": str(self.source)}

    def publish(self):
        with patch.object(builder.sys, "platform", "win32"), \
                patch.object(builder, "provenance", return_value={"source_commit": None, "source_worktree_dirty": None}), \
                patch.object(builder.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(self.event))) as cargo:
            result = builder.build(self.repo)
            args = cargo.call_args.args[0]
            self.assertIn("--locked", args)
            self.assertIn("--release", args)
            self.assertEqual(args[args.index("--target-dir") + 1], str(self.repo / "desktop/target"))
            return result

    def test_repeated_build_replaces_the_same_path_and_records_actual_bytes(self):
        self.assertEqual(self.publish(), self.output)
        self.source.write_bytes(b"new Cargo artifact")
        self.assertEqual(self.publish(), self.output)
        self.assertEqual(self.output.read_bytes(), b"new Cargo artifact")
        info = json.loads((self.output.parent / "BUILD-INFO.json").read_text(encoding="utf-8"))
        self.assertEqual(info["executable_sha256"], builder.sha256(self.source))
        self.assertFalse(info["distribution_ready"])
        self.assertEqual(list((self.repo / "desktop/dist").iterdir()), [self.output.parent])
        self.assertFalse(list(self.output.parent.glob(".publish-*")))

    def test_cargo_failure_keeps_the_previous_executable(self):
        self.publish()
        with patch.object(builder.sys, "platform", "win32"), \
                patch.object(builder.subprocess, "run", side_effect=subprocess.CalledProcessError(101, ["cargo"])):
            with self.assertRaises(subprocess.CalledProcessError):
                builder.build(self.repo)
        self.assertEqual(self.output.read_bytes(), b"first Cargo artifact")

    def test_missing_native_artifact_does_not_copy_stale_cache_binary(self):
        self.publish()
        self.event["target"]["name"] = "unrelated-bin"
        with self.assertRaisesRegex(RuntimeError, "exactly one"):
            self.publish()
        self.assertEqual(self.output.read_bytes(), b"first Cargo artifact")

    def test_locked_output_is_not_deleted_and_staging_is_removed(self):
        self.publish()
        self.source.write_bytes(b"new Cargo artifact")
        with patch.object(builder.os, "replace", side_effect=PermissionError("locked fixture")):
            with self.assertRaises(PermissionError):
                self.publish()
        self.assertEqual(self.output.read_bytes(), b"first Cargo artifact")
        self.assertFalse(list(self.output.parent.glob(".publish-*")))


if __name__ == "__main__":
    unittest.main()
