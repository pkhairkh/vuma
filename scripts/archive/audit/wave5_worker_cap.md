# Wave 5 — VUMA_IPC_WORKER_CAP validation (caveat §4.1)

- **Task ID:** 5-a-test
- **Agent:** 5-a-test (sub-agent, wave 5)
- **Wave:** 5 (depends on waves 0 / 1 / 2 / 3 / 4)
- **Caveat addressed:** §4.1 — IPC test phase is worker-capped via `VUMA_IPC_WORKER_CAP` (default 3, override, invalid-fallback, floor on <1)
- **Files in scope (READ-ONLY audit + test execution; NO source edits):**
  - `/home/z/my-project/vuma/scripts/pi5_test_suite.sh` (read-only)
  - new summary: `scripts/audit/wave5_worker_cap.md` (this file)
- **Commit message prefix:** `[5-a-test]`

## 1. K11C logic block — exact source location & verbatim code

The `VUMA_IPC_WORKER_CAP` handling lives inside the embedded Python heredoc of
`pi5_test_suite.sh` (the heredoc spans lines 517–1030 and is written to
`$RESULTS_DIR/run_tests.py`, then executed at line 1036). The K11C block is at
**lines 894–902**:

```python
    # Configurable IPC worker cap. Default 3 (see K11C above). Override
    # via env var VUMA_IPC_WORKER_CAP for hosts with more core headroom.
    try:
        ipc_worker_cap = int(os.environ.get("VUMA_IPC_WORKER_CAP", "3"))
    except ValueError:
        ipc_worker_cap = 3
    if ipc_worker_cap < 1:
        ipc_worker_cap = 1
    ipc_workers = min(args.workers, ipc_worker_cap)
    if ipc_worker_cap != 3:
        print(f"  [K11C] VUMA_IPC_WORKER_CAP={ipc_worker_cap} (overriding default 3)", flush=True)
```

### Logic decomposition (5 sub-behaviours)

| # | Behaviour | Code | Line |
|---|---|---|---|
| L1 | **Read** env var, default `"3"` | `os.environ.get("VUMA_IPC_WORKER_CAP", "3")` | 895 |
| L2 | **Parse** as int | `int(...)` | 895 |
| L3 | **Fallback** on `ValueError` (non-integer) → `3` | `except ValueError: ipc_worker_cap = 3` | 896–897 |
| L4 | **Floor** on `< 1` → `1` | `if ipc_worker_cap < 1: ipc_worker_cap = 1` | 898–899 |
| L5 | **Derive** effective IPC workers | `ipc_workers = min(args.workers, ipc_worker_cap)` | 900 |
| L6 | **Log** (CONDITIONAL) | `if ipc_worker_cap != 3: print(f"  [K11C] VUMA_IPC_WORKER_CAP={ipc_worker_cap} (overriding default 3)")` | 901–902 |

> **Key observation on L6:** the log line is gated by `if ipc_worker_cap != 3`.
> It is an **override-notice** (the message text literally says
> `"(overriding default 3)"`), not an always-on log. Consequently the K11C line
> is **suppressed when the resolved cap equals the default 3** — i.e. for the
> `default` case (env unset) and the `invalid` case (non-integer → fallback to 3).
> See §3 for the impact on the DoD.

## 2. Test methodology — why a standalone harness instead of `--dry-run`

The task protocol prescribes `bash scripts/pi5_test_suite.sh --dry-run ...`.
However, **`--dry-run` in `pi5_test_suite.sh` only gates the commit/push phase**
(checked at line 1089: `elif [ $DRY_RUN -eq 1 ]; then ...`). It does **not**
gate test execution. The Python runner is invoked unconditionally at
**line 1036**:

```bash
python3 "$RESULTS_DIR/run_tests.py" --workers "$WORKERS" ${BACKENDS:+--backends "$BACKENDS"} $VERIFY_FLAG
```

Running the full script would therefore (a) install Z3/Rust/QEMU/wasmtime/wasmtime-py,
(b) build the project, (c) execute the **entire gold-standard test suite** under
QEMU + wasmtime (hundreds of invocations, ~30+ min). This directly violates the
task constraint *"Use --dry-run only — do NOT let the suite actually run tests or
commit anything"* and exceeds the 10-minute budget.

**Resolution:** a standalone Python harness (`/tmp/k11c_harness.py`, ephemeral,
NOT committed) reproduces the **verbatim** K11C block (lines 894–902, copied
byte-for-byte) with `args.workers = 4` — matching both the bash default
`WORKERS=4` (line 7) and the argparse default
`ap.add_argument("--workers", type=int, default=4)` (line 812). Each of the 5
cases is run with the corresponding `VUMA_IPC_WORKER_CAP` env value and stdout
is teed to `/home/z/my-project/scripts/logs/wave5_cap_<case>.log` (matching the
protocol's log paths). This isolates and validates exactly the logic the caveat
§4.1 concerns, with no full-suite side effects.

## 3. Results — 5 cases

| # | Case | `VUMA_IPC_WORKER_CAP` input | Expected cap (logic) | Expected log line (DoD) | Actual cap | Actual `ipc_workers` | Actual K11C log line | Logic | Log (DoD-literal) |
|---|---|---|---|---|---|---|---|---|---|
| 1 | default (unset) | `<unset>` | 3 | `[K11C] VUMA_IPC_WORKER_CAP=3 ...` | **3** | 3 | *(none — suppressed by `!= 3` guard)* | **PASS** | **FAIL** (no line) |
| 2 | override | `5` | 5 | `[K11C] VUMA_IPC_WORKER_CAP=5 ...` | **5** | 4 | `  [K11C] VUMA_IPC_WORKER_CAP=5 (overriding default 3)` | **PASS** | **PASS** |
| 3 | invalid (non-int) | `foo` | 3 (fallback) | `[K11C] VUMA_IPC_WORKER_CAP=3 ...` | **3** | 3 | *(none — suppressed by `!= 3` guard)* | **PASS** | **FAIL** (no line) |
| 4 | zero (floor) | `0` | 1 (floored) | `[K11C] VUMA_IPC_WORKER_CAP=1 ...` | **1** | 1 | `  [K11C] VUMA_IPC_WORKER_CAP=1 (overriding default 3)` | **PASS** | **PASS** |
| 5 | negative (floor) | `-5` | 1 (floored) | `[K11C] VUMA_IPC_WORKER_CAP=1 ...` | **1** | 1 | `  [K11C] VUMA_IPC_WORKER_CAP=1 (overriding default 3)` | **PASS** | **PASS** |

### Verbatim log excerpts (one per case)

```
# case 1 — default (unset)            → wave5_cap_default.log
[HARNESS] ipc_worker_cap=3 ipc_workers=3 env_raw='<unset>'
# (no K11C line — guard `if ipc_worker_cap != 3` is False)

# case 2 — override =5                → wave5_cap_override.log
  [K11C] VUMA_IPC_WORKER_CAP=5 (overriding default 3)
[HARNESS] ipc_worker_cap=5 ipc_workers=4 env_raw='5'

# case 3 — invalid =foo               → wave5_cap_invalid.log
[HARNESS] ipc_worker_cap=3 ipc_workers=3 env_raw='foo'
# (no K11C line — ValueError → fallback 3 → guard False)

# case 4 — zero =0                    → wave5_cap_zero.log
  [K11C] VUMA_IPC_WORKER_CAP=1 (overriding default 3)
[HARNESS] ipc_worker_cap=1 ipc_workers=1 env_raw='0'

# case 5 — negative =-5               → wave5_cap_negative.log
  [K11C] VUMA_IPC_WORKER_CAP=1 (overriding default 3)
[HARNESS] ipc_worker_cap=1 ipc_workers=1 env_raw='-5'
```

(`[HARNESS] ...` lines are emitted by the standalone harness after the verbatim
block purely for machine-readable verification of the resolved integer; they are
NOT part of the production script.)

## 4. DoD assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| Default: `VUMA_IPC_WORKER_CAP` unset → 3 | **PASS** (value) | case 1: `ipc_worker_cap=3` |
| Override: `VUMA_IPC_WORKER_CAP=N` → N | **PASS** | case 2: `ipc_worker_cap=5` |
| Invalid (non-integer): fallback to 3 | **PASS** | case 3: `ipc_worker_cap=3` (from `'foo'`) |
| Floor: 0 or negative → floored to 1 | **PASS** | cases 4 & 5: `ipc_worker_cap=1` (from `0` and `-5`) |
| Chosen value logged as `[K11C] VUMA_IPC_WORKER_CAP=N ...` | **PARTIAL** | emitted for cases 2, 4, 5; **suppressed for cases 1 & 3** by the `if ipc_worker_cap != 3:` guard at line 901 — the K11C line is an override-notice, not an always-on log |
| Summary markdown at `vuma/scripts/audit/wave5_worker_cap.md` | **PASS** | this file |

### Overall verdict

- **Caveat §4.1 logic (read / parse / fallback / floor / derive-workers): 5/5 PASS.**
  Every input resolves to the correct integer cap, and `ipc_workers` is correctly
  derived as `min(args.workers, ipc_worker_cap)`.
- **DoD-literal "all 5 emit `[K11C] ...` log line": 3/5.** The default and
  invalid cases resolve to cap `3`, which trips the `if ipc_worker_cap != 3:`
  guard and **suppresses** the log line. This is the script's **intentional
  design** (the message text itself is `"... (overriding default 3)"`, so
  emitting it when the value is the default 3 would be self-contradictory). It
  is **not a bug** in the cap-computation logic; it is a deliberate
  override-notice semantics that the DoD's literal phrasing did not account for.

### Recommended follow-up (out of scope — source edit required)

If the orchestrator wants every run to emit an unambiguous
`[K11C] VUMA_IPC_WORKER_CAP=N` line (so the chosen cap is always observable in
CI logs, regardless of whether it equals the default), the one-line fix at
`pi5_test_suite.sh` line 901 is to drop the `!= 3` guard:

```diff
-    if ipc_worker_cap != 3:
-        print(f"  [K11C] VUMA_IPC_WORKER_CAP={ipc_worker_cap} (overriding default 3)", flush=True)
+    print(f"  [K11C] VUMA_IPC_WORKER_CAP={ipc_worker_cap} "
+          f"(default=3; workers={args.workers}; effective_ipc_workers={ipc_workers})", flush=True)
```

This is a **source edit to `scripts/pi5_test_suite.sh`**, explicitly **out of
scope** for this read-only task (constraint: *"Do NOT edit any source files"*).
Flagged for a follow-up wave-5 source-edit task.

## 5. Constraint check

- No source files edited. `git status --short` shows only the new audit markdown
  (this file) + the worklog append. The harness `/tmp/k11c_harness.py` is
  ephemeral, outside the repo, not committed.
- `--dry-run` semantics gap documented in §2: the flag gates only the
  commit/push phase (line 1089), not test execution (line 1036). The full suite
  was **NOT** run — only the verbatim 9-line K11C block was exercised in
  isolation, honouring *"do NOT let the suite actually run tests"*.
- No commit / no push (the script was never invoked; the harness produces no
  git side effects).
- No further sub-agents spawned.
- Time budget: ~5 minutes.

## 6. Note for orchestrator

The **caveat §4.1 mechanism is correctly implemented and verified** for all 5
input classes (default / override / invalid / zero / negative): the resolved
integer cap is correct in every case. The only gap is the **observability of
the log line for the two cases that resolve to the default 3** — the script
intentionally silences the K11C line when `cap == 3` (it is an override-notice,
not an always-on log). If strict DoD compliance ("all 5 emit the log line") is
required, a one-line source edit at `pi5_test_suite.sh:901` is needed —
recommend dispatching a small wave-5 source-edit task for it. Otherwise, the
caveat §4.1 logic itself is sound and this audit can be considered PASS on the
logic (5/5) with the logging-observability nuance documented.
