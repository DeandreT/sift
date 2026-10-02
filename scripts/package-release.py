#!/usr/bin/env python3
"""Package a built Sift desktop binary with documentation and SHA-256 checksums."""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parent.parent
TARGETS = {"x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def verify_binary(data, target):
    if target.endswith("windows-msvc"):
        if data[:2] != b"MZ" or len(data) < 64:
            raise ValueError("Windows package requires a PE executable")
        header = int.from_bytes(data[60:64], "little")
        if data[header : header + 6] != b"PE\x00\x00\x64\x86":
            raise ValueError("Windows package requires an x86_64 PE executable")
    elif data[:6] != b"\x7fELF\x02\x01" or data[18:20] != b"\x3e\x00":
        raise ValueError("Linux package requires an x86_64 little-endian ELF executable")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", choices=sorted(TARGETS), required=True)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "target/release-artifacts")
    parser.add_argument("--tag", help="If supplied, must match the package version, e.g. v0.1.0")
    args = parser.parse_args()

    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
        parser.error("workspace package version must be a SemVer version")
    if args.tag is not None and args.tag != f"v{version}":
        parser.error(f"tag {args.tag!r} does not match workspace version v{version}")

    binary = args.binary.read_bytes()
    verify_binary(binary, args.target)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(
        ["git", "status", "--porcelain", "--untracked-files=normal"], cwd=ROOT, text=True
    ).strip())
    timestamp = int(os.environ.get("SOURCE_DATE_EPOCH") or subprocess.check_output(
        ["git", "log", "-1", "--format=%ct"], cwd=ROOT, text=True
    ).strip())
    name = f"sift-{version}-{args.target}"
    executable = "sift.exe" if args.target.endswith("windows-msvc") else "sift"
    files = {
        executable: binary,
        "LICENSE": (ROOT / "LICENSE").read_bytes(),
        "README.txt": (
            f"Sift {version}\n\nOpen {executable} to start the portable application.\n"
            "See docs/install.md for prerequisites, authentication, and upgrades.\n"
            "See docs/support.md and docs/release-notes.md for preview limitations.\n"
            "RELEASE.json records the source revision and file checksums.\n"
        ).encode(),
    }
    for document in ["install.md", "support.md", "compatibility.md", "release-notes.md",
                     "releasing.md", "release-validation.md"]:
        files[f"docs/{document}"] = (ROOT / "docs" / document).read_bytes()
    metadata = {
        "package_format_version": 1,
        "version": version,
        "target": args.target,
        "git_revision": revision,
        "source_tree_dirty": dirty,
        "files": {path: sha256(data) for path, data in sorted(files.items())},
    }
    files["RELEASE.json"] = (json.dumps(metadata, indent=2, sort_keys=True) + "\n").encode()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    if executable.endswith(".exe"):
        archive = args.output_dir / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for path, data in sorted(files.items()):
                info = zipfile.ZipInfo(f"{name}/{path}")
                info.create_system = 3
                info.external_attr = (0o100755 if path == executable else 0o100644) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                output.writestr(info, data)
    else:
        archive = args.output_dir / f"{name}.tar.gz"
        with archive.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=timestamp) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as output:
                for path, data in sorted(files.items()):
                    info = tarfile.TarInfo(f"{name}/{path}")
                    info.size = len(data)
                    info.mode = 0o755 if path == executable else 0o644
                    info.mtime = timestamp
                    output.addfile(info, io.BytesIO(data))

    checksum = archive.with_name(archive.name + ".sha256")
    checksum.write_text(f"{sha256(archive.read_bytes())}  {archive.name}\n", encoding="utf-8")
    print(archive)
    print(checksum)


if __name__ == "__main__":
    main()
