#!/usr/bin/env python3
"""VUMA test-suite pass-rate trend viewer.

Reads archived `summary.json` snapshots from ``test_results/history/`` (each
produced by ``pi5_test_suite.sh`` Step 2.5a before the in-place summary.json
is overwritten) and prints a compact pass-rate trend table.

Typical usage::

    # Show last 10 runs (default)
    python3 scripts/show_trend.py

    # Show last 25 runs from a non-default results dir
    python3 scripts/show_trend.py --last 25 --results-dir /path/to/test_results

    # Include the current (not-yet-archived) summary.json as the most-recent row
    python3 scripts/show_trend.py --include-current

This is the read-side companion to the archive logic added in Task 9-c; see
the worklog entry and ``pi5_test_suite.sh`` Step 2.5a for the write side.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Iterable


def _ts_sort_key(name: str) -> tuple[int, str]:
    """Sort archive filenames chronologically.

    Filenames are ``<YYYYMMDDHHMMSS>_summary.json`` (digits stripped from the
    summary.json ``timestamp`` field) or ``unknown_<epoch>_summary.json`` /
    ``<ts>_dup<RANDOM>_summary.json`` for fallbacks. Unknown entries sort
    earliest; ``_dup`` entries sort immediately after their original.
    """
    # Pull leading run of digits (the canonical case).
    m = re.match(r"^(\d+)", name)
    if m:
        primary = int(m.group(1))
    else:
        primary = 0
    # ``_dup`` entries should sort after their original at the same timestamp.
    dup_rank = 1 if "_dup" in name else 0
    return (primary, dup_rank, name)


def _read_summary(path: Path) -> dict:
    try:
        with path.open("r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, json.JSONDecodeError) as exc:
        return {"_error": f"{type(exc).__name__}: {exc}", "_path": str(path)}


def _iter_runs(results_dir: Path, include_current: bool) -> Iterable[tuple[str, dict]]:
    """Yield (label, summary_dict) pairs in chronological order."""
    history = results_dir / "history"
    entries: list[tuple[str, dict, tuple]] = []
    if history.is_dir():
        for p in sorted(history.glob("*_summary.json"), key=lambda x: _ts_sort_key(x.name)):
            entries.append((p.name, _read_summary(p), _ts_sort_key(p.name)))

    if include_current:
        cur = results_dir / "summary.json"
        if cur.is_file():
            entries.append(("current_summary.json", _read_summary(cur), (float("inf"), 0, "")))

    # Final stable chronological sort by extracted key.
    entries.sort(key=lambda e: e[2])
    for name, data, _ in entries:
        yield name, data


def _fmt_pct(s: str | float | None) -> str:
    if s is None:
        return "    -"
    if isinstance(s, str) and s.endswith("%"):
        s = s[:-1]
    try:
        return f"{float(s):6.2f}"
    except (TypeError, ValueError):
        return "    -"


def _row(label: str, d: dict, width: int = 22) -> str:
    if "_error" in d:
        return f"  {label:<{width}}  ERROR  {d['_error']}"
    ts = str(d.get("timestamp", "?"))[:19]
    total = d.get("total_runs", "-")
    matches = d.get("matches", "-")
    skipped = d.get("skipped", "-")
    rate = _fmt_pct(d.get("pass_rate"))
    ive = d.get("ive_verification") or {}
    ive_rate = _fmt_pct(ive.get("pass_rate")) if ive else "    -"
    return f"  {label:<{width}}  {ts:<19}  {total:>7}  {matches:>7}  {skipped:>7}  {rate}%  {ive_rate}%"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        description="Print VUMA test-suite pass-rate trend from archived summaries.",
    )
    p.add_argument(
        "--results-dir",
        default="test_results",
        help="Directory containing summary.json and history/ (default: test_results)",
    )
    p.add_argument(
        "--last",
        "-n",
        type=int,
        default=10,
        help="Show only the last N runs (default: 10; use 0 for all)",
    )
    p.add_argument(
        "--include-current",
        action="store_true",
        help="Also include the in-place test_results/summary.json as the most-recent row",
    )
    args = p.parse_args(argv)

    results_dir = Path(args.results_dir)
    history = results_dir / "history"
    if not history.is_dir() and not (results_dir / "summary.json").is_file():
        print(f"show_trend: no test results found under {results_dir}", file=sys.stderr)
        print(
            "  (run pi5_test_suite.sh first — it archives summary.json into "
            "test_results/history/ on each invocation)",
            file=sys.stderr,
        )
        return 1

    runs = list(_iter_runs(results_dir, args.include_current))
    if not runs:
        print(f"show_trend: history/ is empty and no current summary.json found "
              f"under {results_dir}", file=sys.stderr)
        return 1

    if args.last and args.last > 0:
        runs = runs[-args.last:]

    # Header
    print()
    print(f"  VUMA Test-Suite Pass-Rate Trend  ({len(runs)} run(s) shown)")
    print(f"  source: {results_dir}")
    print()
    print(f"  {'archive':<22}  {'timestamp (UTC)':<19}  {'total':>7}  {'match':>7}  "
          f"{'skip':>7}  {'pass':>7}  {'ive':>7}")
    print(f"  {'-' * 22}  {'-' * 19}  {'-' * 7}  {'-' * 7}  {'-' * 7}  {'-' * 7}  {'-' * 7}")
    for name, d in runs:
        # Truncate long archive names to keep column alignment.
        label = name if len(name) <= 22 else "…" + name[-(21):]
        print(_row(label, d, width=22))
    print()

    # Quick summary stats over the displayed window.
    rates: list[float] = []
    for _, d in runs:
        pr = d.get("pass_rate")
        if isinstance(pr, str) and pr.endswith("%"):
            pr = pr[:-1]
        try:
            rates.append(float(pr))
        except (TypeError, ValueError):
            continue
    if rates:
        lo, hi, mean = min(rates), max(rates), sum(rates) / len(rates)
        delta = rates[-1] - rates[0]
        print(f"  window: min={lo:.2f}%  max={hi:.2f}%  mean={mean:.2f}%  "
              f"Δ(first→last)={delta:+.2f}%")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
