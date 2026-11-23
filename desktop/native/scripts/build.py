#!/usr/bin/env python3
"""Build the native host and replace its single standard local executable.

Python 3.11+ and the host's Rust/native build tools are development requirements.
This does not create or upload a distributable release; package.ps1 retains its
separate verification, license, provenance and committed-source requirements.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

REPO = Path(__file__).resolve().parents[3]


def host_output(platform: str) -> tuple[str, str, str]:
    if platform == "win32":
        return "ARTEX-Windows", "artex-native.exe", "ARTEX.exe"
    if platform == "darwin":
        return "ARTEX-macOS", "artex-native", "artex-native"
    if platform.startswith("linux"):
        return "ARTEX-Linux", "artex-native", "artex-native"
    raise RuntimeError(f"Unsupported native build host: {platform}")


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def provenance(repo: Path) -> dict:
    # Source archives may have no .git. Unknown provenance is explicit; never
    # turn a missing Git checkout into a clean/committed-source claim.
    try:
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo,
                              capture_output=True, text=True, check=True).stdout.strip()
        status = subprocess.run(["git", "status", "--porcelain", "--untracked-files=all"],
                                cwd=repo, capture_output=True, text=True, check=True).stdout
        return {"source_commit": head, "source_worktree_dirty": bool(status)}
    except (OSError, subprocess.CalledProcessError):
        return {"source_commit": None, "source_worktree_dirty": None}


def build(repo: Path = REPO) -> Path:
    repo = repo.resolve()
    folder, cargo_name, app_name = host_output(sys.platform)
    cache = (repo / "desktop/target").resolve()
    destination = repo / "desktop/dist" / folder
    # A redirected directory must never turn local publication into a write
    # outside this checkout. The same rule applies to the shared Cargo cache.
    if not cache.is_relative_to(repo) or not destination.resolve().is_relative_to(repo):
        raise RuntimeError("Build cache/output must stay inside this checkout")
    result = subprocess.run(
        ["cargo", "build", "--release", "--locked", "--manifest-path",
         str(repo / "desktop/native/Cargo.toml"), "--target-dir", str(cache),
         "--message-format=json-render-diagnostics"],
        cwd=repo, stdout=subprocess.PIPE, text=True, check=True,
    )
    artifacts = set()
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == "artex-native"
                and "bin" in message.get("target", {}).get("kind", [])
                and message.get("executable")):
            artifacts.add(Path(message["executable"]).resolve())
    if len(artifacts) != 1:
        raise RuntimeError("Cargo did not identify exactly one native executable")
    source = artifacts.pop()
    if not source.is_relative_to(cache) or source.name != cargo_name or not source.is_file():
        raise RuntimeError("Cargo executable does not match this host/cache")
    digest = sha256(source)
    info = {"kind": "local-development-build", "distribution_ready": False,
            "profile": "release", "platform": sys.platform,
            "built_utc": datetime.now(timezone.utc).isoformat(),
            "executable": app_name, "executable_sha256": digest, **provenance(repo)}
    destination.mkdir(parents=True, exist_ok=True)
    # Cargo failure never replaces the previous runnable build. Copy on the
    # destination volume and replace only complete bytes; a running/locked
    # executable fails visibly, without deleting the previous application.
    with tempfile.TemporaryDirectory(prefix=".publish-", dir=destination) as staging:
        staged = Path(staging) / app_name
        shutil.copy2(source, staged)
        if sha256(staged) != digest:
            raise RuntimeError("Native executable copy failed integrity verification")
        os.replace(staged, destination / app_name)
        for source_name, output_name in [("LICENSE", "LICENSE.txt"),
                                         ("ARTEX_NOTICE.md", "ARTEX_NOTICE.md"),
                                         ("ARTEX_NOTICE.en.md", "ARTEX_NOTICE.en.md"),
                                         ("ARTEX_NOTICE.zh-CN.md", "ARTEX_NOTICE.zh-CN.md")]:
            staged_notice = Path(staging) / output_name
            shutil.copy2(repo / source_name, staged_notice)
            os.replace(staged_notice, destination / output_name)
        staged_info = Path(staging) / "BUILD-INFO.json"
        staged_info.write_text(json.dumps(info, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(staged_info, destination / "BUILD-INFO.json")
    print(destination / app_name)
    return destination / app_name


if __name__ == "__main__":
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        build()
    except (OSError, RuntimeError, ValueError, subprocess.CalledProcessError) as error:
        print(f"NATIVE_BUILD_FAILED: {error}", file=sys.stderr)
        sys.exit(1)
