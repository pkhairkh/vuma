# test_results/ — Pi Test Suite Output

This directory is the **exclusive territory of the Pi test runner**
(`scripts/pi5_test_suite.sh`). It is **NOT gitignored** — the Pi commits
its run results here and pushes to `origin/main` after every suite run.

## Pi isolation convention

| Who    | May modify `test_results/`? | May modify other paths? |
|--------|----------------------------|------------------------|
| **Pi** | YES (writes + commits)     | NO (never `git add` outside `test_results/`) |
| **Agents** | NO (never touch)        | YES (`src/`, `tests/gold_standard/`, `scripts/`, `docs/`, `womb/`, root) |

This strict file-disjoint partition guarantees the Pi can always
`git pull && git push` without merge conflicts: agent commits never
touch `test_results/`, and Pi commits never touch anything else.

## Files written by the Pi

| File | Description |
|------|-------------|
| `summary.json` | Aggregate pass/fail counts per backend, timestamp, host info |
| `failures.txt` | List of failing test programs with expected vs actual exit codes |
| `checkpoint.jsonl` | Per-test JSONL records (large; enables resume-after-crash) |
| `build.log` | Compiler build output (for diagnosing build failures) |
| `run_tests.py` | Generated Python harness (inline in `pi5_test_suite.sh`, written here at runtime) |

## Historical note

During the 2026-07 cleanup, this directory was briefly gitignored because
a stale 39 MB `checkpoint.jsonl` from a different host (`pi-pkhairkh-dev`
at `/home/pkhairkh/vuma`) had been committed and was misleading (it
reported 99.99% pass with loongarch64/mips64 at 100%, contradicting the
repo's own `differential_results.txt`). That artifact was deleted and the
directory was un-gitignored so the Pi's own canonical run records can be
committed.
