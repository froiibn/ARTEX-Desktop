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
from pathlib import Path, PurePosixPath
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
