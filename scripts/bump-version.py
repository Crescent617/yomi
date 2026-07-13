#!/usr/bin/env python3
"""Bump version across Cargo workspace, Tauri config, and package.json files."""

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

FILES = {
    "cargo": ROOT / "Cargo.toml",
    "tauri": ROOT / "crates" / "gui" / "tauri.conf.json",
    "gui_pkg": ROOT / "crates" / "gui" / "package.json",
    "gui_lock": ROOT / "crates" / "gui" / "package-lock.json",
    "frontend_pkg": ROOT / "crates" / "gui" / "frontend" / "package.json",
    "frontend_lock": ROOT / "crates" / "gui" / "frontend" / "package-lock.json",
}


def read_version():
    cargo = FILES["cargo"].read_text()
    m = re.search(r'^version\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"', cargo, re.M)
    if not m:
        print("Could not find version in Cargo.toml", file=sys.stderr)
        sys.exit(1)
    return m.group(1)


def bump(v: str, level: str) -> str:
    parts = list(map(int, v.split(".")))
    if level == "major":
        parts[0] += 1
        parts[1] = parts[2] = 0
    elif level == "minor":
        parts[1] += 1
        parts[2] = 0
    else:
        parts[2] += 1
    return ".".join(map(str, parts))


def set_cargo_version(path: Path, new: str):
    text = path.read_text()
    text = re.sub(
        r'^version\s*=\s*"[0-9]+\.[0-9]+\.[0-9]+"',
        f'version = "{new}"',
        text,
        count=1,
        flags=re.M,
    )
    path.write_text(text)
    print(f"  {path.relative_to(ROOT)} → {new}")


def set_json_version(path: Path, new: str):
    text = path.read_text()
    text = re.sub(r'"version":\s*"[0-9]+\.[0-9]+\.[0-9]+"', f'"version": "{new}"', text, count=1)
    path.write_text(text)
    print(f"  {path.relative_to(ROOT)} → {new}")


def set_package_lock_version(path: Path, new: str):
    text = path.read_text()
    pattern = r'("version":\s*)"[0-9]+\.[0-9]+\.[0-9]+"'
    text, replacements = re.subn(pattern, rf'\g<1>"{new}"', text, count=2)
    if replacements != 2:
        raise ValueError(f"Expected two project version fields in {path}")
    path.write_text(text)
    print(f"  {path.relative_to(ROOT)} → {new}")


def main():
    parser = argparse.ArgumentParser(description="Bump or set project version")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--level", choices=["patch", "minor", "major"], help="bump level")
    group.add_argument("--set", metavar="VERSION", help="set exact version (e.g. 0.2.24)")
    args = parser.parse_args()

    current = read_version()
    new = args.set if args.set else bump(current, args.level)

    print(f"Bumping {current} → {new}")
    set_cargo_version(FILES["cargo"], new)
    set_json_version(FILES["tauri"], new)
    set_json_version(FILES["gui_pkg"], new)
    set_package_lock_version(FILES["gui_lock"], new)
    set_json_version(FILES["frontend_pkg"], new)
    set_package_lock_version(FILES["frontend_lock"], new)
    print("Done.")


if __name__ == "__main__":
    main()
