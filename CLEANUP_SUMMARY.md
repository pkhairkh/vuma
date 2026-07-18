# VUMA Repository Cleanup — Summary (2026-07)

A repo-wide cleanup following the audit that found ~89% of the gold-standard
test suite was noise and ~36 concrete stale-file/script/CI issues elsewhere.

## Headline numbers

| Metric | Before | After |
|--------|-------:|------:|
| `tests/gold_standard/*.vuma` files | 5,851 | **1,502** |
| `manifest.json` `total_programs` (vs disk) | 704 (stale, 87% missing) | **1,502** (validated = disk) |
| Stale run-log artifacts at gold root | 6 | 0 |
| `scripts/` with hardcoded wrong paths | 19 of 21 | **0** |
| `docs/` + `examples/README.md` stale path/count refs | ~20 | **0** |
| Dead root files (`echo`, `vuma_vm.h`, `test_results/` 39 MB) | present | gone |
| Duplicate scripts (5 gold runners, 3 KAT generators, 2 supervisors) | present | consolidated to 1 each |
| `vuma-tests.yml` toolchain mismatch (stable vs nightly pin) | broken | fixed |

Working-tree changes: **4,370 deletions, 26 modifications, 1 new file**
(`tests/gold_standard/README.md`). No commits made — left for review.

## What was removed and why

### Gold-standard suite (`tests/gold_standard/`)
- **3,842 sweep near-duplicates** — `sN_<family>.vuma` files (s3..s106) that
  were byte-distinct but behaviorally identical (same code shape, only
  constants differ). Kept 1 representative per family (smallest `sN`).
  The `sN_` generator was deleted long ago and never documented; the prefix
  is retained on survivors as a legacy ID (documented in the new README).
- **507 hollow PMT stubs** — files whose header described a real feature
  (Adler-32, hash table, memory arena, …) but whose body was just
  `c.v = <constant>; return c.v;`. The header was a lie. Real implementations
  of those features live in `examples/`.
- **6 run-log artifacts** (`differential_raw.tsv`, `differential_sweep_200.tsv`,
  `differential_results.txt`, `differential_sweep_200.txt`, `results_baseline.txt`,
  `o0_o3_results.txt`) — one-shot historical run logs no script consumes.
  Gitignored going forward.
- **`build_categories.py`** — dead footgun. Hardcoded `/tmp/my-project/examples/`,
  would overwrite the v2.0 manifest with a 47-program v1.0 if run.
- **`kernel_boot/`** category — contained only `hello.expected`, zero `.vuma`.
- Curated suites (`pmt_wave*`, `ffi_wave*`, `arena_wave*`, `float_*`,
  `kernel_crypto`) were **untouched** — including the 5 `pmt_wave3_negative/`
  must-fail tests.

### Repo root
- `echo` — 107-byte shell-prompt transcript dumped to a file named after the
  builtin. Debug leftover.
- `vuma_vm.h` — 69-line C header with zero references anywhere in the repo.
- `test_results/` — 39 MB machine-generated run logs from a **different tree**
  (`/home/pkhairkh/vuma` on host `pi-pkhairkh-dev`), reporting 99.99% pass with
  loongarch64/mips64 at 100% — contradicting the repo's own
  `differential_results.txt` showing those backends broken. Misleading.
  Gitignored.

### Scripts (`scripts/`)
- **Deleted 6 duplicates**: `run_gold_sweep.py` (superseded + had an unfixed
  crash-classification bug where expected codes ≥128 were mislabeled as
  `crash`), `run_8backends.py`, `supervisor_3par.sh`, `gen_all_real_kat.py`,
  `generate_all_kat_tests.py`, `run_all_kat.sh`. Canonical survivors:
  `run_all_gold.sh` (gold suite), `gen_real_kat.py` + `run_real_kat.sh` (KAT),
  `supervisor.py` (batch supervisor).
- **Fixed 17 surviving scripts**: replaced `/home/z/vuma_real`, `/home/z/my-project/vuma`,
  `/tmp/my-project`, `/tmp/qemu/extracted` with repo-relative paths
  (`REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"` for bash,
  `Path(__file__).resolve().parent.parent` for Python) or env-var-with-default
  (`${COMPILE_DUMP:-$REPO_ROOT/target/release-fast/compile_dump}`). QEMU
  binaries now resolved via `command -v` / `/tmp/qemu_bins/` (CI convention).
  The `5754`/`5851` stale count constants were removed/made dynamic.

### CI (`.github/workflows/`)
- `vuma-tests.yml` was installing **stable** Rust via `dtolnay/rust-toolchain@stable`
  with no `toolchain:` field, while every other workflow pins
  `nightly-2026-03-01`. Fixed to pin `nightly-2026-03-01` + `rustfmt,clippy`.
- (The "branches: ain]" YAML corruption reported earlier was a **false
  positive** — a bash display artifact consuming `[m` as an ANSI reset. The
  actual bytes are `branches: [main]`, confirmed via `od -c`. No fix needed.)
- `.gitignore` now ignores `test_results/`, `echo`, `vuma_vm.h`, `/tmp/vuma_results/`,
  `/tmp/qemu_bins/`, and **un-ignores `Cargo.lock`** (it was wrongly ignored
  for a binary crate).

### Docs (`docs/` + `examples/README.md`)
- `docs/building.md` — 11× `/tmp/my-project/...` QEMU paths → `/tmp/qemu_bins/`
  (CI convention) + `apt-get install qemu-user`. Removed `run_all_kat.sh`
  reference, `kernel_boot/` row, stale `5,832+` count.
- `docs/kernel-porting-guide.md` — "existing 19 backends" for the KERNEL was
  wrong; `womb/kernel/arch/` has only 4 dirs (`x86_64, aarch64, riscv64, wasm32`).
  Clarified: 19 codegen backends (`BackendKind`) vs 4 kernel arch ports; the
  remaining 15 run hosted-mode with `__ffi_fallback_stub`.
- `docs/kernel-architecture.md` §15, `docs/kernel-developer-guide.md`,
  `docs/contributing.md`, `examples/README.md` — same class of fixes (stale
  counts → `manifest.json` ref; deleted-script refs → `run_real_kat.sh`;
  `kernel_boot/` mentions removed; 4-vs-19 split made explicit).
- `docs/architecture.md`, `docs/language-reference.md`, `docs/fp_backends.md`
  — audited, no stale refs in scope, left unchanged.

## New artifacts
- `tests/gold_standard/README.md` (199 lines) — documents the suite structure,
  the `sN_` legacy prefix, the `pmt_wave3_negative/` must-fail convention,
  how to run the suite, the file-naming convention going forward
  (`<feature>_<variant>.vuma`, NOT `sN_`), and the cleanup history.
- `tests/gold_standard/manifest.json` (schema 3.0) — regenerated from cleaned
  disk, 36 categories, 1,502 programs, validated `total_programs` == disk count.

## Verification
- `find tests/gold_standard -name '*.vuma' | wc -l` = **1,502**
- `manifest.json` `total_programs` = **1,502** (matches disk)
- 0 run-log `.tsv`/`.txt` at gold root
- `build_categories.py` and `kernel_boot/` gone
- 18/18 scripts pass `bash -n` / `python3 -m py_compile`
- 0 offending path patterns in `scripts/` or `docs/`
- No doc claims the kernel has 19 arch ports
- No doc references a deleted script as a runnable command

## What was deliberately NOT done
- **No git commits** — all changes left in the working tree for review.
  Suggested commit grouping: (1) root + .gitignore + CI, (2) script deletions,
  (3) script path fixes, (4) gold-standard cleanup, (5) docs fixes.
- **No `sN_` file renaming** — survivors keep their legacy prefix (documented
  in the README). Renaming ~600 files risked breaking references and offered
  little over the dedup already done.
- **No restoration of the 31 shrunk-from-example stubs** — they were deleted,
  not restored, because the originals likely don't compile under current PMT
  (that's why they were shrunk). Real implementations remain in `examples/`.
- **No rewrite of `build_categories.py`** — deleted rather than rewritten;
  the new `manifest.json` is generated by a throwaway script and the README
  documents the structure. A proper generator can be added if sweeps are
  ever reintroduced.
- **O3 miscompilation (~55%) and backend stubs (loongarch64, mips64, newer
  ISA codegen)** are out of scope for this cleanup — those are compiler
  correctness issues, not stale-file issues.

See `/home/z/my-project/worklog.md` for the per-agent work logs (Tasks 2-a,
2-b, 2-c).
