#!/usr/bin/env python3
"""Verify an archive, run a safe headless import, and confirm a visible desktop window."""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import time
import tomllib
import zipfile


def unpack(archive, destination):
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            for entry in source.infolist():
                path = (destination / entry.filename).resolve()
                if not path.is_relative_to(destination.resolve()):
                    raise ValueError("archive contains a path outside its root")
            source.extractall(destination)
    else:
        with tarfile.open(archive, "r:gz") as source:
            source.extractall(destination, filter="data")


def visible_window(pid):
    if os.name != "nt":
        return subprocess.run(
            ["xwininfo", "-name", "sift"], capture_output=True, check=False
        ).returncode == 0
    user32 = ctypes.windll.user32
    found = False
    callback_type = ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)

    def visit(handle, _):
        nonlocal found
        window_pid = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(ctypes.c_void_p(handle), ctypes.byref(window_pid))
        if window_pid.value == pid and user32.IsWindowVisible(ctypes.c_void_p(handle)):
            found = True
        return True

    user32.EnumWindows(callback_type(visit), 0)
    return found


def smoke_gui(binary, env, duration):
    # Capture to a file: a blocked pipe must not make a GUI appear to be healthy.
    with tempfile.TemporaryFile() as log:
        process = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + duration
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"desktop exited before a window appeared (exit {process.returncode})")
                if visible_window(process.pid):
                    time.sleep(2)
                    if process.poll() is not None:
                        raise RuntimeError("desktop exited immediately after opening a window")
                    print("Desktop window opened and remained open for the smoke check.")
                    return
                time.sleep(0.25)
            raise RuntimeError(f"no visible desktop window appeared within {duration:g} seconds")
        except Exception:
            log.seek(0)
            print(log.read().decode(errors="replace"))
            raise
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--headless-only", action="store_true", help="Skip the GUI check on hosts without a desktop")
    parser.add_argument("--timeout", type=float, default=30)
    args = parser.parse_args()
    archive = args.archive.resolve()
    checksum = archive.with_name(archive.name + ".sha256").read_text().split()
    if len(checksum) != 2 or checksum[1] != archive.name:
        raise ValueError("invalid SHA-256 sidecar")
    if hashlib.sha256(archive.read_bytes()).hexdigest() != checksum[0]:
        raise ValueError("archive checksum mismatch")

    with tempfile.TemporaryDirectory(prefix="sift-release-smoke-") as directory:
        sandbox = Path(directory)
        unpack(archive, sandbox)
        manifests = list(sandbox.glob("*/RELEASE.json"))
        if len(manifests) != 1:
            raise ValueError("package must contain exactly one release manifest")
        manifest_path = manifests[0]
        manifest = json.loads(manifest_path.read_text())
        for name, digest in manifest["files"].items():
            path = (manifest_path.parent / name).resolve()
            if not path.is_relative_to(manifest_path.parent.resolve()) or not path.is_file():
                raise ValueError(f"invalid package member {name!r}")
            if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                raise ValueError(f"package member checksum mismatch: {name}")
        target = manifest["target"]
        windows_package = target == "x86_64-pc-windows-msvc"
        if windows_package != (os.name == "nt"):
            raise ValueError(f"package target {target} does not match this host")
        binary = manifest_path.parent / ("sift.exe" if windows_package else "sift")

        env = os.environ.copy()
        config_dir = sandbox / "config"
        data_dir = sandbox / "data"
        config_dir.mkdir()
        data_dir.mkdir()
        runtime_dir = sandbox / "runtime"
        runtime_dir.mkdir(mode=0o700)
        # Isolate saved profiles so this check never uses the operator's namespaces.
        env.update({"SIFT_CONFIG_DIR": str(config_dir),
                    "XDG_CONFIG_HOME": str(config_dir), "XDG_DATA_HOME": str(data_dir),
                    "XDG_RUNTIME_DIR": str(runtime_dir),
                    "APPDATA": str(config_dir), "LOCALAPPDATA": str(data_dir)})
        if os.name != "nt":
            # Window verification uses X11; winit otherwise prefers an inherited
            # Wayland session whose socket is outside this isolated runtime dir.
            env.pop("WAYLAND_DISPLAY", None)
            env.pop("WAYLAND_SOCKET", None)
        legacy = sandbox / "empty.config"
        legacy.write_text('<configuration><serviceBusNamespaces><add key="smoke" value="invalid"/></serviceBusNamespaces></configuration>')
        result = subprocess.run([str(binary), "--import-legacy", str(legacy)],
                                env=env, capture_output=True, timeout=args.timeout, check=False)
        if result.returncode != 0:
            raise RuntimeError(f"headless startup failed (exit {result.returncode}): "
                               + result.stderr.decode(errors="replace"))
        saved_configs = list(config_dir.rglob("config.toml"))
        if len(saved_configs) != 1:
            raise RuntimeError("headless import did not create its isolated configuration file")
        config = tomllib.loads(saved_configs[0].read_text())
        if config.get("profiles") or config.get("schema_version") != 1:
            raise RuntimeError("safe import fixture did not produce the expected empty profile configuration")
        print(f"Archive verified; packaged {manifest['version']} binary launched and saved configuration.")
        if not args.headless_only:
            smoke_gui(binary, env, args.timeout)


if __name__ == "__main__":
    main()
