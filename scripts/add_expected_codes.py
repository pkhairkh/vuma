#!/usr/bin/env python3
"""
Add 'Expected exit code' headers to .vuma files that don't have them.
Uses the fixed compiler's output as the expected value for files that
run cleanly (no crash/timeout) on x86_64.

Only adds headers to files that:
1. Don't already have an 'Expected exit code' header
2. Compile successfully
3. Run cleanly (exit code < 128, not 124/timeout)
4. Have deterministic behavior (no allocate/free — those may vary)
"""

import os
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path("/home/z/my-project/vuma")
COMPILE_DUMP = REPO_ROOT / "target/release/compile_dump"
GOLD_DIR = REPO_ROOT / "tests/gold_standard"
EXPECTED_RE = re.compile(r"^//\s*[Ee]xpected\s+exit\s+code\s*:\s*(-?\d+)", re.MULTILINE)

def has_expected_header(text):
    return EXPECTED_RE.search(text) is not None

def has_allocate(text):
    # Skip files with allocate/free — non-deterministic exit codes
    return "allocate" in text or "free(" in text or "free (" in text

def run_program(vuma_file, backend="x86_64"):
    out_bin = f"/tmp/add_expected_{os.getpid()}.bin"
    try:
        subprocess.run(
            [str(COMPILE_DUMP), str(vuma_file), out_bin, backend],
            capture_output=True, timeout=10
        )
        if not os.path.exists(out_bin) or os.path.getsize(out_bin) == 0:
            return None
        os.chmod(out_bin, 0o755)
        cp = subprocess.run([out_bin], capture_output=True, timeout=2)
        code = cp.returncode
        if code < 0:
            code = 128 + (-code)
        if code >= 128 or code == 124:
            return None  # crash or timeout
        return code
    except:
        return None
    finally:
        if os.path.exists(out_bin):
            os.unlink(out_bin)

def main():
    files = sorted(GOLD_DIR.rglob("*.vuma"))
    print(f"Total files: {len(files)}")

    added = 0
    skipped_has_header = 0
    skipped_has_allocate = 0
    skipped_crash = 0

    for i, f in enumerate(files):
        text = f.read_text(errors="replace")
        if has_expected_header(text):
            skipped_has_header += 1
            continue
        if has_allocate(text):
            skipped_has_allocate += 1
            continue

        code = run_program(f)
        if code is None:
            skipped_crash += 1
            continue

        # Add "Expected exit code: N" header after the first comment block
        lines = text.split("\n")
        insert_pos = 0
        for j, line in enumerate(lines):
            if line.startswith("//"):
                insert_pos = j + 1
            else:
                break

        # Find the last comment line that's part of the first block
        # Insert after it
        header_line = f"// Expected exit code: {code}"
        lines.insert(insert_pos, header_line)
        f.write_text("\n".join(lines))
        added += 1

        if (i + 1) % 500 == 0:
            print(f"  {i+1}/{len(files)} processed, {added} headers added", flush=True)

    print(f"\nResults:")
    print(f"  Already had header: {skipped_has_header}")
    print(f"  Skipped (has allocate): {skipped_has_allocate}")
    print(f"  Skipped (crash/timeout): {skipped_crash}")
    print(f"  Headers added: {added}")

if __name__ == "__main__":
    main()
