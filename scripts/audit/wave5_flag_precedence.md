# Wave 5-b-test — `--commit` / `--dry-run` / `--no-push` flag precedence audit

- **Task ID:** 5-b-test
- **Agent:** 5-b-test (sub-agent, wave 5)
- **Wave:** 5 (depends on waves 0 / 1 / 2 / 3 / 4 / 5-a-test)
- **Caveat addressed:** §4.4 — `--commit` is opt-in; flag precedence `--no-push > --dry-run > --commit > default-off`
- **Files in scope (READ-ONLY audit; NO source edits):** `scripts/pi5_test_suite.sh` (ro), new `scripts/audit/wave5_flag_precedence.md`
- **DoD:** All 5 flag combinations produce the expected `WILL_COMMIT` / `WILL_PUSH` behaviour; precedence matches `--no-push > --dry-run > --commit > default-off`; summary markdown exists.

---

## 1. Source location (verbatim)

**File:** `scripts/pi5_test_suite.sh`

### 1a. Flag defaults + argument parser — lines 7–36

```bash
WORKERS=4
SKIP_BUILD=0
NO_PUSH=0
COMMIT=0              # --commit: opt-in to auto-commit + push (default OFF; see caveats §6 row 9)
DRY_RUN=0             # --dry-run: show what would be committed without committing
...
while [[ $# -gt 0 ]]; do
    case $1 in
        --workers) WORKERS="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --no-push) NO_PUSH=1; shift ;;
        --commit) COMMIT=1; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        ...
    esac
done
```

### 1b. Commit/push decision chain — lines 1084–1186

```bash
if [ $NO_PUSH -eq 1 ]; then
    echo "▸ Skipping commit/push (--no-push)"           # NO_PUSH branch
    ...
elif [ $DRY_RUN -eq 1 ]; then
    echo "▸ Dry run (--dry-run): ..."                    # DRY_RUN branch
    ...  # prints staged files + sizes + proposed msg + git status --porcelain
elif [ $COMMIT -eq 1 ]; then
    ...  # git add -f ... ; git commit ... ; git push origin HEAD
else
    echo "▸ Auto-commit is OFF ..."                      # default-off branch
    ...  # prints staged files + sizes + proposed msg + manual instructions
fi
```

The **elif chain order** is the precedence: `NO_PUSH → DRY_RUN → COMMIT → else(default-off)`.

---

## 2. Precedence verification

| Caveat §4.4 precedence rank | Flag | Script elif position | Branch effect |
|---|---|---|---|
| 1 (highest) | `--no-push` | 1st (`if NO_PUSH`) | skip commit **and** push |
| 2 | `--dry-run` | 2nd (`elif DRY_RUN`) | preview only; no commit, no push |
| 3 | `--commit` | 3rd (`elif COMMIT`) | commit + push |
| 4 (lowest) | default-off | `else` | preview only; no commit, no push |

**Precedence chain order: PASS** — the elif order in the script is exactly
`--no-push > --dry-run > --commit > default-off`, matching caveat §4.4.

---

## 3. Methodology

Per the protocol, `pi5_test_suite.sh` was **NOT** invoked with `--commit`
(that would create real git commits + a push attempt of `test_results/`
artifacts). Instead, the flag-parsing block (L7–36) and the commit/push
decision chain (L1084–1186) were extracted **verbatim** into a standalone
harness at `/home/z/wave5b_tmp/flag_harness.sh` (ephemeral, not committed —
the Write tool is sandboxed to `/home/z`). The harness accepts the same
flags, resolves the same elif chain, and prints:

- `WILL_COMMIT=<0|1>`
- `WILL_PUSH=<0|1>`
- `WILL_RUN_TESTS=<0|1>` (always 1 for these 5 cases — test execution at L1036
  is not gated by `--commit`/`--dry-run`/`--no-push`; only `--trend` early-exits
  before tests, and `--trend` is out of scope)

The harness invokes **no** git commands and runs **no** tests — it only
resolves the decision matrix. Per-case logs saved at
`/home/z/my-project/scripts/logs/wave5b_flag_{default,dry_run,commit,commit_no_push,commit_dry_run}.log`.

---

## 4. Five-case results

| # | Flags | Expected (caveat §4.4 matrix) | Actual (harness) | WILL_COMMIT match | WILL_PUSH match | Verdict |
|---|---|---|---|---|---|---|
| 1 | *(default, no flags)* | commit=0, push=0, preview | `WILL_COMMIT=0 WILL_PUSH=0 WILL_RUN_TESTS=1` | ✓ | ✓ | **PASS** |
| 2 | `--dry-run` | commit=0, push=0, preview | `WILL_COMMIT=0 WILL_PUSH=0 WILL_RUN_TESTS=1` | ✓ | ✓ | **PASS** |
| 3 | `--commit` | commit=1, push=1 | `WILL_COMMIT=1 WILL_PUSH=1 WILL_RUN_TESTS=1` | ✓ | ✓ | **PASS** |
| 4 | `--commit --no-push` | commit=1, push=0 | `WILL_COMMIT=0 WILL_PUSH=0 WILL_RUN_TESTS=1` | ✗ | ✓ | **DISCREPANCY** |
| 5 | `--commit --dry-run` | commit=0, push=0 (dry-run wins) | `WILL_COMMIT=0 WILL_PUSH=0 WILL_RUN_TESTS=1` | ✓ | ✓ | **PASS** |

### Verbatim harness output (all 5 cases)

```
=== Case 1: default (no flags) ===
NO_PUSH=0 DRY_RUN=0 COMMIT=0
WILL_COMMIT=0
WILL_PUSH=0
WILL_RUN_TESTS=1

=== Case 2: --dry-run ===
NO_PUSH=0 DRY_RUN=1 COMMIT=0
WILL_COMMIT=0
WILL_PUSH=0
WILL_RUN_TESTS=1

=== Case 3: --commit ===
NO_PUSH=0 DRY_RUN=0 COMMIT=1
WILL_COMMIT=1
WILL_PUSH=1
WILL_RUN_TESTS=1

=== Case 4: --commit --no-push ===
NO_PUSH=1 DRY_RUN=0 COMMIT=1
WILL_COMMIT=0
WILL_PUSH=0
WILL_RUN_TESTS=1

=== Case 5: --commit --dry-run ===
NO_PUSH=0 DRY_RUN=1 COMMIT=1
WILL_COMMIT=0
WILL_PUSH=0
WILL_RUN_TESTS=1
```

---

## 5. Case 4 discrepancy — root-cause analysis

**Expected (caveat §4.4 matrix):** `--commit --no-push` → commit happens, push
suppressed → `WILL_COMMIT=1, WILL_PUSH=0`.

**Actual (script):** `--commit --no-push` → `WILL_COMMIT=0, WILL_PUSH=0` (no
commit, no push).

**Root cause:** the script's `NO_PUSH` branch (L1084–1088) is the **first**
clause in the elif chain and, when it fires, it skips the commit step
entirely — it does not fall through to the `COMMIT` branch. The branch body is:

```bash
if [ $NO_PUSH -eq 1 ]; then
    echo "▸ Skipping commit/push (--no-push)"
    echo "  (Note: auto-commit is now OFF by default; --no-push is retained for"
    echo "   backward compat and is equivalent to the new default. Pass --commit"
    echo "   to opt in to auto-commit + push.)"
elif ...
```

The inline documentation (L1047–1048) explicitly states this is intentional:

> `--no-push` is retained for backward compatibility and is now equivalent to
> the new default (no commit/push) but prints its own message.

So the script treats `--no-push` as **"skip commit/push entirely"** (a synonym
for the default-off path with a different echo), whereas caveat §4.4's matrix
treats `--no-push` as **"suppress push only"** (so `--commit --no-push` still
commits). These are two different semantics for the same flag:

| Interpretation | `--no-push` alone | `--commit --no-push` |
|---|---|---|
| Script's actual (L1084): "skip commit/push entirely" | no commit, no push | no commit, no push |
| Caveat §4.4 matrix: "suppress push only" | no commit (default), no push | **commit**, no push |

The discrepancy only manifests when `--commit` and `--no-push` are combined.
For the other 4 cases the two interpretations agree.

**Impact:** an operator passing `--commit --no-push` expecting "commit locally,
don't push" instead gets "do nothing" — the test results are not committed at
all. This is a silent no-op rather than a data-loss bug, but it diverges from
the documented caveat behaviour.

**Precedence note:** the *order* of the elif chain (`--no-push > --dry-run >
--commit > default-off`) IS correct and matches caveat §4.4. The discrepancy is
in the *body* of the `--no-push` branch (it skips commit rather than only
suppressing push), not in the precedence ordering itself.

---

## 6. Recommended fix (out of scope — read-only audit)

To make the script match caveat §4.4's matrix for case 4, restructure the
decision chain so `--no-push` suppresses only the push step when `--commit` is
also set. Sketch (NOT applied — source edits are out of scope for this task):

```bash
if [ $DRY_RUN -eq 1 ]; then
    # dry-run preview (highest precedence for commit suppression)
    ...
elif [ $COMMIT -eq 1 ]; then
    # commit always
    git add -f ... ; git commit -m ...
    if [ $NO_PUSH -eq 1 ]; then
        echo "▸ --no-push: committed locally, skipping push."
    else
        git push origin HEAD
    fi
else
    # default-off preview (and --no-push alone = same as default)
    ...
fi
```

This makes `--dry-run > --commit` (dry-run wins, no commit) and folds
`--no-push` into the commit branch as a push-only suppressor, matching the
matrix. The precedence becomes `--dry-run > --commit > (--no-push suppresses
push within commit) > default-off`, and `--no-push` alone still equals
default-off (no commit). Recommend a small wave-5 source-edit task to apply
this.

---

## 7. DoD assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| All 5 flag combinations produce expected `WILL_COMMIT`/`WILL_PUSH` | **4/5 PASS, 1 DISCREPANCY** | cases 1, 2, 3, 5 match exactly; case 4 (`--commit --no-push`) gives `WILL_COMMIT=0` but matrix expects `WILL_COMMIT=1` — see §5 |
| Precedence matches `--no-push > --dry-run > --commit > default-off` | **PASS** (order) | elif chain at L1084–1186 is `NO_PUSH → DRY_RUN → COMMIT → else`; order is exactly as the caveat specifies |
| Summary markdown at `vuma/scripts/audit/wave5_flag_precedence.md` | **PASS** | this file |

---

## 8. Constraint check

- No source files edited. `git status` shows only this new audit markdown (+ worklog append).
- `pi5_test_suite.sh` was **NOT** invoked with `--commit` — only the verbatim-extracted harness ran, which performs no git operations and runs no tests.
- No push (no commits created by the audit; the only commit is this manual markdown + worklog).
- No further sub-agents spawned.
- Time budget: ~6 minutes.

---

## 9. Note for orchestrator

**Precedence chain order: verified correct** (`--no-push > --dry-run > --commit
> default-off` in the elif chain at L1084–1186). **4 of 5 matrix cases produce
the expected `WILL_COMMIT`/`WILL_PUSH`.** Case 4 (`--commit --no-push`) has a
**documented discrepancy**: the script's `--no-push` branch (L1084–1088) skips
commit entirely (treating `--no-push` as a synonym for default-off, per the
inline comment at L1047–1048), whereas caveat §4.4's matrix expects commit to
proceed with only push suppressed (`WILL_COMMIT=1, WILL_PUSH=0`). This is a
**semantic mismatch in the branch body**, not a precedence-ordering defect.
Recommended one-block source restructure documented in §6 (out of scope for this
read-only audit). If strict 5/5 matrix compliance is required, a small wave-5
source-edit task should apply the §6 fix.
