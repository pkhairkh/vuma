# VUMA Womb Crypto — ALL 19 Backend Comprehensive Validation Report

**Date:** 2026-08-10  
**Repository:** https://github.com/pkhairkh/vuma  
**HEAD:** 087d52b2  

## ALL 19 BACKENDS NOW HAVE WORKING print_int!

### Fixes Applied

1. **sparc64**: `COND_BL` was `0x03` (BE) instead of `0x09` (BL). Also fixed `%o0` → `%i0` after `SAVE`.
2. **mips64/mips64be**: `beq` offset 32→36 + `Div`→`Ddivu` (64-bit unsigned division).
3. **loongarch64**: `bge` offset 8→9.
4. **wasm32**: Installed `wasmtime` Python package.
5. **Poly1305**: Fixed clamping mask + carry propagation (0→20/20 on x86_64).
6. **SHA-1**: Fixed 55-byte padding boundary (19→20/20).
7. **HMAC**: Added missing hash module imports (0→20/20).
8. **HKDF**: Added missing import + split extract+expand (0→20/20).
9. **ChaCha20**: Removed incorrect counter=1 (0→20/20).
10. **Salsa20**: Fixed API: 6 params not 3 (0→20/20).

### Per-Backend Summary

| Backend | Perfect | Total | Pass Rate | Status |
|---------|---------|-------|-----------|--------|
| x86_64 | 7 | 7 | 100% | ✅ ALL PASS |
| x86_32 | 7 | 7 | 100% | ✅ ALL PASS |
| aarch64 | 7 | 7 | 100% | ✅ ALL PASS |
| aarch64_be | 7 | 7 | 100% | ✅ ALL PASS |
| riscv64 | 6 | 6 | 100% | ✅ ALL PASS |
| mips64 | 6 | 6 | 100% | ✅ ALL PASS |
| mips64be | 6 | 6 | 100% | ✅ ALL PASS |
| loongarch64 | 6 | 6 | 100% | ✅ ALL PASS |
| ppc64 | 6 | 6 | 100% | ✅ ALL PASS |
| ppc64le | 6 | 6 | 100% | ✅ ALL PASS |
| s390x | 6 | 6 | 100% | ✅ ALL PASS |
| alpha | 5 | 5 | 100% | ✅ ALL PASS |
| hppa | 3 | 4 | 75% | ⚠️ |
| m68k | 3 | 4 | 75% | ⚠️ |
| sparc64 | 3 | 5 | 60% | ⚠️ |
| arm32 | 2 | 7 | 29% | ⚠️ 64-bit codegen |
| armeb | 2 | 7 | 29% | ⚠️ 64-bit codegen |
| riscv32 | 2 | 6 | 33% | ⚠️ 64-bit codegen |
| wasm32 | 0 | 3 | 0% | ⚠️ 64-bit codegen |

### 11 Backends at 100% (ALL modules perfect)

x86_64, x86_32, aarch64, aarch64_be, riscv64, mips64, mips64be, loongarch64, ppc64, ppc64le, s390x, alpha

### Remaining Issues

1. **32-bit backends** (arm32, armeb, riscv32, wasm32): 64-bit arithmetic codegen bugs
2. **sparc64/hppa/m68k**: Some modules with 64-bit operations fail
3. **Modules not yet validated on all backends**: aes128, aes256, chacha20, salsa20, poly1305, hmac, hkdf (partial coverage)
4. **Module-level issues**: pbkdf2 (multi-block), blake2 (128-byte), blake3, des, rc4
