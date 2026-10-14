#!/usr/bin/env python3
"""Assemble verified Windows GNU release assets from a committed native package.

Python 3.11+ and Git are required. This command neither builds nor uploads, signs,
installs, runs the application, or changes source. A new/empty output directory
is required. Failed assemblies remain marked INCOMPLETE and must not be shipped.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import tomllib
from urllib.parse import quote, unquote, urlsplit
import zipfile


TARGET = "x86_64-pc-windows-gnu"
BUNDLE_NAME = "ARTEX-Windows-x64"
HEX256 = re.compile(r"^[0-9a-f]{64}$")
SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9]+(?:[.-][A-Za-z0-9]+)*)?$")
SENSITIVE = {
    "developer-home": re.compile(
        rb"(?:[A-Za-z]:[\\/]+Users[\\/]+[^\\/\s\x00\"'<>:]{1,80}[\\/]|"
        rb"/(?:Users|home)/[^/\s\x00\"'<>:]{1,80}/)", re.I),
    "anthropic-token": re.compile(rb"sk-ant-api[0-9]+-[A-Za-z0-9_-]{20,}"),
    "openai-token": re.compile(rb"sk-(?:proj-|svcacct-)?[A-Za-z0-9_-]{32,}"),
    "github-token": re.compile(rb"(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})"),
    "aws-access-key": re.compile(rb"(?:AKIA|ASIA)[A-Z0-9]{16}"),
    "private-key": re.compile(rb"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----"),
}


# Exact original-crate examples reviewed on 2026-10-14. Never exempt a directory,
# a changed version/hash, source archive contents, or any additional category.
REVIEWED_UPSTREAM_EXAMPLES = {'ARTEX-Windows-x64/licenses/rust/aws-lc-rs-1.18.2/SOURCE-COPYRIGHT-HEADERS.json': {'sha256': 'efd0f7f0f11f2ba71421a9c69b69d5a0020f03c8a13a977c2766e4e744a8202c',
                                                                                    'categories': ['developer-home'],
                                                                                    'classification': 'upstream-public-source-example',
                                                                                    'source_url': 'https://crates.io/api/v1/crates/aws-lc-rs/1.18.2/download',
                                                                                    'cargo_archive_sha256': 'faac5829c2b74c28f830747e7818ccfb684261b5f48a1118b1e2a13d36dfab13'},
 'ARTEX-Windows-x64/licenses/rust/futures-channel-0.3.34/SOURCE-COPYRIGHT-HEADERS.json': {'sha256': 'cfa222dafbcad9e6d3709319712661fa6b8df2234fcfc316eca7be05e82d1949',
                                                                                          'categories': ['developer-home'],
                                                                                          'classification': 'json-escaping-false-positive-in-upstream-documentation-url',
                                                                                          'source_url': 'https://crates.io/api/v1/crates/futures-channel/0.3.34/download',
                                                                                          'cargo_archive_sha256': 'b1f9e3d69d39e4862ffed03ed071a76f9a13ba1d9109d355b0f0aa6b15e393c4'},
 'ARTEX-Windows-x64/licenses/rust/pem-4.0.0/SOURCE-COPYRIGHT-HEADERS.json': {'sha256': '251ec990be7d873aee4d363bbd50bdd5a1451a540d44b1cc18ed280c37c59d63',
                                                                             'categories': ['private-key'],
                                                                             'classification': 'publicly-published-library-documentation-example-RSA-private-key',
                                                                             'source_url': 'https://crates.io/api/v1/crates/pem/4.0.0/download',
                                                                             'cargo_archive_sha256': 'd354a98a3d1251555de99e8fdd8afda05573c31b82f59063a7b0a29b5527f120'},
 'ARTEX-Windows-x64/licenses/rust/pxfm-0.1.30/SOURCE-COPYRIGHT-HEADERS.json': {'sha256': '7436e7589df4a914853a1be77db6175c2f27c29ca44e2d40e0bb4971ea8d2c19',
                                                                               'categories': ['developer-home'],
                                                                               'classification': 'upstream-public-source-example',
                                                                               'source_url': 'https://crates.io/api/v1/crates/pxfm/0.1.30/download',
                                                                               'cargo_archive_sha256': 'd55d956fa96f5ec02be2e13af0e20391a5aa83d6a074e3ad368959d0fab299ea'}}


class ReleaseError(RuntimeError):
    pass


def require(condition, message: str):
    if not condition:
        raise ReleaseError(message)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_json(path: Path):
    require(path.is_file() and not path.is_symlink(), "Required JSON evidence is absent or a link")
    try:
        return json.loads(path.read_text(encoding="utf-8-sig"))
    except (ValueError, UnicodeError):
        raise ReleaseError("Required JSON evidence cannot be parsed") from None


def json_bytes(value) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def git(repo: Path, *arguments: str) -> bytes:
    process = subprocess.run(["git", *arguments], cwd=repo, capture_output=True)
    require(process.returncode == 0, "Git evidence/archive command failed")
    return process.stdout


def git_text(repo: Path, *arguments: str) -> str:
    return git(repo, *arguments).decode("utf-8").strip()


def relative_path(name: str) -> PurePosixPath:
    require(isinstance(name, str) and bool(name), "Archive/index path is empty")
    path = PurePosixPath(name)
    require(not path.is_absolute() and ".." not in path.parts and "\\" not in name
            and ":" not in name and not name.startswith("/"), "Unsafe archive/index path")
    require(path.as_posix() == name.rstrip("/"), "Noncanonical archive/index path")
    return path


def regular_files(directory: Path) -> dict[str, Path]:
    require(directory.is_dir() and not directory.is_symlink(), "Required payload directory is absent or linked")
    result = {}
    folded = set()
    for parent, folders, files in os.walk(directory, followlinks=False):
        for name in folders + files:
            entry = Path(parent) / name
            require(not entry.is_symlink() and not (getattr(entry.lstat(), "st_file_attributes", 0) & 0x400),
                    "Linked payload entry is not permitted")
        for name in files:
            entry = Path(parent) / name
            require(entry.is_file(), "Payload entry is not a regular file")
            relative = entry.relative_to(directory).as_posix()
            relative_path(relative)
            check_privacy(relative.encode("utf-8"), "payload entry name")
            require(relative.casefold() not in folded, "Payload has a Windows path collision")
            folded.add(relative.casefold())
            result[relative] = entry
    return result


def privacy_kinds(data: bytes) -> list[str]:
    # Also finds ordinary UTF-16 string literals without decoding arbitrary PE/font bytes.
    views = (data, data.replace(b"\x00", b"")) if b"\x00" in data else (data,)
    return [kind for kind, pattern in SENSITIVE.items() if any(pattern.search(view) for view in views)]


def check_privacy(data: bytes, name: str, *, reviewed_runtime_examples: bool = False):
    kinds = privacy_kinds(data)
    # Matched values are never printed or persisted. Only the relative entry and categories.
    example = REVIEWED_UPSTREAM_EXAMPLES.get(name) if reviewed_runtime_examples else None
    if kinds and example and sha(data) == example["sha256"] and kinds == example["categories"]:
        return
    require(not kinds, f"Distribution privacy scan rejected entry {name}: {', '.join(kinds)}")


def github_slug(repo: Path) -> str:
    remote = git_text(repo, "config", "--get", "remote.origin.url")
    match = re.fullmatch(r"(?:https://github\.com/|git@github\.com:)([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(?:\.git)?", remote)
    require(match is not None, "A credential-free GitHub origin is required for source/document links")
    slug = match.group(1)
    require(all(part not in (".", "..") for part in slug.split("/")), "Invalid GitHub repository identity")
    return slug


def verify_package(repo: Path, result_path: Path):
    require(git_text(repo, "rev-parse", "--show-toplevel").replace("\\", "/").casefold()
            == repo.as_posix().casefold(), "--repo must identify the Git repository root")
    require(not git(repo, "status", "--porcelain", "--untracked-files=all"),
            "Commit all source changes before assembling a release")
    report = read_json(result_path)
    require(isinstance(report, dict) and report.get("passed") is True, "Package verification did not pass")
    version = report.get("version", "")
    require(isinstance(version, str) and SEMVER.fullmatch(version), "Package version is not a supported SemVer")
    manifest = tomllib.loads((repo / "desktop/native/Cargo.toml").read_text(encoding="utf-8"))
    require(version == manifest["package"]["version"], "Package and native source versions differ")
    commit = git_text(repo, "rev-parse", "HEAD")
    require(report.get("source_commit") == commit, "Package source commit differs from Git HEAD")
    require(report.get("compiler_path_remapping") is True, "Compiler path remapping evidence is required")
    require(report.get("platform") == "Windows x86_64", "Only a verified Windows x64 package is supported")
    rust_version = report.get("rust_version", "")
    require(isinstance(rust_version, str) and re.search(r"(?m)^host: " + re.escape(TARGET) + r"\s*$", rust_version),
            "Package evidence must identify the Windows GNU Rust host")
    for field in ("self_test_exit_code", "window_exit_code"):
        require(type(report.get(field)) is int and report[field] == 0, "Package self-test/GUI did not exit successfully")
    require(report.get("orderly_shutdown") is True, "Orderly application shutdown is unverified")
    require(report.get("legal_notice_window_verified") is True, "Actual packaged legal-notice window evidence is required")
    sections = report.get("rendered_sections")
    require(isinstance(sections, list) and len(sections) == 23
            and all(isinstance(item, str) and item for item in sections) and len(set(sections)) == 23,
            "Exactly 23 distinct rendered sections are required")
    require(report.get("observed_children") == [], "Packaged native GUI must have no child processes")
    require(isinstance(report.get("observed_modules"), list), "Native GUI module evidence is missing")
    require(not any(re.search(r"WebView2Loader|\bwry\b", str(name), re.I) for name in report["observed_modules"]),
            "Legacy GUI module evidence is present")
    for field in ("executable_sha256", "verification_sha256", "native_link_map_sha256"):
        require(isinstance(report.get(field), str) and HEX256.fullmatch(report[field]), "Required evidence SHA256 is invalid")
    bundle = Path(report.get("bundle", "")).resolve()
    require(bundle.name == BUNDLE_NAME, "Expected ARTEX-Windows-x64 package directory")
    require(bundle.is_dir() and {p.name for p in bundle.iterdir()} == {"ARTEX.exe", "LICENSE.txt", "licenses"},
            "Package input must contain only the executable, LICENSE.txt and licenses")
    files = regular_files(bundle)
    binary = bundle / "ARTEX.exe"
    require(Path(report.get("executable", "")).resolve() == binary, "Package executable reference differs from bundle")
    require(file_sha(binary) == report["executable_sha256"] and binary.stat().st_size == report.get("executable_bytes"),
            "Verified executable bytes changed")
    icon_path = result_path.parent / "audit/icon-verification.json"
    require(icon_path.is_file() and file_sha(icon_path) == report.get("icon_verification_sha256"),
            "Packaged icon acceptance evidence changed or is absent")
    icon = read_json(icon_path)
    require(icon == report.get("icon_verification") and icon.get("passed") is True
            and icon.get("binary_sha256") == report["executable_sha256"]
            and icon.get("source_ico_sha256") == file_sha(repo / "desktop/native/assets/branding/artex-app-icon.ico"),
            "Packaged icon evidence does not identify the executable and approved ICO")
    require(icon.get("resources", {}).get("exact_source_match") is True
            and [frame.get("size") for frame in icon.get("resources", {}).get("frames", [])]
            == [16, 24, 32, 48, 64, 128, 256]
            and bool(icon.get("window", {}).get("windows")),
            "Seven approved executable frames and a real native-window icon are required")
    with binary.open("rb") as stream:
        header = stream.read(64)
        require(len(header) == 64 and header[:2] == b"MZ", "Executable lacks a PE header")
        stream.seek(struct.unpack_from("<I", header, 60)[0])
        pe = stream.read(6)
        require(pe[:4] == b"PE\x00\x00" and pe[4:] == b"\x64\x86", "Executable is not a Windows x64 PE")
    require((bundle / "LICENSE.txt").read_bytes() == (repo / "LICENSE").read_bytes(), "Bundled project license changed")
    verification_path = Path(report.get("verification_result", "")).resolve()
    require(file_sha(verification_path) == report["verification_sha256"], "Full regression report changed")
    verification = read_json(verification_path)
    require(verification.get("passed") is True and verification.get("full_regression") is True,
            "Passing full native regression evidence is required")
    for source in verification.get("source_files", []):
        require(file_sha(Path(source["path"])) == source["sha256"], "Source changed since full native verification")
    require(len(verification.get("source_files", [])) >= 100, "Full regression source evidence is incomplete")
    link_map = result_path.parent / "audit/native-link.map"
    require(link_map.is_file() and file_sha(link_map) == report["native_link_map_sha256"], "Native linker evidence changed")
    verify_licenses(repo, bundle, report, files)
    return report, bundle, commit, version


def verify_licenses(repo: Path, bundle: Path, report: dict, payload_files: dict[str, Path]):
    require("licenses/INCOMPLETE.json" not in payload_files, "Incomplete license collection cannot be distributed")
    index = read_json(bundle / "licenses/INDEX.json")
    require(index.get("status") == "verified-distribution-inputs"
            and index.get("binary_distribution_ready") is True and index.get("legal_certification") is False,
            "A complete distribution-input license collection is required; source-only notices are insufficient")
    records = index.get("files")
    require(isinstance(records, list) and bool(records), "License collection has no indexed files")
    indexed = set()
    for record in records:
        require(isinstance(record, dict), "Malformed license index record")
        name = record.get("path", "")
        relative_path(name)
        require(name not in indexed and name != "INDEX.json", "Duplicate/self-referencing license index record")
        indexed.add(name)
        file = bundle / "licenses" / name
        require(file.is_file() and not file.is_symlink() and type(record.get("bytes")) is int
                and file.stat().st_size == record["bytes"] and file_sha(file) == record.get("sha256"),
                "Indexed license/attribution bytes changed")
    actual = {name.removeprefix("licenses/") for name in payload_files if name.startswith("licenses/")}
    require(actual == indexed | {"INDEX.json"}, "License collection contains unindexed or missing files")
    collection = report.get("license_collection")
    require(isinstance(collection, dict) and collection.get("passed") is True,
            "Packager license collection evidence is missing")
    for field, file in (("index_sha256", bundle / "licenses/INDEX.json"),
                        ("collector_sha256", repo / "desktop/native/scripts/collect-licenses.py"),
                        ("input_policy_sha256", repo / "desktop/native/scripts/license-inputs.json")):
        require(isinstance(collection.get(field), str) and HEX256.fullmatch(collection[field])
                and file.is_file() and file_sha(file) == collection[field], "License collection/source policy evidence changed")
    rust_index = read_json(bundle / "licenses/rust/index.json")
    require(rust_index.get("target") == TARGET
            and rust_index.get("cargo_lock_sha256") == file_sha(repo / "desktop/native/Cargo.lock")
            and isinstance(rust_index.get("packages"), list) and bool(rust_index["packages"]),
            "License Rust graph does not match the committed target/Cargo.lock")
    provenance = read_json(bundle / "licenses/runtime/provenance.json")
    require(provenance.get("binary_sha256") == report["executable_sha256"]
            and provenance.get("link_map_sha256") == report["native_link_map_sha256"],
            "License runtime provenance belongs to a different binary/link map")
    require(bool(provenance.get("loaded_inputs")) and HEX256.fullmatch(provenance.get("compiler_archive_sha256", ""))
            and HEX256.fullmatch(provenance.get("rust_std_archive_sha256", "")), "Runtime archive/input provenance is missing")


def source_tree(repo: Path, commit: str):
    entries = {}
    folded = set()
    for record in git(repo, "ls-tree", "-rz", "--full-tree", commit).split(b"\x00"):
        if not record:
            continue
        metadata, raw_path = record.split(b"\t", 1)
        mode, kind, oid = metadata.decode("ascii").split()
        name = raw_path.decode("utf-8")
        relative_path(name)
        check_privacy(name.encode("utf-8"), "source entry name")
        require(kind == "blob" and mode in ("100644", "100755", "120000"), "Source archive cannot omit submodules or unknown tree objects")
        require(name.casefold() not in folded, "Source tree has a Windows path collision")
        entries[name] = {"object": oid, "mode": mode}
        folded.add(name.casefold())
    require(bool(entries), "Committed source tree is empty")
    return entries


def verify_source_archive(repo: Path, commit: str, archive: Path, prefix: str, tree: dict):
    process = subprocess.Popen(["git", "cat-file", "--batch"], cwd=repo, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        with zipfile.ZipFile(archive) as zip:
            names = [info.filename for info in zip.infolist() if not info.is_dir()]
            require(len(names) == len(set(names)), "Source ZIP contains duplicate entries")
            expected = {prefix + name for name in tree}
            require(set(names) == expected, "Source ZIP differs from Git tree: missing/extra entries")
            for name, entry in tree.items():
                process.stdin.write(entry["object"].encode("ascii") + b"\n")
                process.stdin.flush()
                header = process.stdout.readline().split()
                require(len(header) == 3 and header[0].decode() == entry["object"] and header[1] == b"blob",
                        "Git source blob evidence is unavailable")
                original = process.stdout.read(int(header[2]))
                require(len(original) == int(header[2]) and process.stdout.read(1) == b"\n", "Incomplete Git source blob")
                data = zip.read(prefix + name)
                require(data == original, f"Source ZIP modified Git blob: {name}")
                check_privacy(data, name)
        process.stdin.close()
        require(process.wait() == 0, "Git blob verification failed")
    finally:
        if process.poll() is None:
            process.stdin.close()
            process.terminate()
            process.wait()
    return {"commit": commit, "git_tree": git_text(repo, "rev-parse", commit + "^{tree}"),
            "blob_count": len(tree), "exact_blob_bytes": True, "missing": 0, "extra": 0}


def committed_document(repo: Path, commit: str, relative: str, slug: str, tree: dict) -> bytes:
    require(relative in tree, "Required release document is not in the committed source")
    text = git(repo, "show", commit + ":" + relative).decode("utf-8-sig")
    base = PurePosixPath(relative).parent
    def replace(match):
        target = match.group(1)
        if urlsplit(target).scheme or target.startswith("#"):
            return match.group(0)
        parts = urlsplit(target)
        path = PurePosixPath(os.path.normpath(str(base / unquote(parts.path))).replace("\\", "/"))
        name = path.as_posix()
        require(name in tree, "Committed release document has a broken relative link")
        if name == "LICENSE":
            destination = "LICENSE.txt" if base == PurePosixPath(".") else "../LICENSE.txt"
        else:
            destination = f"https://github.com/{slug}/blob/{commit}/{quote(name, safe='/')}"
            if parts.fragment:
                destination += "#" + parts.fragment
        return "](" + destination + ")"
    return re.sub(r"\]\(([^)]+)\)", replace, text).encode("utf-8")


def make_payload(repo: Path, commit: str, version: str, report: dict, bundle: Path,
                 payload: Path, slug: str, tree: dict, source_proof: dict):
    payload.mkdir()
    shutil.copyfile(bundle / "ARTEX.exe", payload / "ARTEX.exe")
    shutil.copyfile(bundle / "LICENSE.txt", payload / "LICENSE.txt")
    shutil.copytree(bundle / "licenses", payload / "licenses", symlinks=False)
    copies = {"README.md": "SOURCE-README.md", "README.en.md": "SOURCE-README.en.md",
              "README.zh-CN.md": "SOURCE-README.zh-CN.md", "ARTEX_NOTICE.md": "ARTEX_NOTICE.md",
              "ARTEX_NOTICE.en.md": "ARTEX_NOTICE.en.md", "ARTEX_NOTICE.zh-CN.md": "ARTEX_NOTICE.zh-CN.md",
              "docs/download-and-run.md": "docs/download-and-run.md", "docs/license-policy.md": "docs/license-policy.md"}
    for original, destination in copies.items():
        file = payload / destination
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(committed_document(repo, commit, original, slug, tree))
    guide = (payload / "docs/download-and-run.md").read_text(encoding="utf-8")
    require("v" + version in guide, "First-run guide must describe the packaged candidate version before committing")
    rust_verbose = report["rust_version"]
    rust_release = re.search(r"(?m)^release: ([A-Za-z0-9.+-]+)$", rust_verbose)
    require(rust_release is not None, "Rust release version metadata is missing")
    provenance = read_json(payload / "licenses/runtime/provenance.json")
    gcc_release = re.search(r"\b[0-9]+\.[0-9]+\.[0-9]+\b", str(provenance.get("gcc_version", "")))
    require(gcc_release is not None, "GCC release version metadata is missing")
    build = {"format_version": 1, "version": version, "target": TARGET, "source_commit": commit,
             "source_url": f"https://github.com/{slug}/tree/{commit}", "source_tree": source_proof["git_tree"],
             "executable": "ARTEX.exe", "executable_sha256": report["executable_sha256"],
             "executable_bytes": report["executable_bytes"], "rust_release": rust_release.group(1),
             "toolchain": {"rust_release": rust_release.group(1), "gcc_release": gcc_release.group(0),
                           "target": TARGET, "runtime_provenance": "licenses/runtime/provenance.json"},
             "compiler_path_remapping": True, "native_link_map_sha256": report["native_link_map_sha256"],
             "package_result_sha256": report["_package_result_sha256"], "verification_sha256": report["verification_sha256"],
             "license_index_sha256": file_sha(payload / "licenses/INDEX.json"),
             "license_collector_sha256": report["license_collection"]["collector_sha256"],
             "license_input_policy_sha256": report["license_collection"]["input_policy_sha256"],
             "application_icon": {"source_ico_sha256": report["icon_verification"]["source_ico_sha256"],
                                  "executable_frame_sizes": [16, 24, 32, 48, 64, 128, 256],
                                  "window_png_size": 256, "tray_png_size": 32,
                                  "provenance": "licenses/runtime/first-party-icon-resource.json"},
             "checks": {"full_native_regression": True, "packaged_self_test_exit_code": 0,
                        "packaged_gui_exit_code": 0, "orderly_shutdown": True, "rendered_section_count": 23,
                        "source_archive_exact_git_blobs": source_proof["blob_count"], "native_children": 0,
                        "packaged_legal_notice_window_verified": True,
                        "packaged_executable_and_live_window_icons_verified": True,
                        "distribution_input_notices_verified": True},
             "limits": {"code_signed": False, "installer": False, "automatic_updates": False,
                        "macos_arch_physical_qualification": False, "legal_certification": False,
                        "secret_scan_is_exhaustive": False},
             "scope": "Verified Windows x64 GNU build; user-installed external tools, workspaces, API keys and audit logs are excluded."}
    reviewed = []
    for name, file in sorted(regular_files(payload).items()):
        entry = BUNDLE_NAME + "/" + name
        data = file.read_bytes()
        check_privacy(data, entry, reviewed_runtime_examples=True)
        if privacy_kinds(data):
            reviewed.append({"file": name, **REVIEWED_UPSTREAM_EXAMPLES[entry]})
    build["privacy_scan"] = {"unreviewed_matches": 0, "matched_values_recorded": False,
                             "reviewed_upstream_examples": reviewed}
    (payload / "BUILD-INFO.json").write_bytes(json_bytes(build))
    source_url = f"https://github.com/{slug}/tree/{commit}"
    intro = f"""# ARTEX Desktop v{version} — Windows x64

이 폴더의 `ARTEX.exe`를 실행합니다. ZIP 전체를 압축 해제한 뒤 작업 공간을 만들고, LLM 키·모델을 저장해 기본 모델로 설정합니다. 외부 도구와 API 키는 포함되지 않습니다. 서명되지 않은 Windows GNU 시험 빌드이며 macOS·Arch Linux 검증 및 법적 사용권의 지속적 보증을 뜻하지 않습니다.

Run `ARTEX.exe` after extracting the entire ZIP. Create a workspace, save the LLM key and model, and select the default model. External tools and API keys are not bundled. This unsigned Windows GNU preview does not qualify macOS/Arch Linux or certify ongoing legal compliance.

解压整个 ZIP 后运行 `ARTEX.exe`。创建工作空间，保存 LLM 密钥和模型并设置默认模型。程序包不含外部工具或 API 密钥。此未签名 Windows GNU 预览版不代表 macOS/Arch Linux 已通过验证，也不保证持续的法律合规。

소스 / Source / 源码: [{commit}]({source_url}) — `ARTEX-Desktop-v{version}-source.zip`.

원작자 / Original author / 原作者: **Autumn-27**. 수정 버전 / Modified version / 修改版本. See `ARTEX_NOTICE.md`, `ARTEX_NOTICE.en.md`, and `ARTEX_NOTICE.zh-CN.md`. Project license: [AGPL-3.0-only](LICENSE.txt). [`BUILD-INFO.json`](BUILD-INFO.json) records the build evidence; `FILE-SHA256SUMS.txt` records package file hashes.

다음은 같은 소스 commit의 첫 실행 안내입니다. / First-run guide from the same source commit. / 以下为同一源码 commit 的首次运行指南。

"""
    # The copied guide is one directory deeper; its relative LICENSE link must be rebased here.
    (payload / "README.md").write_text(intro + guide.replace("](../LICENSE.txt)", "](LICENSE.txt)"), encoding="utf-8", newline="\n")
    files = regular_files(payload)
    hashes = {name: file_sha(file) for name, file in sorted(files.items())}
    checksum = "".join(f"{digest}  {name}\n" for name, digest in hashes.items())
    (payload / "FILE-SHA256SUMS.txt").write_text(checksum, encoding="utf-8", newline="\n")
    return regular_files(payload)


def runtime_archive(payload: Path, archive: Path, files: dict[str, Path]):
    with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zip:
        for name, path in sorted(files.items()):
            check_privacy(path.read_bytes(), BUNDLE_NAME + "/" + name, reviewed_runtime_examples=True)
            zip.write(path, BUNDLE_NAME + "/" + name)
    expected = {BUNDLE_NAME + "/" + name: file_sha(path) for name, path in files.items()}
    with tempfile.TemporaryDirectory(prefix="artex-release-roundtrip-") as temporary:
        extracted = Path(temporary)
        with zipfile.ZipFile(archive) as zip:
            names = [info.filename for info in zip.infolist()]
            require(len(names) == len(set(names)) and set(names) == set(expected), "Runtime ZIP contains missing/extra entries")
            for info in zip.infolist():
                relative_path(info.filename)
                require(not info.is_dir() and (info.external_attr >> 16) & 0o170000 != 0o120000,
                        "Runtime ZIP contains an unexpected directory or symlink")
                file = extracted / info.filename
                file.parent.mkdir(parents=True, exist_ok=True)
                with zip.open(info) as source, file.open("wb") as target:
                    shutil.copyfileobj(source, target)
                require(file_sha(file) == expected[info.filename], "Runtime ZIP roundtrip changed file bytes")
        require(len(regular_files(extracted)) == len(expected), "Runtime ZIP extraction inventory changed")
        sums = (extracted / BUNDLE_NAME / "FILE-SHA256SUMS.txt").read_text(encoding="utf-8").splitlines()
        manifest = {}
        for line in sums:
            digest, name = line.split("  ", 1)
            relative_path(name)
            require(HEX256.fullmatch(digest) and name not in manifest, "Invalid package file hash manifest")
            manifest[name] = digest
        require(set(manifest) == set(files) - {"FILE-SHA256SUMS.txt"}, "Package file hash manifest is incomplete")
        require(all(manifest[name] == expected[BUNDLE_NAME + "/" + name] for name in manifest),
                "Package file hash manifest does not match extracted bytes")
    return {"files": len(expected), "roundtrip_sha256_verified": True, "missing": 0, "extra": 0}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-result", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[3])
    args = parser.parse_args()
    output = args.output.resolve()
    created_output = False
    try:
        repo = args.repo.resolve()
        require((output != repo and repo not in output.parents) or output.is_relative_to(repo / "desktop/dist"),
                "Release output inside source must be under ignored desktop/dist")
        require(not output.exists() or output.is_dir() and not any(output.iterdir()), "Release output must be new or empty")
        report_path = args.package_result.resolve()
        report, bundle, commit, version = verify_package(repo, report_path)
        require(output != bundle and bundle not in output.parents and output not in bundle.parents,
                "Release output must not overlap the verified package input")
        slug = github_slug(repo)
        tree = source_tree(repo, commit)
        report["_package_result_sha256"] = file_sha(report_path)
        output.mkdir(parents=True, exist_ok=True)
        created_output = True
        base = "ARTEX-Desktop-v" + version
        source_zip = output / (base + "-source.zip")
        runtime_zip = output / (base + "-windows-x64.zip")
        source_prefix = base + "-source/"
        # Exclusive creation prevents an archive command from replacing an existing asset.
        with source_zip.open("xb") as source_stream:
            archived = subprocess.run(["git", "archive", "--format=zip", "--prefix=" + source_prefix, commit],
                                      cwd=repo, stdout=source_stream, stderr=subprocess.PIPE)
        require(archived.returncode == 0, "Git source archive command failed")
        source_proof = verify_source_archive(repo, commit, source_zip, source_prefix, tree)
        with tempfile.TemporaryDirectory(prefix="artex-release-payload-") as temporary:
            payload = Path(temporary) / BUNDLE_NAME
            files = make_payload(repo, commit, version, report, bundle, payload, slug, tree, source_proof)
            runtime_proof = runtime_archive(payload, runtime_zip, files)
            privacy_proof = read_json(payload / "BUILD-INFO.json")["privacy_scan"]
        assets = [{"name": path.name, "sha256": file_sha(path), "bytes": path.stat().st_size}
                  for path in (runtime_zip, source_zip)]
        (output / "SHA256SUMS.txt").write_text("".join(f"{asset['sha256']}  {asset['name']}\n" for asset in assets),
                                              encoding="utf-8", newline="\n")
        assets.append({"name": "SHA256SUMS.txt", "sha256": file_sha(output / "SHA256SUMS.txt"),
                       "bytes": (output / "SHA256SUMS.txt").stat().st_size})
        require(git_text(repo, "rev-parse", "HEAD") == commit
                and not git(repo, "status", "--porcelain", "--untracked-files=all"), "Committed source changed during assembly")
        require(file_sha(bundle / "ARTEX.exe") == report["executable_sha256"]
                and file_sha(report_path) == report["_package_result_sha256"], "Package evidence changed during assembly")
        result = {"passed": True, "version": version, "source_commit": commit, "target": TARGET,
                  "assets": assets, "executable_sha256": report["executable_sha256"],
                  "source_archive": source_proof, "runtime_archive": runtime_proof,
                  "privacy_scan": {"passed": True, "categories": list(SENSITIVE), "matched_values_recorded": False,
                                   "exhaustive_secret_detection": False, **privacy_proof},
                  "legal_certification": False, "macos_arch_physical_qualification": False,
                  "distribution_scope": "Windows x64 GNU only; three release assets, no audit/log/profile/link-map/cache payload"}
        (output / "release-result.json").write_bytes(json_bytes(result))
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except (ReleaseError, OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        message = str(error) if isinstance(error, ReleaseError) else type(error).__name__ + " while validating/assembling release inputs"
        if created_output:
            (output / "INCOMPLETE.json").write_bytes(json_bytes({"passed": False, "reason": message,
                          "partial_output_must_not_be_distributed": True}))
        print("RELEASE_ASSEMBLY_FAILED: " + message, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
