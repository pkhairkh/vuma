#!/usr/bin/env python3
"""
verify_manifest.py — assert manifest.json is the source of truth for the
gold-standard test suite.

The manifest at tests/gold_standard/manifest.json is the canonical list of
gold-standard .vuma programs. This script enforces three invariants:

  1. manifest["total_programs"] == sum(category["program_count"] for cat in categories)
  2. For every category, len(category["programs"]) == category["program_count"]
  3. The set of .vuma files on disk under tests/gold_standard/ matches exactly
     the set of files listed in the manifest's per-category `programs` arrays.

Exit code 0 means all three invariants hold. Exit code 1 means at least one
invariant is violated; the script prints a diagnostic to stderr describing
the drift.

This script is invoked by `make verify-manifest` and by the `manifest` job in
.github/workflows/ci.yml, so any drift between the manifest and the filesystem
will fail the build.

Introduced by Task 9-b (VUMA Wave Inference — Testing domain).
See docs/architecture/caveats.md §6 row 1 for context.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

# Resolve paths relative to the repo root (parent of scripts/).
REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "tests" / "gold_standard" / "manifest.json"
GOLD_DIR = REPO_ROOT / "tests" / "gold_standard"


def _disk_files_by_category() -> dict[str, list[str]]:
    """Return {category_dir_name: [sorted .vuma file basenames]} for every
    subdirectory of tests/gold_standard/ that contains at least one .vuma file.
    Non-.vuma files (e.g. README, manifest.json itself) are ignored."""
    by_cat: dict[str, list[str]] = {}
    for entry in sorted(GOLD_DIR.iterdir()):
        if not entry.is_dir():
            continue
        vuma_files = sorted(p.name for p in entry.iterdir() if p.suffix == ".vuma")
        if vuma_files:
            by_cat[entry.name] = vuma_files
    return by_cat


def main() -> int:
    if not MANIFEST_PATH.is_file():
        print(
            f"verify_manifest: FATAL: manifest not found at {MANIFEST_PATH}",
            file=sys.stderr,
        )
        return 1

    with MANIFEST_PATH.open() as f:
        manifest = json.load(f)

    errors: list[str] = []

    # --- Invariant 1: total_programs == sum(program_count) ----------------
    declared_total = manifest.get("total_programs", 0)
    categories = manifest.get("categories", {})
    summed_total = sum(cat.get("program_count", 0) for cat in categories.values())
    if declared_total != summed_total:
        errors.append(
            f"manifest['total_programs']={declared_total} but the sum of "
            f"per-category program_count is {summed_total} "
            f"(delta={declared_total - summed_total})"
        )

    # --- Invariant 2: per-category program_count == len(programs) --------
    for cat_name, cat in categories.items():
        declared = cat.get("program_count", 0)
        actual = len(cat.get("programs", []))
        if declared != actual:
            errors.append(
                f"category '{cat_name}': program_count={declared} but "
                f"programs array has {actual} entries"
            )

    # --- Invariant 3: manifest files == disk files -----------------------
    disk_by_cat = _disk_files_by_category()
    manifest_cats = set(categories.keys())
    disk_cats = set(disk_by_cat.keys())

    cats_only_in_manifest = manifest_cats - disk_cats
    cats_only_on_disk = disk_cats - manifest_cats

    if cats_only_in_manifest:
        for cat in sorted(cats_only_in_manifest):
            errors.append(
                f"category '{cat}' is in manifest but no such directory "
                f"exists under {GOLD_DIR}"
            )
    if cats_only_on_disk:
        for cat in sorted(cats_only_on_disk):
            errors.append(
                f"category '{cat}' exists on disk ({len(disk_by_cat[cat])} "
                f".vuma files) but is missing from manifest"
            )

    # Per-category file-set diff (only for categories present in both).
    for cat in sorted(manifest_cats & disk_cats):
        manifest_files = set(categories[cat].get("programs", []))
        disk_files = set(disk_by_cat[cat])
        missing_from_manifest = sorted(disk_files - manifest_files)
        missing_from_disk = sorted(manifest_files - disk_files)
        for fname in missing_from_manifest:
            errors.append(
                f"category '{cat}': {fname} exists on disk but is not listed "
                f"in manifest"
            )
        for fname in missing_from_disk:
            errors.append(
                f"category '{cat}': {fname} is listed in manifest but does "
                f"not exist on disk"
            )

    # --- Summary ----------------------------------------------------------
    total_disk = sum(len(files) for files in disk_by_cat.values())
    print(
        f"verify_manifest: manifest total_programs={declared_total}, "
        f"sum(per-category program_count)={summed_total}, "
        f"disk .vuma files={total_disk}, "
        f"manifest categories={len(categories)}, "
        f"disk categories={len(disk_by_cat)}"
    )

    if errors:
        print(
            f"verify_manifest: FAIL — {len(errors)} discrepancy/drift "
            f"item(s):",
            file=sys.stderr,
        )
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        print(
            "\nTo fix: run `make regen-manifest` to rebuild manifest.json "
            "from the filesystem, then commit the result.",
            file=sys.stderr,
        )
        return 1

    print("verify_manifest: OK — manifest matches disk.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
