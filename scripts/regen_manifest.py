#!/usr/bin/env python3
"""
regen_manifest.py — rebuild tests/gold_standard/manifest.json from the
filesystem.

Walks tests/gold_standard/ and rebuilds the per-category `programs` arrays
and `program_count` fields from the .vuma files actually on disk, then
rewrites `total_programs` to match. The top-level metadata fields
(`schema_version`, `suite`, `description`, `source_dir`) are preserved
unchanged. Per-category `title` fields are preserved if present, otherwise
default to the category key.

Use this after adding or removing .vuma files to bring the manifest back in
sync with the filesystem. Always run `make verify-manifest` afterwards to
confirm the reconciliation.

This script is invoked by `make regen-manifest`. It is intentionally
non-destructive: it refuses to overwrite the manifest unless
`--write` is passed (default is a dry-run that prints the proposed delta).

Introduced by Task 9-b (VUMA Wave Inference — Testing domain).
See docs/architecture/caveats.md §6 row 1 for context.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "tests" / "gold_standard" / "manifest.json"
GOLD_DIR = REPO_ROOT / "tests" / "gold_standard"


def _disk_files_by_category() -> dict[str, list[str]]:
    by_cat: dict[str, list[str]] = {}
    for entry in sorted(GOLD_DIR.iterdir()):
        if not entry.is_dir():
            continue
        vuma_files = sorted(p.name for p in entry.iterdir() if p.suffix == ".vuma")
        if vuma_files:
            by_cat[entry.name] = vuma_files
    return by_cat


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--write",
        action="store_true",
        help="Write the regenerated manifest to disk (default: dry-run).",
    )
    args = ap.parse_args()

    if not MANIFEST_PATH.is_file():
        print(f"regen_manifest: FATAL: {MANIFEST_PATH} not found", file=sys.stderr)
        return 1

    with MANIFEST_PATH.open() as f:
        manifest = json.load(f)

    disk_by_cat = _disk_files_by_category()
    categories = manifest.setdefault("categories", {})

    # Preserve existing titles; default to the category key.
    new_categories: dict[str, dict] = {}
    for cat_name in sorted(disk_by_cat.keys()):
        files = disk_by_cat[cat_name]
        old = categories.get(cat_name, {})
        title = old.get("title", cat_name)
        new_categories[cat_name] = {
            "title": title,
            "program_count": len(files),
            "programs": list(files),
        }

    # Detect categories that were present in the manifest but have no .vuma
    # files on disk — these would be silently dropped by the regen.
    dropped = sorted(set(categories.keys()) - set(new_categories.keys()))
    if dropped:
        print(
            "regen_manifest: WARNING — the following manifest categories have "
            "no .vuma files on disk and will be dropped:",
            file=sys.stderr,
        )
        for c in dropped:
            print(f"  - {c}", file=sys.stderr)

    old_total = manifest.get("total_programs", 0)
    new_total = sum(c["program_count"] for c in new_categories.values())

    print(
        f"regen_manifest: total_programs {old_total} -> {new_total} "
        f"(delta={new_total - old_total}); "
        f"categories {len(categories)} -> {len(new_categories)}"
    )

    manifest["categories"] = new_categories
    manifest["total_programs"] = new_total

    if args.write:
        with MANIFEST_PATH.open("w") as f:
            json.dump(manifest, f, indent=2)
            f.write("\n")
        print(f"regen_manifest: wrote {MANIFEST_PATH}")
        print("regen_manifest: run `make verify-manifest` to confirm.")
    else:
        print("regen_manifest: dry-run only — pass --write to commit changes.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
