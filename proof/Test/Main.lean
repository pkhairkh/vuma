import PMT.Test.ValidProgram
import PMT.Test.UafProgram
import PMT.Test.OverflowProgram
import PMT.Test.EmptyProgram
import PMT.Test.MultiStepProgram

/-!
# PMT Test Runner — harness entry point

This is the `lake exe test` entry point (`root = "Test.Main"` in
`lakefile.toml`). It imports every `PMT.Test.*` module so that running
`lake exe test` (or `make proof-test` / `just proof-test` from the repo
root) type-checks all five test modules in one shot and prints a success
banner.

The test modules themselves contain `example`/`theorem` assertions that
are machine-checked at build time — `lake build` already verifies them
when it builds the `PMT` library (since `PMT.lean` imports each
`PMT.Test.*` module). This runner's job is to provide a single CLI
entry point that fails loudly if any test module fails to type-check,
and prints a green summary banner otherwise.

Test modules (all sorry-free, all close by `rfl`/`decide`/`omega`/`simp`):

  * `PMT.Test.ValidProgram`     (W7-A) — valid 2-step happy path:
    `exec prog initState = Result.ok 32`.
  * `PMT.Test.UafProgram`       (W7-B) — UAF trap: dead input →
    `Result.trap 135`, short-circuits remaining steps.
  * `PMT.Test.OverflowProgram`  (W7-C) — arena overflow: oversized
    layout → `Result.trap 1`, with boundary `exact-fit` negative control.
  * `PMT.Test.EmptyProgram`     (W7-D) — nil case: `exec [] s =
    Result.ok s.arena.used`, vacuous `WellTyped []`.
  * `PMT.Test.MultiStepProgram` (W7-E) — 4-step capacity preservation:
    `exec prog initState = Result.ok 64`, each step closed by `rfl`.
-/

/-- Test runner entry point. Importing the five `PMT.Test.*` modules
above is the actual test — if any module fails to type-check, `lake
exe test` exits non-zero before `main` ever runs. Once we reach `main`,
all tests have passed; we print a green banner and exit 0. -/
def main : IO Unit := do
  IO.println "All PMT tests passed."
