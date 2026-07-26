-- check_pmt.lean — executable wrapper that imports PMT (the root module
-- that re-exports PMT.Basic, PMT.Field, PMT.Liveness, PMT.Soundness) and
-- (when uncommented) runs the #eval sanity checks.
--
-- Build: `lake exe check-pmt`
-- Run:   `lake exe check-pmt`

import PMT

open PMT

def main : IO Unit := do
  IO.println "PMT imported successfully."
  IO.println "Lean proof library is linked and ready."
  IO.println "Uncomment #eval lines in PMT/Soundness.lean to run sanity checks."
