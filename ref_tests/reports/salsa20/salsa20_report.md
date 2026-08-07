# Salsa20 eSTREAM Conformance Test Report

**Date:** 2026-08-07T09:01:42.931951+00:00
**Module:** `womb/crypto/symmetric/salsa20.vuma`
**Compile driver:** `target/release-fast/compile_dump`
**Cross-check:** PyCryptodome

## Block-level vectors (first 32 bytes per block)

| # | Label | Block | Key (hex) | Nonce | Expected[0..31] | Result |
|---|-------|-------|-----------|-------|------------------|--------|
| 0 | eSTREAM Set 1, vector# 0 (key byte 0 = 0x80) | 0 | `8000000000000000…` | `0000000000000000` | `E3BE8FDD8BECA2E3…` | PASS |
| 1 | eSTREAM Set 1, vector# 9 (key byte 1 = 0x40) | 0 | `0040000000000000…` | `0000000000000000` | `01F191C3A1F2CC6E…` | PASS |
| 2 | eSTREAM Set 1, vector# 18 (key byte 2 = 0x20) | 0 | `0000200000000000…` | `0000000000000000` | `C29BA0DA9EBEBFAC…` | PASS |
| 3 | eSTREAM Set 6, vector# 0 (block 0) | 0 | `0053A6F94C9FF245…` | `0D74DB42A91077DE` | `F5FAD53F79F9DF58…` | PASS |
| 4 | eSTREAM Set 6, vector# 0 (block 1 — counter increment) | 1 | `0053A6F94C9FF245…` | `0D74DB42A91077DE` | `86B8FE274643AA1E…` | PASS |

**Block-level summary:** 5/5 passed

## Full 64-byte block 0 verification

**Result:** 3/3 passed

## Multi-block streaming encrypt (counter increment)

**Result:** 7/7 passed

## Grand Summary

**15/15 passed, 0 failed**
