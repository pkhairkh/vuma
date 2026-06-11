# Task W3: BCM2712 SoC Targeting

**Agent**: main
**Status**: ✅ Completed

## Summary

Updated the VUMA Pi5 bare-metal crate to properly target the BCM2712 SoC (Raspberry Pi 5).

## Key Changes

1. **GIC-400 Driver** (`src/pi5/src/gic.rs`): Full GIC-400 interrupt controller driver with BCM2712 constants, 9 tests
2. **Exception Handlers** (`src/pi5/src/exception.rs`): ExceptionContext, ExceptionType, handlers, install_handlers(), 7 tests
3. **Boot Assembly** (`src/pi5/src/boot.rs`): Replaced spin-loop handlers with proper save/call/restore/ERET assembly using `exception_entry!` macro
4. **QEMU Targets** (Makefile, justfile): Changed `-M raspi3b` → `-M raspi4b`, added x86-64-run and riscv64-run
5. **UART Base** (`src/std/src/io.rs`): Replaced hardcoded 0xFE201000 with BCM2712 platform constant computation (= 0x1D0A_0000)
6. **Lib.rs**: Added `pub mod gic;`, `pub mod exception;` and re-exports

## Verification

- `cargo clippy -p vuma-pi5 -p vuma-std -- -D warnings`: 0 warnings
- gic tests: 9 pass, exception tests: 7 pass
- vuma-std tests: 312 pass (including io tests with new BCM2712 addresses)
