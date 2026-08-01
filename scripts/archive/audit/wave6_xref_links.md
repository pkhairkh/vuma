# Wave 6 — Caveat §6 Cross-reference Link Audit

- **Task ID:** 6-b-audit
- **Agent:** 6-b-audit (sub-agent, wave 6)
- **Wave:** 6 (depends on waves 0 / 1 / 2 / 3 / 4 / 5 / 6-a-audit)
- **Caveat addressed:** §6 — Cross-references (all links in `docs/caveats.md` must resolve to existing files)
- **Files in scope (READ-ONLY audit):** `/home/z/my-project/vuma/docs/caveats.md` and all link targets
- **Files out of scope:** any source file (no source edits)
- **DoD:** this summary markdown exists at `vuma/scripts/audit/wave6_xref_links.md`; either zero broken links OR all broken links listed for orchestrator follow-up
- **HEAD before this task:** `138ff874 [6-d-fix]`

## Scope

Audit every Markdown link of the form `[label](target)` in
`docs/caveats.md` whose target is a `.md` file. Two groups are
covered:

1. **§6 explicit cross-reference list** (caveats.md L272–L280) —
   the 9 doc links called out in the protocol.
2. **Inline `.md` links elsewhere in `caveats.md`** — links to
   `scripts/audit/*.md` audit artefacts and to `docs/*.md` files
   referenced from §2, §3, §4 bodies.

Anchor-bearing links (`[label](file.md#section-name)`) are checked
for both file existence and anchor (header) resolution. Bare-file
links are checked for file existence only.

## Commands run

```
# Extract every .md link in caveats.md (link + line number)
rg -n '\]\([^)]+\.md[^)]*\)' docs/caveats.md

# Extract every anchor-bearing link (none found in this file)
rg -n '\]\([^)]*#[^)]*\)' docs/caveats.md

# Per-target existence check
for f in <target list>; do [ -f "$f" ] && echo "EXISTS: $f" || echo "MISSING: $f"; done
```

## Aggregate results

- **Total `.md` link occurrences in `caveats.md`:** 17
- **Unique link targets:** 11 (9 docs/*.md + 2 scripts/audit/*.md)
- **Anchor-bearing links:** 0 (no link in `caveats.md` uses a
  `#section-name` URL fragment; section references such as "§1"
  appear in surrounding prose, not in the URL).
- **Broken file targets:** 0
- **Broken anchors:** 0 (no anchors to check)

All 11 unique targets resolve to existing files. Zero broken links.

## Per-link table

| # | Link source (caveats.md:line) | Target (relative to `docs/`) | Absolute path | Exists? | Anchor? | Anchor resolves? |
|---|-------------------------------|------------------------------|---------------|---------|---------|------------------|
| 1 | L47  | `../scripts/audit/allocator_classification.md` | `vuma/scripts/audit/allocator_classification.md` | YES | — | — |
| 2 | L96  | `../scripts/audit/wave2_stackslot_results.md`   | `vuma/scripts/audit/wave2_stackslot_results.md`   | YES | — | — |
| 3 | L116 | `../scripts/audit/allocator_classification.md` | `vuma/scripts/audit/allocator_classification.md` | YES | — | — |
| 4 | L117 | `backends.md`                                   | `vuma/docs/backends.md`                           | YES | — | — |
| 5 | L148 | `backends.md`                                   | `vuma/docs/backends.md`                           | YES | — | — |
| 6 | L172 | `pmt-formal-spec.md`                            | `vuma/docs/pmt-formal-spec.md`                    | YES | — | — |
| 7 | L172 | `pmt-iris-spec.md`                              | `vuma/docs/pmt-iris-spec.md`                      | YES | — | — |
| 8 | L204 | `building.md`                                   | `vuma/docs/building.md`                           | YES | — | — |
| 9 | L272 | `building.md`                                   | `vuma/docs/building.md`                           | YES | — | — |
| 10| L273 | `backends.md`                                   | `vuma/docs/backends.md`                           | YES | — | — |
| 11| L274 | `fp_backends.md`                                | `vuma/docs/fp_backends.md`                        | YES | — | — |
| 12| L275 | `pipeline.md`                                   | `vuma/docs/pipeline.md`                           | YES | — | — |
| 13| L276 | `pmt-formal-spec.md`                            | `vuma/docs/pmt-formal-spec.md`                    | YES | — | — |
| 14| L277 | `pmt-iris-spec.md`                              | `vuma/docs/pmt-iris-spec.md`                      | YES | — | — |
| 15| L278 | `testing.md`                                    | `vuma/docs/testing.md`                            | YES | — | — |
| 16| L279 | `architecture.md`                               | `vuma/docs/architecture.md`                       | YES | — | — |
| 17| L280 | `kernel-architecture.md`                        | `vuma/docs/kernel-architecture.md`                | YES | — | — |

The `—` entries in the "Anchor?" / "Anchor resolves?" columns mean
"not applicable" (the link is a bare-file link with no URL
fragment).

## Protocol-mandated §6 link list (9 targets)

All 9 cross-reference targets enumerated in the protocol were
verified to exist:

| Protocol target                       | Exists? | Path                                    |
|---------------------------------------|---------|-----------------------------------------|
| `docs/building.md`                    | YES     | `vuma/docs/building.md`                 |
| `docs/backends.md`                    | YES     | `vuma/docs/backends.md`                 |
| `docs/fp_backends.md`                 | YES     | `vuma/docs/fp_backends.md`              |
| `docs/pipeline.md`                    | YES     | `vuma/docs/pipeline.md`                 |
| `docs/pmt-formal-spec.md`             | YES     | `vuma/docs/pmt-formal-spec.md`          |
| `docs/pmt-iris-spec.md`               | YES     | `vuma/docs/pmt-iris-spec.md`            |
| `docs/testing.md`                     | YES     | `vuma/docs/testing.md`                  |
| `docs/architecture.md`                | YES     | `vuma/docs/architecture.md`             |
| `docs/kernel-architecture.md`         | YES     | `vuma/docs/kernel-architecture.md`      |

## Prose section references (informational)

Two places in `caveats.md` reference a *section* of a target file in
prose (not via a URL fragment, so strictly out of scope for anchor
validation, but checked for courtesy):

- **caveats.md L117:** "…[`docs/backends.md`](backends.md) §1 carries
  the matching `Regalloc` column." — `docs/backends.md` does contain
  a `## 1. Backend Overview Table` header at L32 (verified via
  `rg -n '^##? 1' docs/backends.md`). **Prose reference resolves.**
- **caveats.md L248:** references `scripts/audit/wave5_flag_precedence.md`
  §6 (not as a Markdown link, only as a prose mention). The file
  exists (verified via LS) and contains a `## 6.` section.
  **Prose reference resolves.**

Neither prose reference is a Markdown URL fragment, so neither
contributes a "broken anchor" to this audit; both are recorded here
for completeness.

## Other Markdown links inside `caveats.md`

No other kinds of `.md` link appear in `caveats.md`. The 17 link
occurrences in the per-link table above are exhaustive. (Verified
via `rg -n '\]\([^)]+\.md[^)]*\)' docs/caveats.md` — output matches
the table exactly.)

Non-`.md` link targets (none of which are in scope for caveat §6):
- The intro blockquote references `rg -n 'TargetAgnosticRegAlloc'
  src/codegen/src/regalloc.rs` as an inline-command hint, not a
  Markdown link.

## DoD Assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| Summary markdown exists at `vuma/scripts/audit/wave6_xref_links.md` | **PASS** | this file |
| Zero broken links, OR all broken links listed for orchestrator follow-up | **PASS** | zero broken links (17/17 link occurrences resolve; 11/11 unique targets exist; 0 anchor-bearing links to validate) |

## Constraint check

- READ-ONLY audit: no source files edited. `git status` will show
  only the new audit markdown (+ the worklog append).
- No `git push` invoked (local commit only).
- No further sub-agents spawned.
- Time budget: ~3 minutes.

## Status: PASS — all 17 `.md` link occurrences in `docs/caveats.md` resolve to existing files; zero broken links; zero broken anchors; zero follow-up items for the orchestrator.
