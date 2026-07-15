//! # Local C Type Aliases and FFI Bindings
//!
//! Replaces the `libc` crate dependency for VUMA's test suite.
//!
//! VUMA's codegen backends already maintain their own per-architecture
//! syscall number tables (Waves 1–9). The `libc` crate was previously used
//! only as a source of C type aliases (`c_int`, `c_void`, `c_long`, `size_t`,
//! …) and a handful of Linux/POSIX constants (`PROT_*`, `MAP_*`, …) plus
//! thin FFI veneers around the `mmap`/`mprotect`/`munmap` symbols. All three
//! of these concerns are trivially defined locally, which lets us drop the
//! `libc = "0.2"` workspace dependency entirely.
//!
//! ## Scope
//!
//! The module is gated on `#[cfg(all(unix, any(target_arch = "x86_64",
//! target_arch = "aarch64")))]` to mirror the architecture matrix supported
//! by the JIT execution tests (`sha256d_backends`, `execution_validation`).
//! On other targets the module is empty — the test modules that depend on
//! these symbols are themselves `#[cfg(target_arch = "x86_64")]`-gated, so
//! no broken references are produced.
//!
//! ## Constants
//!
//! The numeric values are the asm-generic / Linux uapi values used by both
//! x86_64 and aarch64 (identical for the symbols below):
//!
//! | Symbol           | Value     | Meaning                                  |
//! |------------------|-----------|------------------------------------------|
//! | `PROT_READ`      | `0x1`     | Page may be read                         |
//! | `PROT_WRITE`     | `0x2`     | Page may be written                      |
//! | `PROT_EXEC`      | `0x4`     | Page may be executed                     |
//! | `MAP_PRIVATE`    | `0x02`    | Changes are private (copy-on-write)      |
//! | `MAP_ANONYMOUS`  | `0x20`    | Mapping not backed by any file           |
//! | `MAP_FAILED`     | `!0u64`   | Sentinel returned by `mmap` on failure   |
//!
//! These match the values used by vuma-cor's Wave 45 raw-syscall FFI
//! (`src/cor/src/runtime.rs`), so the test harness exercises the same
//! calling convention as production code.

#![allow(dead_code)]
// The C-style snake_case type names (`c_int`, `c_void`, `size_t`, …) are
// required by the FFI module spec — they mirror the names used by `libc` and
// `core::ffi` so that downstream call sites read identically to the original
// `libc::`-based code. This is the same idiom `core::ffi` itself uses.
#![allow(non_camel_case_types)]

// ---------------------------------------------------------------------------
// C type aliases
// ---------------------------------------------------------------------------

/// C `int` — signed 32-bit integer.
pub type c_int = i32;
/// C `unsigned int` — unsigned 32-bit integer.
pub type c_uint = u32;
/// C `long` — signed pointer-sized integer (LP64 on Linux x86_64/aarch64).
pub type c_long = isize;
/// C `unsigned long` — unsigned pointer-sized integer (LP64 on Linux).
pub type c_ulong = usize;
/// C `size_t` — unsigned pointer-sized count type.
pub type size_t = usize;
/// C `ssize_t` — signed pointer-sized count type.
pub type ssize_t = isize;
/// C `void` — re-export of the standard library's `core::ffi::c_void`.
pub type c_void = core::ffi::c_void;

// ---------------------------------------------------------------------------
// Linux/POSIX constants (asm-generic uapi values; x86_64 and aarch64 agree)
// ---------------------------------------------------------------------------

/// `PROT_READ` — page may be read.
pub const PROT_READ: c_int = 0x1;
/// `PROT_WRITE` — page may be written.
pub const PROT_WRITE: c_int = 0x2;
/// `PROT_EXEC` — page may be executed.
pub const PROT_EXEC: c_int = 0x4;

/// `MAP_PRIVATE` — private copy-on-write mapping.
pub const MAP_PRIVATE: c_int = 0x02;
/// `MAP_ANONYMOUS` — mapping not backed by any file.
pub const MAP_ANONYMOUS: c_int = 0x20;

/// `MAP_FAILED` — sentinel value returned by `mmap(2)` on failure.
///
/// Defined by POSIX as `(void *)-1`.
pub const MAP_FAILED: *mut c_void = !0usize as *mut c_void;

// ---------------------------------------------------------------------------
// Raw FFI veneers around the libc-vendor `mmap`/`mprotect`/`munmap` symbols
// ---------------------------------------------------------------------------

#[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
extern "C" {
    /// `void *mmap(void *addr, size_t len, int prot, int flags, int fd,
    /// off_t offset)` — map files or devices into memory.
    ///
    /// See `mmap(2)`. Returns `MAP_FAILED` on failure.
    pub fn mmap(
        addr: *mut c_void,
        len: size_t,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;

    /// `int mprotect(void *addr, size_t len, int prot)` — set protection on
    /// a region of memory.
    ///
    /// See `mprotect(2)`. Returns `0` on success, `-1` on failure.
    pub fn mprotect(addr: *mut c_void, len: size_t, prot: c_int) -> c_int;

    /// `int munmap(void *addr, size_t len)` — unmap a region of memory.
    ///
    /// See `munmap(2)`. Returns `0` on success, `-1` on failure.
    pub fn munmap(addr: *mut c_void, len: size_t) -> c_int;
}
