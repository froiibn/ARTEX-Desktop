#!/usr/bin/env python3
"""Collect verified native distribution notices, without a developer-path baseline.

Python 3.11+, cargo/rustc and a populated Cargo cache are required. The normal /
build graph includes build tools; the index never equates that with embedded code.
GNU binary collection requires the actual linker map. Unknown licenses, modified
cached sources, unverified runtime provenance and ambiguous notices fail closed.
Network requests are limited to pinned upstream notices/component archives.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
import urllib.error
import zipfile

ROOT = Path(__file__).resolve().parents[3]
MANIFEST = ROOT / "desktop/native/Cargo.toml"
LOCK = MANIFEST.with_name("Cargo.lock")
NOTICE = re.compile(r"^(?:licen[cs]e|copying|notice|copyright)(?:[-._]|$)", re.I)
KNOWN = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib",
         "0BSD", "MIT-0", "BSL-1.0", "Unlicense", "CC0-1.0", "MPL-2.0",
         "Unicode-3.0", "OFL-1.1", "Ubuntu-font-1.0", "CDLA-Permissive-2.0",
         "Apache-2.0 WITH LLVM-exception", "LGPL-2.1-or-later"}
PREFERENCE = ["MIT", "Apache-2.0", "ISC", "BSD-3-Clause", "BSD-2-Clause",
              "Zlib", "BSL-1.0", "0BSD", "MIT-0", "Unlicense", "CC0-1.0",
              "Apache-2.0 WITH LLVM-exception", "LGPL-2.1-or-later"]


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def windows_build_path(value):
    """Accept ordinary absolute drive/UNC paths, including Rust's verbatim spelling."""
    def normalize(text):
        if not isinstance(text, str) or any(character in text for character in "\0\r\n"):
            raise RuntimeError("Invalid Windows build-record path")
        text = text.replace("/", "\\")
        if text.lower().startswith("\\\\?\\unc\\"):
            text = "\\\\" + text[8:]
        elif text.startswith("\\\\?\\"):
            if not re.match(r"^[A-Za-z]:\\", text[4:]):
                raise RuntimeError("Windows build-record device namespace is not permitted")
            text = text[4:]
        parsed = PureWindowsPath(text)
        if not parsed.is_absolute() or text.startswith(("\\\\.\\", "\\??\\")):
            raise RuntimeError("Windows build-record path must be an absolute drive or UNC path")
        if parsed.drive.startswith("\\\\"):
            pieces = parsed.drive[2:].split("\\")
            if len(pieces) != 2 or any(part in ("", ".", "?", "..") or ":" in part for part in pieces):
                raise RuntimeError("Invalid Windows UNC build-record path")
        elif not re.fullmatch(r"[A-Za-z]:", parsed.drive):
            raise RuntimeError("Unsupported Windows build-record namespace")
        if ":" in text[len(parsed.drive):]:
            raise RuntimeError("Windows build-record alternate streams are not permitted")
        return Path(text)
    return normalize(str(normalize(value).resolve()))


def command(*args: str) -> str:
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True,
                            encoding="utf-8", errors="replace")
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def safe_member(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or "\\" in name or ":" in name:
        raise RuntimeError(f"Unsafe archive member: {name}")
    return path


def select_license(expression: str, available_text: str | None = None) -> list[str]:
    """Interpret Cargo's historical slash as OR; never drop an AND component."""
    tokens = re.findall(r"\(|\)|AND|OR|WITH|[A-Za-z0-9.+-]+", expression.replace("/", " OR "))
    pos = 0
    def atom():
        nonlocal pos
        if pos >= len(tokens):
            raise RuntimeError(f"Malformed license: {expression}")
        if tokens[pos] == "(":
            pos += 1
            value = alternatives()
            if pos >= len(tokens) or tokens[pos] != ")":
                raise RuntimeError(f"Malformed license: {expression}")
            pos += 1
            return value
        identifier = tokens[pos]
        pos += 1
        if pos < len(tokens) and tokens[pos] == "WITH":
            if pos + 1 >= len(tokens):
                raise RuntimeError(f"Malformed exception: {expression}")
            identifier += " WITH " + tokens[pos + 1]
            pos += 2
        if identifier not in KNOWN:
            raise RuntimeError(f"License requires policy review: {identifier}")
        return [identifier]
    def conjunction():
        nonlocal pos
        result = atom()
        while pos < len(tokens) and tokens[pos] == "AND":
            pos += 1
            result += atom()
        return sorted(set(result))
    def alternatives():
        nonlocal pos
        candidates = [conjunction()]
        while pos < len(tokens) and tokens[pos] == "OR":
            pos += 1
            candidates.append(conjunction())
        def rank(value):
            missing = sum(not has_terms(available_text, v) for v in value) if available_text is not None else 0
            return (missing, max((PREFERENCE.index(v) if v in PREFERENCE else 100) for v in value), len(value))
        return min(candidates, key=rank)
    chosen = alternatives()
    if pos != len(tokens):
        raise RuntimeError(f"Malformed license: {expression}")
    return chosen


def has_terms(text: str, license_id: str) -> bool:
    text = re.sub(r"\s+", " ", text).lower()
    signatures = {
        "MIT": ["permission is hereby granted", "without restriction"],
        "MIT-0": ["permission is hereby granted", "without restriction"],
        "Apache-2.0": ["apache license", "version 2.0", "redistribution"],
        "Apache-2.0 WITH LLVM-exception": ["apache license", "llvm exception"],
        "BSD-2-Clause": ["redistribution and use in source and binary forms", "disclaimer"],
        "BSD-3-Clause": ["redistribution and use in source and binary forms", "endorse"],
        "ISC": ["permission to use, copy, modify", "with or without fee"],
        "Zlib": ["as-is", "altered source versions"],
        "0BSD": ["permission to use, copy, modify", "with or without fee"],
        "BSL-1.0": ["boost software license", "version 1.0"],
        "Unlicense": ["unencumbered software", "public domain"],
        "CC0-1.0": ["cc0", "waiver"],
        "MPL-2.0": ["mozilla public license", "3.2", "executable"],
        "Unicode-3.0": ["unicode", "copyright and permission notice"],
        "OFL-1.1": ["open font license", "1.1"],
        "Ubuntu-font-1.0": ["ubuntu font licence", "1.0"],
        "CDLA-Permissive-2.0": ["community data license agreement", "2.1"],
        "LGPL-2.1-or-later": ["gnu lesser general public license", "2.1"],
    }
    return all(item in text for item in signatures[license_id])


class Collector:
    def __init__(self, args):
        self.args = args
        self.output = args.output.resolve()
        if self.output.exists():
            raise RuntimeError("Output must be a new directory; existing distributions are immutable")
        self.output.mkdir(parents=True)
        self.cache = args.cache.resolve() if args.cache else Path(tempfile.mkdtemp(prefix="artex-license-cache-"))
        self.cache.mkdir(parents=True, exist_ok=True)
        self.files = []
        self.provenance = []
        self.registry_native_inputs = {}

    def write(self, relative: str, data: bytes, origin: str, kind: str):
        safe_member(relative)
        path = self.output / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.exists() and path.read_bytes() != data:
            raise RuntimeError(f"Conflicting notice: {relative}")
        path.write_bytes(data)
        if not any(item["path"] == relative for item in self.files):
            self.files.append({"path": relative, "sha256": sha(data), "bytes": len(data),
                               "origin": origin, "kind": kind})
        return relative

    def json(self, relative, data):
        return self.write(relative, (json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode(),
                          "generated from verified input", "index")

    def fetch(self, url: str, expected: str | None = None) -> Path:
        if not url.startswith("https://"):
            raise RuntimeError("Remote provenance must use HTTPS")
        key = sha(url.encode())
        target = self.cache / key
        receipt = self.cache / (key + ".json")
        if receipt.exists():
            saved = json.loads(receipt.read_text())
            if saved.get("url") == url and saved.get("status") == 404:
                raise urllib.error.HTTPError(url, 404, "Pinned source previously returned 404", None, None)
        if not target.exists() or (not receipt.exists() and not expected):
            if self.args.offline:
                raise RuntimeError(f"Offline source not cached: {url}")
            request = urllib.request.Request(url, headers={"User-Agent": "ARTEX-Desktop-license-collector"})
            try:
                with urllib.request.urlopen(request, timeout=60) as response, target.with_suffix(".part").open("wb") as stream:
                    shutil.copyfileobj(response, stream)
            except urllib.error.HTTPError as error:
                if error.code == 404:
                    receipt.write_text(json.dumps({"url": url, "status": 404}) + "\n", encoding="utf-8")
                raise
            target.with_suffix(".part").replace(target)
            receipt.write_text(json.dumps({"url": url, "sha256": file_sha(target)}) + "\n", encoding="utf-8")
        if receipt.exists():
            saved = json.loads(receipt.read_text())
            if saved["url"] != url or saved["sha256"] != file_sha(target):
                raise RuntimeError(f"Cached upstream material changed: {url}")
        elif not expected:
            # Existing files without a trusted digest are not silently accepted.
            raise RuntimeError(f"Cached source has no integrity receipt: {url}")
        if expected and file_sha(target) != expected:
            raise RuntimeError(f"Upstream checksum mismatch: {url}")
        return target

    def crates(self):
        metadata = json.loads(command("cargo", "metadata", "--locked", "--offline", "--format-version", "1",
                                      "--filter-platform", self.args.target, "--manifest-path", str(MANIFEST)))
        tree = command("cargo", "tree", "--locked", "--offline", "--manifest-path", str(MANIFEST),
                       "--target", self.args.target, "--edges", "normal,build", "--prefix", "none", "--format", "{p}")
        keys = set()
        for line in tree.splitlines():
            match = re.match(r"([^ ]+) v([^ ]+)", line)
            if match:
                keys.add(match.groups())
        locked = tomllib.loads(LOCK.read_text(encoding="utf-8"))["package"]
        checksums = {(p["name"], p["version"], p.get("source")): p.get("checksum") for p in locked}
        self.packages = []
        for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
            if (package["name"], package["version"]) not in keys:
                continue
            if not package["source"]:
                if package["license"] != "AGPL-3.0-only":
                    raise RuntimeError(f"First-party license changed: {package['name']}")
                continue
            if not package["source"].startswith("registry+"):
                raise RuntimeError(f"Non-registry dependency needs separate provenance: {package['name']}")
            base = Path(package["manifest_path"]).parent
            stem = package["name"] + "-" + package["version"]
            archive = base.parent.parent.parent / "cache" / base.parent.name / (stem + ".crate")
            expected = checksums.get((package["name"], package["version"], package["source"]))
            if not expected or not archive.is_file() or file_sha(archive) != expected:
                raise RuntimeError(f"Missing / unverified Cargo archive: {stem}")
            entry = {"name": package["name"], "version": package["version"], "declared_license": package["license"],
                     "selected_license_branch": select_license(package["license"]), "archive_sha256": expected,
                     "source_download_url": f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download",
                     "repository": package.get("repository"), "authors": package["authors"], "notice_files": [],
                     "source_modified": False, "scope": "normal/build candidate; not proof of embedded code"}
            self.packages.append(entry)
            self.read_crate(package, entry, archive, base, stem)
        self.json("rust/index.json", {"target": self.args.target, "cargo_lock_sha256": file_sha(LOCK),
                                     "graph": "cargo tree normal,build; development edges excluded", "packages": self.packages})

    def read_crate(self, package, entry, archive, base, stem):
        all_data = {}
        with tarfile.open(archive, "r:gz") as tar:
            for member in tar:
                if not member.isfile():
                    continue
                path = safe_member(member.name).relative_to(stem)
                content = tar.extractfile(member).read()
                if not (base / path).is_file() or (base / path).read_bytes() != content:
                    raise RuntimeError(f"Cached dependency differs from locked original: {stem}/{path}")
                all_data[path.as_posix()] = content
                if path.suffix in (".a", ".o"):
                    self.registry_native_inputs[(base / path).resolve()] = {
                        "package": stem, "member": path.as_posix(), "sha256": sha(content),
                        "archive_sha256": entry["archive_sha256"], "source_download_url": entry["source_download_url"]}
                if NOTICE.match(path.name) or path.name.lower() in ("ofl.txt", "ufl.txt") or (
                        package["name"] == "epaint_default_fonts" and path.suffix == ".txt"):
                    relative = self.write(f"rust/{stem}/{path}", content, entry["source_download_url"], "original notice")
                    entry["notice_files"].append(relative)
        text = "\n".join((self.output / p).read_text(encoding="utf-8", errors="replace") for p in entry["notice_files"])
        entry["selected_license_branch"] = select_license(package["license"], text)
        audited_declaration_only = {"fxhash": "0.2.1", "mac": "0.1.1", "hexf-parse": "0.2.1",
                                    "winapi-x86_64-pc-windows-gnu": "0.4.0"}
        if package["name"] in ("fxhash", "mac", "winapi-x86_64-pc-windows-gnu") and entry["selected_license_branch"] == ["MIT"]:
            # Both audited versions explicitly offer Apache OR MIT.
            if "Apache-2.0" not in package["license"]:
                raise RuntimeError(f"Audited alternative no longer offered: {stem}")
            entry["selected_license_branch"] = ["Apache-2.0"]
        source_headers = []
        for path, content in all_data.items():
            if not path.endswith((".rs", ".c", ".h", ".cc", ".S")):
                continue
            lines = content.decode("utf-8", errors="replace").splitlines()
            header = []
            in_block = False
            for line in lines[:100]:
                stripped = line.lstrip()
                if stripped.startswith("/*"):
                    in_block = True
                if in_block or not stripped or stripped.startswith("//"):
                    header.append(line)
                    if "*/" in line:
                        in_block = False
                else:
                    break
            value = "\n".join(header)
            if re.search("copyright|licensed under|SPDX-License-Identifier", value, re.I):
                source_headers.append({"source_path": path, "original_header": value})
        if source_headers:
            entry["source_header_index"] = self.json(f"rust/{stem}/SOURCE-COPYRIGHT-HEADERS.json", source_headers)
        if package["name"] == "brotli-decompressor" and package["version"] == "6.0.1":
            # This old slash declaration offers BSD/MIT, but a ported Google file
            # independently retains an explicit MIT notice. Preserve it as an
            # additive source notice rather than discarding it after selecting BSD.
            header = all_data["src/context.rs"].decode().splitlines()[:12]
            if not any("Copyright 2013 Google" in line for line in header) or not any("MIT" in line for line in header):
                raise RuntimeError("Audited Brotli port copyright header changed")
            terms = (ROOT / "docs/licenses/common/MIT.txt").read_bytes()
            entry["notice_files"].append(self.write(f"rust/{stem}/Google-MIT-HEADER.txt", "\n".join(header).encode(), entry["source_download_url"], "original additive source notice"))
            entry["notice_files"].append(self.write(f"rust/{stem}/standard-MIT.txt", terms, "https://opensource.org/licenses/MIT", "standard terms supplement; copyright in original header"))
            entry["additional_source_license"] = "MIT for Google-origin source; BSD branch selection does not erase it"
        if "MPL-2.0" in entry["selected_license_branch"]:
            header = next((data.decode("utf-8", errors="replace")[:1000] for path, data in all_data.items()
                           if path.endswith(".rs") and b"Mozilla Public" in data[:1000]), None)
            if not has_terms(text, "MPL-2.0"):
                if not header:
                    raise RuntimeError(f"MPL provenance cannot be attached: {stem}")
                mpl = (ROOT / "desktop/crates/domain/resources/psl/MPL-2.0.txt").read_bytes()
                if not has_terms(mpl.decode(), "MPL-2.0"):
                    raise RuntimeError("Standard MPL text is invalid")
                entry["notice_files"].append(self.write(f"rust/{stem}/MPL-2.0.txt", mpl,
                                                        "https://www.mozilla.org/en-US/MPL/2.0/", "standard terms supplement"))
                entry["notice_files"].append(self.write(f"rust/{stem}/ORIGINAL-HEADER.txt", header.encode(),
                                                        entry["source_download_url"], "original source header"))
                text += mpl.decode()
            entry["source_offer"] = {"license": "MPL-2.0", "modified": False, "download_url": entry["source_download_url"],
                                      "sha256": entry["archive_sha256"], "notice": "The covered source is available at this exact version URL under MPL-2.0. No covered source modifications were made."}
        missing = [value for value in entry["selected_license_branch"] if not has_terms(text, value)]
        if missing:
            vcs = json.loads(all_data.get(".cargo_vcs_info.json", b"{}"))
            revision = vcs.get("git", {}).get("sha1")
            match = re.match(r"https://github.com/([^/]+/[^/]+)", package.get("repository") or "")
            if revision and match:
                repository = match.group(1).removesuffix(".git")
                for filename in ("LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "COPYING", "NOTICE"):
                    url = f"https://raw.githubusercontent.com/{repository}/{revision}/{filename}"
                    try:
                        content = self.fetch(url).read_bytes()
                    except urllib.error.HTTPError as error:
                        if error.code == 404:
                            continue
                        raise
                    if filename == "NOTICE" or any(has_terms(content.decode("utf-8", errors="replace"), value) for value in missing):
                        entry["notice_files"].append(self.write(f"rust/{stem}/upstream/{filename}", content, url,
                                                               "pinned package-repository notice"))
                        text += content.decode("utf-8", errors="replace")
            missing = [value for value in entry["selected_license_branch"] if not has_terms(text, value)]
        if missing and audited_declaration_only.get(package["name"]) == package["version"]:
            # Audited old packages publish a license declaration without a separate
            # LICENSE file. Keep declaration, exact original attribution/source,
            # and unmodified standard terms; never pretend a fabricated copyright
            # notice was an upstream file.
            for value in missing:
                if value == "Apache-2.0":
                    terms = (ROOT / "docs/licenses/brevduva-Apache-2.0.txt").read_bytes()
                elif value == "CC0-1.0":
                    terms = (ROOT / "docs/licenses/common/CC0-1.0.txt").read_bytes()
                else:
                    raise RuntimeError(f"Audited exception branch changed: {stem}/{value}")
                entry["notice_files"].append(self.write(f"rust/{stem}/standard-{value}.txt", terms,
                                                        "declared license standard terms; not original author-specific notice", "standard terms supplement"))
                text += terms.decode()
            for path, content in all_data.items():
                if path.endswith(".rs") or path in ("Cargo.toml", "Cargo.toml.orig", "README.md"):
                    entry["notice_files"].append(self.write(f"rust/{stem}/declaration-evidence/{path}", content,
                                                            entry["source_download_url"], "original declaration/source attribution"))
            entry["notice_collection_exception"] = "Audited absence of standalone LICENSE; original declaration/source attribution and standard terms preserved."
        unresolved = [value for value in entry["selected_license_branch"] if not has_terms(text, value)]
        if unresolved:
            raise RuntimeError(f"Unresolved original license/notice mapping: {stem}: {unresolved}")
        if package["name"] == "epaint_default_fonts":
            fonts = []
            for path, content in all_data.items():
                if path.endswith(".ttf"):
                    names = font_names(content)
                    fonts.append({"file": path, "sha256": sha(content), "name_records": names})
            hack = all_data.get("fonts/Hack-Regular.txt", b"")
            if b"BITSTREAM VERA LICENSE" not in hack or b"Source Foundry" not in hack:
                raise RuntimeError("Hack's underlying Bitstream notice is missing")
            if not any("Copyright 2011 Canonical" in str(p) for p in fonts) or not any("Copyright 2013 Google" in str(p) for p in fonts):
                raise RuntimeError("Default-font copyright mapping changed; review new fonts")
            self.json("fonts/default-fonts.json", {"package": stem, "fonts": fonts,
                       "licenses": "Hack: MIT + Bitstream Vera; Noto Emoji: OFL-1.1; Ubuntu Light: Ubuntu-font-1.0; emoji-icon-font: MIT",
                       "font_files_license_separate_from_application": True, "modified": False})

    def resources(self):
        paths = ["LICENSE", "docs/licenses/third-party-notices.md", "docs/licenses/norma-Apache-2.0.txt",
                 "docs/licenses/brevduva-Apache-2.0.txt", "docs/licenses/brevduva-NOTICE.txt",
                 "docs/licenses/brevduva-provenance.json", "desktop/native/assets/fonts/OFL.txt",
                 "desktop/native/assets/fonts/PROVENANCE.md", "desktop/crates/domain/resources/psl/LICENSE",
                 "desktop/crates/domain/resources/psl/MPL-2.0.txt", "desktop/crates/domain/resources/psl/PROVENANCE.md",
                 "desktop/crates/domain/resources/psl/public_suffix_list.dat"]
        branding = ["artex-app-icon-v1.png", "artex-app-icon-32.png", "artex-app-icon-256.png",
                    "artex-app-icon.ico", "artex-app-icon-v1-prompt.txt", "README.md"]
        branding_paths = ["desktop/native/assets/branding/" + name for name in branding]
        branding_paths += ["desktop/native/build.rs", "desktop/native/examples/export-app-icons.rs"]
        paths += branding_paths
        policy = json.loads((ROOT / "desktop/native/scripts/license-inputs.json").read_text())
        for record in policy["standard_terms_supplements"]:
            if file_sha(ROOT / record["path"]) != record["sha256"]:
                raise RuntimeError(f"Standard terms supplement changed: {record['path']}")
        for relative in paths:
            path = ROOT / relative
            if not path.is_file():
                raise RuntimeError(f"Required native attribution missing: {relative}")
            if relative in policy["source_material_sha256"] and file_sha(path) != policy["source_material_sha256"][relative]:
                raise RuntimeError(f"Pinned native source material changed: {relative}")
            self.write("source/" + relative, path.read_bytes(), "repository:" + relative, "retained source/resource notice")
        frames = icon_frames((ROOT / "desktop/native/assets/branding/artex-app-icon.ico").read_bytes())
        approved = (ROOT / "desktop/native/assets/branding/artex-app-icon-v1.png").read_bytes()
        if len(approved) < 33 or approved[:16] != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR":
            raise RuntimeError("Approved icon source is not a PNG image")
        width, height = struct.unpack_from(">II", approved, 16)
        if width != height or width < 256:
            raise RuntimeError("Approved icon source must be square and at least 256 pixels")
        for dimension in (32, 256):
            frame = next(data for entry, data in frames if (entry[0] or 256) == dimension)
            if frame != (ROOT / f"desktop/native/assets/branding/artex-app-icon-{dimension}.png").read_bytes():
                raise RuntimeError("ICO frame differs from the retained application PNG")
        self.json("source/resources.json", {
            "provider": "ARTEX Desktop first-party artwork and resource build code",
            "scope": "Approved generated artwork and derived platform icons; no additional third-party library",
            "license_context": "Application AGPL-3.0-only; artwork provenance/attribution retained in branding README",
            "files": [{"path": relative, "sha256": file_sha(ROOT / relative),
                       "size": (ROOT / relative).stat().st_size} for relative in branding_paths],
            "ico_png_sizes": [entry[0] or 256 for entry, _ in frames],
            "ico_32_256_match_retained_png": True})
        brevduva = json.loads((ROOT / "docs/licenses/brevduva-provenance.json").read_text())
        for record in brevduva["source_files"]:
            if file_sha(ROOT / record["local_path"]) != record["sha256"]:
                raise RuntimeError("Brevduva original notice changed")
        self.json("source/reference-notices.json", {
            "norma": {"version": "0.4.3", "source_commit": "52f17dd52d95b087265e5d5b306fe85c1ea05599",
                      "license": "Apache-2.0", "copyright": "Copyright 2026 Autumn-27", "modified": True,
                      "scope": "Rust port of source contracts/prompts; original attribution remains"},
            "brevduva": brevduva,
            "psl": {"revision": "d6c92f1bbb7433e5db7b8405c25d4035fb8ff376", "license": "MPL-2.0",
                    "code_license": "BSD-3-Clause", "go_version": "golang.org/x/net v0.58.0",
                    "preferred_source": "source/desktop/crates/domain/resources/psl/public_suffix_list.dat",
                    "original_source_url": "https://github.com/publicsuffix/list/blob/d6c92f1bbb7433e5db7b8405c25d4035fb8ff376/public_suffix_list.dat"}})
        korean_font = ROOT / "desktop/native/assets/fonts/NotoSansKR-Regular.ttf"
        if file_sha(korean_font) != policy["source_material_sha256"][korean_font.relative_to(ROOT).as_posix()]:
            raise RuntimeError("Embedded Korean font changed; review modification/license provenance")
        self.json("fonts/Noto-Sans-KR.json", {"file": "NotoSansKR-Regular.ttf", "sha256": file_sha(korean_font),
                  "name_records": font_names(korean_font.read_bytes()), "license": "OFL-1.1",
                  "modified": "Static weight400 instancing of the original variable font; see retained PROVENANCE.md",
                  "font_license_separate_from_application": True})

    def runtime(self):
        if self.args.target != "x86_64-pc-windows-gnu":
            raise RuntimeError("Native runtime policy currently qualifies Windows GNU x64 only")
        if not self.args.link_map or not self.args.binary:
            raise RuntimeError("Windows binary requires --binary and its actual --link-map")
        binary = self.args.binary.resolve()
        map_path = self.args.link_map.resolve()
        if not binary.is_file() or not map_path.is_file():
            raise RuntimeError("Built binary/linker map missing")
        text = map_path.read_text(encoding="utf-8", errors="replace")
        if "Linker script and memory map" not in text or "LOAD " not in text:
            raise RuntimeError("Invalid GNU linker map")
        outputs = re.findall(r"(?m)^OUTPUT\((.+) pei-x86-64\)$", text)
        if len(outputs) != 1:
            raise RuntimeError("Linker map has no unique Windows x64 output")
        map_output = Path(outputs[0]).resolve()
        if not map_output.is_file() or file_sha(map_output) != file_sha(binary):
            raise RuntimeError("Linker map output does not match the supplied binary")
        sysroot = Path(command("rustc", "--print", "sysroot")).resolve()
        compiler = Path(shutil.which("gcc") or "").resolve()
        if not compiler.is_file():
            raise RuntimeError("GNU compiler cannot be located")
        root = compiler.parent.parent
        version_file = root / "version_info.txt"
        policy = json.loads((ROOT / "desktop/native/scripts/license-inputs.json").read_text())
        vendor = policy["windows_gnu_runtime"]
        gcc_version = command(str(compiler), "-dumpfullversion")
        if gcc_version != vendor["gcc_version"] or not version_file.is_file() or vendor["vendor_build"] not in version_file.read_text():
            raise RuntimeError("GNU compiler distribution has no reviewed exact-version policy")
        vendor_archive = self.fetch(vendor["archive_url"], vendor["archive_sha256"])
        component_manifest = tomllib.loads((sysroot / "lib/rustlib/multirust-channel-manifest.toml").read_text())
        rust_version = command("rustc", "-vV")
        rust_target = component_manifest["pkg"]["rust-std"]["target"][self.args.target]
        std_archive = self.fetch(rust_target["xz_url"], rust_target["xz_hash"])
        with tarfile.open(std_archive, "r:xz") as tar:
            std_files = {"/".join(safe_member(m.name).parts[2:]): sha(tar.extractfile(m).read())
                         for m in tar if m.isfile() and len(safe_member(m.name).parts) > 2}
        paths = set()
        transient_inputs = []
        target_root = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "desktop/target")).resolve()
        for line in text.splitlines():
            if line.startswith("LOAD "):
                name = line[5:].strip().strip('"')
                if name.endswith((".a", ".o", ".rlib")):
                    path = Path(name)
                    if not path.is_absolute():
                        path = ROOT / path
                    if path.is_file():
                        paths.add(path.resolve())
                    else:
                        path = path.resolve()
                        if path.is_relative_to(target_root) and path.name.startswith("artex_native-") and path.name.endswith(".rcgu.o"):
                            transient_inputs.append({"file": path.name, "provider": "Cargo/rustc LTO object",
                                                     "sha256": None, "limit": "Compiler removes this transient after link; no original object-byte comparison is claimed"})
                        elif path.is_relative_to(target_root) and re.fullmatch(r"rustc[A-Za-z0-9]+", path.parent.name) and path.suffix == ".rlib":
                            original = target_root / "release/deps" / path.name
                            standard = sysroot / "lib/rustlib" / self.args.target / "lib" / path.name
                            if original.is_file():
                                paths.add(original.resolve())
                            elif standard.is_file():
                                paths.add(standard.resolve())
                            else:
                                raise RuntimeError(f"Transient Rust archive has no corresponding original: {path.name}")
                            transient_inputs.append({"file": path.name, "provider": "rustc trimmed link archive",
                                                     "sha256": None, "original_archive_sha256": file_sha(original if original.is_file() else standard),
                                                     "limit": "Original archive is recorded; removed trimmed intermediate is not asserted byte-identical"})
                        else:
                            raise RuntimeError(f"Linker input cannot be resolved: {name}")
        if not paths:
            raise RuntimeError("No actual linker inputs found")
        evidence = []
        with zipfile.ZipFile(vendor_archive) as archive:
            for path in sorted(paths):
                digest = file_sha(path)
                if path.is_relative_to(root):
                    relative = path.relative_to(root).as_posix()
                    member = "mingw64/" + relative
                    try:
                        original = archive.read(member)
                    except KeyError:
                        raise RuntimeError(f"Runtime input absent in verified compiler archive: {relative}")
                    if sha(original) != digest:
                        raise RuntimeError(f"Compiler runtime input differs from reviewed archive: {relative}")
                    evidence.append({"file": relative, "sha256": digest, "provider": "Winlibs GCC/MinGW",
                                     "archive_sha256": vendor["archive_sha256"], "kind": "loaded archive/startup input; archive members may be discarded"})
                elif path.is_relative_to(sysroot):
                    relative = path.relative_to(sysroot).as_posix()
                    if std_files.get(relative) != digest:
                        raise RuntimeError(f"Rust native runtime outside verified rust-std component: {relative}; rust-mingw inputs require additional policy")
                    evidence.append({"file": relative, "sha256": digest, "provider": "Rust rust-std component",
                                     "archive_sha256": rust_target["xz_hash"]})
                elif path in self.registry_native_inputs:
                    original = self.registry_native_inputs[path]
                    if original["sha256"] != digest:
                        raise RuntimeError("Prebuilt Cargo native library changed since source verification")
                    evidence.append({"file": original["package"] + "/" + original["member"], "sha256": digest,
                                     "provider": "locked Cargo source archive native import library", "origin": original})
                elif path.is_relative_to(Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "desktop/target")).resolve()):
                    relative = path.relative_to(Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "desktop/target")).resolve()).as_posix()
                    if re.fullmatch(r"(?:x86_64-pc-windows-gnu/)?release/build/artex-native-[0-9a-f]{16}/out/artex-icon-resource\.o", relative):
                        resource_proof = self.icon_resource(path, binary, archive, root)
                        evidence.append({"file": relative, "sha256": digest,
                                         "provider": "ARTEX Desktop first-party Windows icon/version resource",
                                         "origin": resource_proof})
                        continue
                    if "/build/" in "/" + relative and "/out/" in relative:
                        directory = relative.split("/build/", 1)[1].split("/", 1)[0]
                        candidates = [p for p in self.packages if directory.startswith(p["name"] + "-")]
                        if len(candidates) != 1:
                            raise RuntimeError(f"Compiled native library has ambiguous package attribution: {relative}")
                        owner = candidates[0]["name"] + "-" + candidates[0]["version"]
                    elif path.suffix == ".rlib" and "/deps/" in relative:
                        owner = "Cargo locked normal/build graph (Rust library)"
                    elif path.suffix == ".o":
                        owner = "ARTEX compiler-generated object"
                    else:
                        raise RuntimeError(f"Unexpected Cargo linker input: {relative}")
                    evidence.append({"file": relative, "sha256": digest, "provider": owner})
                else:
                    raise RuntimeError(f"Unreviewed native library outside compiler/Rust/Cargo provenance: {path.name}")
            if sha(archive.read("mingw64/bin/gcc.exe")) != file_sha(compiler):
                raise RuntimeError("Compiler executable differs from verified vendor archive")
        if sum(record["provider"] == "ARTEX Desktop first-party Windows icon/version resource" for record in evidence) != 1:
            raise RuntimeError("Linker map must load exactly one verified first-party icon/version object")
        for record in policy["runtime_notice_files"]:
            path = ROOT / record["path"]
            if not path.is_file() or file_sha(path) != record["sha256"]:
                raise RuntimeError(f"Runtime original notice not verified: {record['path']}")
            self.write("runtime/" + path.name, path.read_bytes(), record["source_url"], "pinned runtime original notice")
        exception = (ROOT / vendor["gcc_exception_path"]).read_text()
        if "Eligible" not in exception or "Grant of Additional Permission" not in exception:
            raise RuntimeError("GCC runtime exception text is incomplete")
        for name in ("COPYRIGHT-library.html", "README.md"):
            source = sysroot / "share/doc/rust" / name
            if not source.is_file():
                raise RuntimeError(f"Installed Rust runtime attribution missing: {name}")
            self.write("runtime/rust/" + name, source.read_bytes(), "installed verified-version Rust runtime documents", "runtime attribution")
        for source in (sysroot / "share/doc/rust/licenses").glob("*"):
            if source.is_file():
                self.write("runtime/rust/licenses/" + source.name, source.read_bytes(), "installed Rust license texts", "runtime terms")
        self.json("runtime/provenance.json", {"binary_sha256": file_sha(binary), "link_map_sha256": file_sha(map_path),
                  "map_output_binary_sha256": file_sha(map_output), "map_output_matches_binary": True,
                  "rust_version": rust_version, "gcc_version": gcc_version, "gcc_executable_sha256": file_sha(compiler),
                  "compiler_distribution": vendor["vendor_build"], "compiler_archive_url": vendor["archive_url"],
                  "compiler_archive_sha256": vendor["archive_sha256"], "rust_std_archive_url": rust_target["xz_url"],
                  "rust_std_archive_sha256": rust_target["xz_hash"], "loaded_inputs": evidence,
                  "removed_compiler_transient_inputs": transient_inputs,
                  "gcc_exception": "GCC Runtime Library Exception 3.1 permits combined Target Code from eligible compilation, subject to independent modules' licenses. Toolchain use alone does not relicense ARTEX. No non-GPL-compatible GCC intermediate-code optimizer is introduced by this Cargo build.",
                  "limit": "Linker LOAD provenance is conservative; import archives are not Microsoft DLL redistribution. This verifies technical inputs/notices, not legal certification."})

    def icon_resource(self, path, binary, archive, compiler_root):
        """Qualify only the root application's exact icon object, never other native outputs."""
        package = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))["package"]
        if package["name"] != "artex-native" or package["license"] != "AGPL-3.0-only":
            raise RuntimeError("First-party icon resource owner changed")
        icon = ROOT / "desktop/native/assets/branding/artex-app-icon.ico"
        resource = path.with_suffix(".rc")
        source = resource.read_text(encoding="utf-8")
        match = re.findall(r'(?m)^1 ICON "([^"\r\n]+)"$', source)
        if len(match) != 1 or Path(match[0]).resolve() != icon.resolve():
            raise RuntimeError("Root icon RC does not reference the retained ICO exactly")
        version = package["version"]
        core, _, pre = version.partition("-")
        revision = pre.rsplit(".", 1)[-1]
        revision = int(revision) if revision.isdigit() and int(revision) <= 65535 else 0
        numbers = core.replace(".", ",") + f",{revision}"
        for expected in ("1 VERSIONINFO", f" FILEVERSION {numbers}", f" PRODUCTVERSION {numbers}",
                         f'   VALUE "FileVersion", "{version}\\0"',
                         f'   VALUE "ProductVersion", "{version}\\0"'):
            if source.splitlines().count(expected) != 1:
                raise RuntimeError("Root icon RC version contract differs from Cargo manifest")
        frames = icon_frames(icon.read_bytes())
        object_resources = windows_resources(path.read_bytes(), coff=True)
        binary_resources = windows_resources(binary.read_bytes(), coff=False)
        verify_icon_resources(object_resources, frames, version)
        verify_icon_resources(binary_resources, frames, version)
        if object_resources != binary_resources:
            raise RuntimeError("Linked executable resource bytes differ from the first-party object")
        build_record = path.with_name("artex-icon-resource-build.json")
        build = json.loads(build_record.read_text(encoding="utf-8"))
        if not isinstance(build, dict) or set(build) != {"target", "target_env", "compiler_path", "resource_source", "linked_resource", "icon"}:
            raise RuntimeError("Icon build record has an unexpected schema")
        if build.get("target") != self.args.target or build.get("target_env") != "gnu":
            raise RuntimeError("Icon build record target differs from the qualified GNU target")
        for key, expected in (("resource_source", resource), ("linked_resource", path), ("icon", icon)):
            value = build.get(key)
            if windows_build_path(value) != windows_build_path(str(expected.resolve())):
                raise RuntimeError("Icon build record does not match the actual resource inputs/output")
        compiler_path = build.get("compiler_path")
        windres = windows_build_path(compiler_path)
        if not windres.is_file() or not windres.is_relative_to(compiler_root):
            raise RuntimeError("Icon resource compiler is outside the verified GNU distribution")
        member = "mingw64/" + windres.relative_to(compiler_root).as_posix()
        if sha(archive.read(member)) != file_sha(windres):
            raise RuntimeError("Icon resource compiler differs from the verified vendor archive")
        proof = {"owner": "artex-native", "license_context": "AGPL-3.0-only application resource",
                 "ico_sha256": file_sha(icon), "object_sha256": file_sha(path),
                 "version": version, "object_and_executable_resource_bytes_match": True,
                 "ico_frames_match_object_and_executable": True,
                 "resource_compiler": {"member": member, "sha256": file_sha(windres),
                                       "recorded_build_time_selection": True,
                                       "scope": "Verified build tool, not a redistributed dependency"},
                 "source_index": "source/resources.json",
                 "build_record_sha256": file_sha(build_record),
                 "build_record_paths_match": True,
                 "build_record_limit": "Recorded build-time paths are verified locally; only the digest/result is distributed",
                 "rc_sha256": file_sha(resource),
                 "rc_path_limit": "Generated RC contains a local absolute icon path; only its digest is distributed",
                 "limit": "Icon pixel derivation is recorded by retained exporter/provenance, not independently rerendered"}
        self.json("runtime/first-party-icon-resource.json", proof)
        return proof


def icon_frames(data):
    """Strictly validate the seven PNG frames exported by the retained Rust example."""
    if len(data) < 6 or struct.unpack_from("<HHH", data) != (0, 1, 7):
        raise RuntimeError("Application ICO header does not match the seven-frame export contract")
    frames = []
    expected_offset = 6 + 7 * 16
    for index, size in enumerate((16, 24, 32, 48, 64, 128, 256)):
        entry = data[6 + index * 16:22 + index * 16]
        width, height, colors, reserved, planes, bits, length, offset = struct.unpack("<BBBBHHII", entry)
        if (width or 256, height or 256, colors, reserved, planes, bits, offset) != (size, size, 0, 0, 1, 32, expected_offset):
            raise RuntimeError("Application ICO directory differs from the retained exporter contract")
        frame = data[offset:offset + length]
        if len(frame) != length or len(frame) < 33 or frame[:16] != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" or struct.unpack_from(">II", frame, 16) != (size, size):
            raise RuntimeError("Application ICO contains a missing or unexpected PNG frame")
        frames.append((entry, frame))
        expected_offset += length
    if expected_offset != len(data):
        raise RuntimeError("Application ICO contains unaccounted trailing bytes")
    return frames


def windows_resources(data, *, coff):
    """Read numeric resource trees, checking COFF relocation ownership and PE RVA bounds."""
    def take(start, length):
        if start < 0 or length < 0 or start + length > len(data):
            raise RuntimeError("Windows resource structure exceeds file bounds")
        return data[start:start + length]
    header = 0 if coff else struct.unpack("<I", take(0x3c, 4))[0] + 4
    if not coff and (data[:2] != b"MZ" or take(header - 4, 4) != b"PE\0\0"):
        raise RuntimeError("Resource proof requires a Windows PE executable")
    machine, count, _, symbol_table, symbol_count, optional_size, _ = struct.unpack("<HHIIIHH", take(header, 20))
    if machine != 0x8664 or not 1 <= count <= 96 or (coff and optional_size != 0):
        raise RuntimeError("Resource proof requires ordinary x64 COFF/PE")
    sections = []
    for index in range(count):
        name, virtual_size, address, size, offset, relocations, _, relocation_count, _, flags = struct.unpack(
            "<8sIIIIIIHHI", take(header + 20 + optional_size + 40 * index, 40))
        sections.append((name.rstrip(b"\0"), address, size, offset, relocations, relocation_count, flags, index + 1))
    matches = [section for section in sections if section[0] == b".rsrc"]
    if len(matches) != 1:
        raise RuntimeError("Windows file has no unique .rsrc section")
    _, address, size, offset, relocation_start, relocation_count, flags, section_number = matches[0]
    resource = take(offset, size)
    if flags & (0x20 | 0x20000000) or (coff and any(s[2] for s in sections if s[0] != b".rsrc")):
        raise RuntimeError("First-party resource object contains unexpected executable/native content")
    def record(start, length):
        if start < 0 or start + length > len(resource):
            raise RuntimeError("Resource directory exceeds .rsrc bounds")
        return resource[start:start + length]
    if coff:
        relocation_offsets = set()
        for index in range(relocation_count):
            field, symbol, kind = struct.unpack("<IIH", take(relocation_start + index * 10, 10))
            if symbol >= symbol_count or kind != 3 or field in relocation_offsets:
                raise RuntimeError("Icon COFF has an unexpected relocation")
            symbol_record = take(symbol_table + symbol * 18, 18)
            symbol_value, owner = struct.unpack_from("<Ih", symbol_record, 8)
            if owner != section_number or symbol_value != 0:
                raise RuntimeError("Icon COFF relocation references content outside .rsrc")
            relocation_offsets.add(field)
        base = 0
    else:
        optional = take(header + 20, optional_size)
        if len(optional) < 136 or struct.unpack_from("<H", optional)[0] != 0x20b:
            raise RuntimeError("Icon PE resource directory requires PE32+")
        resource_rva, resource_size = struct.unpack_from("<II", optional, 128)
        if resource_rva != address or resource_size > size or not resource_size:
            raise RuntimeError("PE resource data directory does not match .rsrc")
        base = address
        relocation_offsets = None
    result = {}
    visited = set()
    fields = set()
    def walk(directory, ids):
        if directory in visited or len(ids) >= 3:
            raise RuntimeError("Unexpected recursive/deep Windows resource tree")
        visited.add(directory)
        named, numeric = struct.unpack_from("<HH", record(directory, 16), 12)
        if named or numeric > 64:
            raise RuntimeError("First-party icon resource has unexpected named/excess entries")
        for index in range(numeric):
            identifier, pointer = struct.unpack("<II", record(directory + 16 + index * 8, 8))
            if identifier & 0x80000000:
                raise RuntimeError("Unexpected named resource identifier")
            key = ids + (identifier,)
            if pointer & 0x80000000:
                walk(pointer & 0x7fffffff, key)
            else:
                if len(key) != 3 or key in result:
                    raise RuntimeError("Malformed/duplicate Windows resource leaf")
                rva, length, _, reserved = struct.unpack("<IIII", record(pointer, 16))
                if reserved:
                    raise RuntimeError("Unexpected resource data entry")
                fields.add(pointer)
                result[key] = record(rva - base, length)
    walk(0, ())
    if relocation_offsets is not None and fields != relocation_offsets:
        raise RuntimeError("COFF relocation coverage does not match resource leaves")
    return result


def verify_icon_resources(resources, frames, version):
    groups = [(key, value) for key, value in resources.items() if key[0] == 14]
    versions = [(key, value) for key, value in resources.items() if key[0] == 16]
    if len(groups) != 1 or groups[0][0][1] != 1 or len(versions) != 1 or versions[0][0][1] != 1 or groups[0][0][2] != versions[0][0][2]:
        raise RuntimeError("Icon resource requires exactly the app icon/version group")
    group_key, group = groups[0]
    if len(group) != 6 + 14 * len(frames) or struct.unpack_from("<HHH", group) != (0, 1, len(frames)):
        raise RuntimeError("Icon resource group does not match the retained ICO")
    expected = {group_key, versions[0][0]}
    for index, (entry, frame) in enumerate(frames):
        resource_entry = group[6 + index * 14:20 + index * 14]
        identifier = struct.unpack_from("<H", resource_entry, 12)[0]
        key = (3, identifier, group_key[2])
        if resource_entry[:12] != entry[:12] or key in expected or resources.get(key) != frame:
            raise RuntimeError("Compiled icon PNG differs from the retained ICO bytes")
        expected.add(key)
    if set(resources) != expected or (version + "\0").encode("utf-16-le") not in versions[0][1]:
        raise RuntimeError("Windows resource contains unaccounted content or mismatched version")


def font_names(data: bytes):
    """Read original TTF copyright/name/license records using the standard library."""
    count = struct.unpack_from(">H", data, 4)[0]
    table = None
    for i in range(count):
        tag, _, offset, length = struct.unpack_from(">4sIII", data, 12 + 16 * i)
        if tag == b"name":
            table = data[offset:offset + length]
    if table is None:
        raise RuntimeError("Font name/copyright table missing")
    _, count, offset = struct.unpack_from(">HHH", table, 0)
    values = []
    for i in range(count):
        platform, _, _, name_id, length, start = struct.unpack_from(">HHHHHH", table, 6 + 12 * i)
        if name_id in (0, 1, 13, 14):
            raw = table[offset + start:offset + start + length]
            value = raw.decode("utf-16-be" if platform in (0, 3) else "mac_roman", errors="replace")
            item = {"name_id": name_id, "value": value}
            if item not in values:
                values.append(item)
    return values


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", default="x86_64-pc-windows-gnu")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--link-map", type=Path)
    parser.add_argument("--cache", type=Path)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--notices-only", action="store_true", help="Validate source/Cargo/font notices without claiming a distributable binary/runtime")
    args = parser.parse_args()
    collector = None
    try:
        collector = Collector(args)
        collector.resources()
        collector.crates()
        if not args.notices_only:
            collector.runtime()
        collector.json("INDEX.json", {"status": "source-notices-only" if args.notices_only else "verified-distribution-inputs",
                       "legal_certification": False, "binary_distribution_ready": not args.notices_only,
                       "collector_sha256": file_sha(Path(__file__)),
                       "input_policy_sha256": file_sha(ROOT / "desktop/native/scripts/license-inputs.json"),
                       "files": sorted(collector.files, key=lambda p: p["path"]),
                       "scope": "Native application normal/build graph, embedded resources and observed GNU linker inputs; external user-installed programs excluded"})
        print(json.dumps({"passed": True, "binary_distribution_ready": not args.notices_only,
                          "third_party_normal_build_candidates": len(collector.packages), "notice_files": len(collector.files)}, indent=2))
        return 0
    except Exception as error:
        if collector:
            collector.json("INCOMPLETE.json", {"passed": False, "binary_distribution_ready": False,
                                               "reason": str(error), "partial_output_must_not_be_distributed": True})
        print(f"LICENSE_COLLECTION_FAILED: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
