//! VUMA syscall ABI translation (P1 foundation).
//!
//! The VUMA `syscall(nr, args...)` intrinsic (lexer → `TokenKind::Syscall` →
//! `Expr::Syscall` → `IRInstr::Syscall { nr, args, dst }` at `ir.rs:1579`)
//! historically emitted `nr` VERBATIM into each backend's syscall-number
//! register. This is NOT portable: `syscall(1, ...)` is `write` on x86_64 but
//! `io_destroy` on aarch64 (where `write` = 64).
//!
//! This module introduces the **VUMA-generic syscall numbering** (= Linux
//! `asm-generic/unistd.h`, used natively by aarch64 / riscv64 / riscv32 /
//! loongarch64) and per-arch translation tables for the backends whose native
//! numbering differs (x86_64, x86_32). It is the foundation for migrating
//! womb's `extern "C" { fn write }` Rust-emitted wrappers to vuma-native
//! `syscall(64, ...)` calls (Open Work §5).
//!
//! ## Foundation-only scope (this wave P1-a)
//!
//! This module is **not wired** into any backend's `IRInstr::Syscall`
//! emission yet — that is wave P1-b. The existing `IRInstr::Syscall { nr }`
//! continues to receive the raw `nr` from the parser and emit it verbatim;
//! `generic_syscall_name` in `ir.rs` continues to use the legacy
//! x86_64-style numbering for IR Display only. Only this new module and its
//! tests are added.
//!
//! ## Identity arches (no translation needed)
//!
//! `aarch64`, `riscv64`, `riscv32`, `loongarch64`, `arm32` (EABI) use the
//! asm-generic numbering natively → `translate` is the identity function.
//!
//! ## Translated arches
//!
//! `x86_64` and `x86_32` have per-arch `match` tables below. The remaining
//! arches (`mips64`, `ppc64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`)
//! return `None` for all numbers with a `TODO(P1-b)` marker — the intrinsic
//! is not portable on these arches yet.
//!
//! ## wasm32
//!
//! `wasm32` uses host imports (`vuma.*`), not syscalls — `translate` returns
//! `None`; the intrinsic is not meaningful on this target.
//!
//! ## BE wrappers
//!
//! `aarch64_be`, `armeb`, `mips64be`, `ppc64le` delegate to their
//! little-endian (or, for `ppc64le`, the bare `ppc64`) counterpart.

use crate::backend::BackendKind;

/// VUMA-generic syscall numbering = Linux `asm-generic/unistd.h` (used
/// natively by aarch64, riscv64, riscv32, loongarch64). Other arches
/// translate via [`translate`].
///
/// Returns the canonical syscall name for a VUMA-generic number, or `None`
/// for numbers outside the table. The table covers at least the entries
/// listed in the existing `backend.rs:2930+` generic stub table (which
/// already uses asm-generic numbers) plus the `*at` family used by the
/// Wave-7 / Wave-8 / Wave-9 POSIX syscall stubs.
///
/// This is the authoritative source for VUMA-generic syscall names. The
/// legacy `ir::generic_syscall_name` (which uses x86_64-style numbers) is
/// preserved only for IR Display and must not be used as a portable key.
pub fn vuma_generic_name(nr: u32) -> Option<&'static str> {
    match nr {
        // ── Process / cwd / root ──
        17 => Some("getcwd"),
        49 => Some("chdir"),
        50 => Some("fchdir"),
        51 => Some("chroot"),
        // ── Event / signal fds ──
        19 => Some("eventfd2"),
        74 => Some("signalfd4"),
        // ── epoll ──
        20 => Some("epoll_create1"),
        21 => Some("epoll_ctl"),
        // NOTE: asm-generic has NO epoll_wait syscall (userspace uses
        // epoll_pwait, nr 22 on some legacy arches but 441 on asm-generic).
        // Nr 22 on asm-generic is `pipe`. The epoll_wait name is intentionally
        // absent here to avoid a collision with pipe.
        22 => Some("pipe"),
        // ── fd ops ──
        23 => Some("dup"),
        24 => Some("dup3"),
        25 => Some("fcntl"),
        29 => Some("ioctl"),
        // ── inotify ──
        26 => Some("inotify_init1"),
        27 => Some("inotify_add_watch"),
        28 => Some("inotify_rm_watch"),
        // ── *at family (file metadata / link / dir ops) ──
        35 => Some("unlinkat"),
        36 => Some("symlinkat"),
        37 => Some("linkat"),
        38 => Some("renameat"),
        46 => Some("ftruncate"),
        48 => Some("faccessat"),
        52 => Some("fchmod"),
        53 => Some("fchmodat"),
        54 => Some("fchownat"),
        55 => Some("fchown"),
        56 => Some("openat"),
        57 => Some("close"),
        62 => Some("lseek"),
        78 => Some("readlinkat"),
        79 => Some("newfstatat"),
        80 => Some("fstat"),
        // ── Core I/O ──
        63 => Some("read"),
        64 => Some("write"),
        65 => Some("readv"),
        66 => Some("writev"),
        67 => Some("pread"),
        68 => Some("pwrite"),
        69 => Some("preadv"),
        70 => Some("pwritev"),
        // ── Sync family ──
        81 => Some("sync"),
        82 => Some("fsync"),
        83 => Some("fdatasync"),
        // ── timerfd ──
        85 => Some("timerfd_create"),
        86 => Some("timerfd_settime"),
        87 => Some("timerfd_gettime"),
        // ── Process / exit ──
        93 => Some("exit"),
        94 => Some("exit_group"),
        95 => Some("waitid"),
        98 => Some("futex"),
        101 => Some("nanosleep"),
        113 => Some("clock_gettime"),
        117 => Some("ptrace"),
        129 => Some("kill"),
        130 => Some("tkill"),
        131 => Some("tgkill"),
        134 => Some("rt_sigaction"),
        135 => Some("rt_sigprocmask"),
        // ── Identity / process group ──
        144 => Some("setgid"),
        146 => Some("setuid"),
        147 => Some("setresuid"),
        149 => Some("setresgid"),
        153 => Some("times"),
        154 => Some("setpgid"),
        155 => Some("getpgid"),
        156 => Some("getsid"),
        157 => Some("setsid"),
        163 => Some("getrlimit"),
        164 => Some("setrlimit"),
        165 => Some("getrusage"),
        166 => Some("umask"),
        169 => Some("gettimeofday"),
        172 => Some("getpid"),
        173 => Some("getppid"),
        174 => Some("getuid"),
        175 => Some("geteuid"),
        176 => Some("getgid"),
        177 => Some("getegid"),
        // ── Networking ──
        198 => Some("socket"),
        200 => Some("bind"),
        201 => Some("listen"),
        202 => Some("accept"),
        203 => Some("connect"),
        206 => Some("sendto"),
        207 => Some("recvfrom"),
        208 => Some("setsockopt"),
        209 => Some("getsockopt"),
        210 => Some("shutdown"),
        // ── Memory ──
        214 => Some("brk"),
        215 => Some("munmap"),
        216 => Some("mremap"),
        220 => Some("clone"),
        221 => Some("execve"),
        222 => Some("mmap"),
        226 => Some("mprotect"),
        227 => Some("msync"),
        228 => Some("mlock"),
        229 => Some("munlock"),
        230 => Some("mlockall"),
        231 => Some("munlockall"),
        232 => Some("mincore"),
        233 => Some("madvise"),
        // ── Misc ──
        260 => Some("wait4"),
        261 => Some("prlimit64"),
        278 => Some("getrandom"),
        281 => Some("execveat"),
        306 => Some("syncfs"),
        435 => Some("clone3"),
        _ => None,
    }
}

/// Translate a VUMA-generic syscall number (Linux `asm-generic/unistd.h`)
/// to its native per-arch syscall number.
///
/// Returns `Some(native_nr)` if the translation is known, `None` otherwise.
/// `None` indicates the intrinsic is not portable for that (backend, nr)
/// pair — either because the backend does not use syscalls at all (wasm32),
/// or because the per-arch table has not been filled in yet (mips64 / ppc64
/// / s390x / sparc64 / alpha / hppa / m68k — TODO P1-b), or because the
/// specific `nr` is not in the per-arch `match` table.
///
/// Identity arches (aarch64, riscv64, riscv32, loongarch64, arm32 EABI)
/// always return `Some(generic_nr)` regardless of whether the number is in
/// [`vuma_generic_name`]'s table — the kernel will return `-ENOSYS` for
/// genuinely unknown numbers, which is the correct behaviour.
pub fn translate(backend: BackendKind, generic_nr: u32) -> Option<u32> {
    match backend {
        // ── Identity arches: native numbering == asm-generic ──
        // aarch64 / riscv64 / riscv32 / loongarch64 all use asm-generic
        // natively (verified in their respective backend stub tables:
        // aarch64 backend.rs:2941+, riscv64.rs:6054+, riscv32.rs:6086+,
        // loongarch64/mod.rs uses the same table).
        BackendKind::AArch64
        | BackendKind::RiscV64
        | BackendKind::RiscV32
        | BackendKind::LoongArch64 => Some(generic_nr),

        // arm32 (EABI): modern kernels use asm-generic numbering.
        // NOTE: arm32-OABI would differ, but VUMA targets EABI exclusively
        // (the arm32 backend's `extern "C"` stubs in arm32/mod.rs:7212+ use
        // the legacy ARM EABI table for its `extern "C"` wrappers, but the
        // `syscall` intrinsic is being standardized on asm-generic numbers
        // per the P1 design — see TASKS.md Open Work §5). P1-b will
        // reconcile the arm32 backend's `extern "C"` stub table with the
        // intrinsic's asm-generic numbering if needed.
        BackendKind::Arm32 => Some(generic_nr),

        // ── wasm32: uses `vuma.*` host imports, not syscalls ──
        // The `syscall` intrinsic is not meaningful on this target.
        BackendKind::Wasm32 => None,

        // ── Translated arches: per-arch match table ──
        BackendKind::X86_64 => translate_x86_64(generic_nr),
        BackendKind::X86_32 => translate_x86_32(generic_nr),

        // ── TODO(P1-b): per-arch table ──
        // The following arches have their own native syscall tables which
        // differ from asm-generic. Translation tables are NOT yet filled
        // in (foundation wave P1-a). The intrinsic is not portable on
        // these arches yet — `translate` returns `None` for all numbers.
        // P1-b will fill these in:
        //   * mips64  — Linux `arch/mips/include/uapi/asm/unistd.h`
        //   * ppc64   — Linux `arch/powerpc/include/uapi/asm/unistd.h`
        //   * s390x   — Linux `arch/s390/include/uapi/asm/unistd.h`
        //   * sparc64 — Linux `arch/sparc/include/uapi/asm/unistd.h`
        //   * alpha   — Linux `arch/alpha/include/uapi/asm/unistd.h`
        //   * hppa    — Linux `arch/parisc/include/uapi/asm/unistd.h`
        //   * m68k    — Linux `arch/m68k/include/uapi/asm/unistd.h`
        BackendKind::Mips64
        | BackendKind::PowerPC64
        | BackendKind::S390X
        | BackendKind::Sparc64
        | BackendKind::Alpha
        | BackendKind::Hppa
        | BackendKind::M68k => None,

        // ── BE wrappers: delegate to their LE (or bare) counterpart ──
        // aarch64_be → aarch64 (identity)
        // armeb      → arm32   (identity)
        // mips64be   → mips64  (returns None — TODO P1-b)
        // ppc64le    → ppc64   (returns None — TODO P1-b)
        BackendKind::AArch64Be => translate(BackendKind::AArch64, generic_nr),
        BackendKind::ArmEb => translate(BackendKind::Arm32, generic_nr),
        BackendKind::Mips64Be => translate(BackendKind::Mips64, generic_nr),
        BackendKind::PowerPC64LE => translate(BackendKind::PowerPC64, generic_nr),
    }
}

/// x86_64 translation table (Linux `arch/x86/entry/syscalls/syscall_64.tbl`).
///
/// Source of truth: the x86_64 `build_runtime_syscall_stubs` table at
/// `src/codegen/src/x86_64/mod.rs:3004+`. Each entry maps a VUMA-generic
/// (asm-generic) number to its native x86_64 number.
fn translate_x86_64(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        // ── Process / cwd / root ──
        17 => Some(79),    // getcwd
        49 => Some(80),    // chdir
        50 => Some(81),    // fchdir
        51 => Some(161),   // chroot
        // ── Event / signal fds ──
        19 => Some(290),   // eventfd2
        74 => Some(289),   // signalfd4
        // ── epoll ──
        20 => Some(291),   // epoll_create1
        21 => Some(233),   // epoll_ctl
        // Nr 22 = pipe on asm-generic. x86_64 pipe = 22.
        22 => Some(22),    // pipe
        // ── fd ops ──
        23 => Some(32),    // dup
        24 => Some(292),   // dup3
        25 => Some(72),    // fcntl
        29 => Some(16),    // ioctl
        // ── inotify ──
        26 => Some(294),   // inotify_init1
        27 => Some(254),   // inotify_add_watch
        28 => Some(255),   // inotify_rm_watch
        // ── *at family ──
        35 => Some(263),   // unlinkat
        36 => Some(266),   // symlinkat
        37 => Some(265),   // linkat
        38 => Some(264),   // renameat
        46 => Some(46),    // ftruncate
        48 => Some(269),   // faccessat
        52 => Some(91),    // fchmod
        53 => Some(268),   // fchmodat
        54 => Some(260),   // fchownat
        55 => Some(93),    // fchown
        56 => Some(257),   // openat
        57 => Some(3),     // close
        62 => Some(8),     // lseek
        78 => Some(267),   // readlinkat
        79 => Some(262),   // newfstatat
        80 => Some(5),     // fstat
        // ── Core I/O ──
        63 => Some(0),     // read
        64 => Some(1),     // write
        65 => Some(19),    // readv
        66 => Some(20),    // writev
        67 => Some(17),    // pread
        68 => Some(18),    // pwrite
        69 => Some(295),   // preadv
        70 => Some(296),   // pwritev
        // ── Sync family ──
        81 => Some(162),   // sync
        82 => Some(74),    // fsync
        83 => Some(75),    // fdatasync
        // ── timerfd ──
        85 => Some(283),   // timerfd_create
        86 => Some(286),   // timerfd_settime
        87 => Some(287),   // timerfd_gettime
        // ── Process / exit ──
        93 => Some(60),    // exit
        94 => Some(231),   // exit_group
        98 => Some(202),   // futex
        101 => Some(35),   // nanosleep
        113 => Some(228),  // clock_gettime
        117 => Some(101),  // ptrace
        129 => Some(62),   // kill
        130 => Some(200),  // tkill
        131 => Some(234),  // tgkill
        134 => Some(13),   // rt_sigaction
        135 => Some(14),   // rt_sigprocmask
        // ── Identity / process group ──
        144 => Some(106),  // setgid
        146 => Some(105),  // setuid
        147 => Some(117),  // setresuid
        149 => Some(119),  // setresgid
        153 => Some(100),  // times
        154 => Some(109),  // setpgid
        155 => Some(121),  // getpgid
        156 => Some(124),  // getsid
        157 => Some(112),  // setsid
        163 => Some(97),   // getrlimit
        164 => Some(160),  // setrlimit
        165 => Some(98),   // getrusage
        166 => Some(95),   // umask
        169 => Some(96),   // gettimeofday
        172 => Some(39),   // getpid
        173 => Some(110),  // getppid
        174 => Some(102),  // getuid
        175 => Some(107),  // geteuid
        176 => Some(104),  // getgid
        177 => Some(108),  // getegid
        // ── Networking ──
        198 => Some(41),   // socket
        200 => Some(49),   // bind
        201 => Some(50),   // listen
        202 => Some(43),   // accept
        203 => Some(42),   // connect
        206 => Some(44),   // sendto
        207 => Some(45),   // recvfrom
        208 => Some(54),   // setsockopt
        209 => Some(55),   // getsockopt
        210 => Some(48),   // shutdown
        // ── Memory ──
        214 => Some(12),   // brk
        215 => Some(11),   // munmap
        216 => Some(25),   // mremap
        220 => Some(56),   // clone
        221 => Some(59),   // execve
        222 => Some(9),    // mmap
        226 => Some(10),   // mprotect
        227 => Some(26),   // msync
        228 => Some(149),  // mlock
        229 => Some(150),  // munlock
        230 => Some(151),  // mlockall
        231 => Some(152),  // munlockall
        232 => Some(27),   // mincore
        233 => Some(28),   // madvise
        // ── Misc ──
        260 => Some(61),   // wait4
        261 => Some(302),  // prlimit64
        278 => Some(318),  // getrandom
        281 => Some(322),  // execveat
        306 => Some(306),  // syncfs
        435 => Some(435),  // clone3
        _ => None,
    }
}

/// x86_32 (i386) translation table (Linux `arch/x86/entry/syscalls/syscall_32.tbl`).
///
/// Source: `src/codegen/src/x86_32/mod.rs:1876+` stubs table AND known i386
/// numbers. i386 numbers DIFFER from x86_64 — the two tables must not be
/// confused. Note that for legacy syscalls with both an old 16-bit form and
/// a modern `*32` variant (e.g. `fchown` 95 vs `fchown32` 207), this table
/// uses the legacy form to match the task spec; the existing x86_32 backend
/// stubs use the modern `*32` variants in some cases — P1-b will reconcile.
fn translate_x86_32(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        // ── Process / cwd / root ──
        17 => Some(183),   // getcwd
        49 => Some(12),    // chdir
        50 => Some(133),   // fchdir
        51 => Some(61),    // chroot
        // ── Event / signal fds ──
        19 => Some(328),   // eventfd2
        74 => Some(327),   // signalfd4
        // ── fd ops ──
        23 => Some(41),    // dup
        24 => Some(330),   // dup3
        25 => Some(55),    // fcntl
        29 => Some(54),    // ioctl
        // ── inotify ──
        26 => Some(332),   // inotify_init1
        27 => Some(293),   // inotify_add_watch
        28 => Some(294),   // inotify_rm_watch
        // ── *at family ──
        35 => Some(296),   // unlinkat
        36 => Some(304),   // symlinkat
        37 => Some(303),   // linkat
        38 => Some(302),   // renameat
        46 => Some(93),    // ftruncate
        48 => Some(307),   // faccessat
        52 => Some(94),    // fchmod
        53 => Some(306),   // fchmodat
        54 => Some(298),   // fchownat
        55 => Some(95),    // fchown
        56 => Some(295),   // openat
        57 => Some(6),     // close
        62 => Some(19),    // lseek
        78 => Some(305),   // readlinkat
        79 => Some(300),   // newfstatat
        80 => Some(108),   // fstat
        // ── Core I/O ──
        63 => Some(3),     // read
        64 => Some(4),     // write
        65 => Some(145),   // readv
        66 => Some(146),   // writev
        67 => Some(180),   // pread
        68 => Some(181),   // pwrite
        69 => Some(333),   // preadv
        70 => Some(334),   // pwritev
        // ── Sync family ──
        81 => Some(36),    // sync
        82 => Some(95),    // fsync
        83 => Some(148),   // fdatasync
        // ── timerfd ──
        85 => Some(322),   // timerfd_create
        86 => Some(323),   // timerfd_settime
        87 => Some(324),   // timerfd_gettime
        // ── Process / exit ──
        93 => Some(1),     // exit
        94 => Some(252),   // exit_group
        98 => Some(240),   // futex
        101 => Some(162),  // nanosleep
        113 => Some(265),  // clock_gettime
        117 => Some(26),   // ptrace
        129 => Some(37),   // kill
        130 => Some(238),  // tkill
        131 => Some(270),  // tgkill
        134 => Some(67),   // rt_sigaction
        135 => Some(126),  // rt_sigprocmask
        // ── Identity / process group ──
        144 => Some(46),   // setgid
        146 => Some(23),   // setuid
        147 => Some(164),  // setresuid
        149 => Some(170),  // setresgid
        153 => Some(43),   // times
        154 => Some(57),   // setpgid
        155 => Some(132),  // getpgid
        156 => Some(147),  // getsid
        157 => Some(66),   // setsid
        163 => Some(76),   // getrlimit
        164 => Some(75),   // setrlimit
        165 => Some(77),   // getrusage
        166 => Some(60),   // umask
        169 => Some(78),   // gettimeofday
        172 => Some(20),   // getpid
        173 => Some(64),   // getppid
        174 => Some(24),   // getuid
        175 => Some(25),   // geteuid
        176 => Some(47),   // getgid
        177 => Some(49),   // getegid
        // ── Networking ──
        198 => Some(359),  // socket
        200 => Some(361),  // bind
        201 => Some(363),  // listen
        202 => Some(364),  // accept
        203 => Some(362),  // connect
        206 => Some(369),  // sendto
        207 => Some(371),  // recvfrom
        208 => Some(366),  // setsockopt
        209 => Some(365),  // getsockopt
        // ── Memory ──
        214 => Some(45),   // brk
        215 => Some(91),   // munmap
        216 => Some(163),  // mremap
        220 => Some(120),  // clone
        221 => Some(11),   // execve
        222 => Some(90),   // mmap
        226 => Some(125),  // mprotect
        227 => Some(144),  // msync
        228 => Some(150),  // mlock
        229 => Some(151),  // munlock
        230 => Some(152),  // mlockall
        231 => Some(153),  // munlockall
        232 => Some(218),  // mincore
        233 => Some(219),  // madvise
        // ── Misc ──
        260 => Some(114),  // wait4
        261 => Some(340),  // prlimit64
        278 => Some(355),  // getrandom
        281 => Some(356),  // execveat
        306 => Some(326),  // syncfs
        435 => Some(435),  // clone3
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── vuma_generic_name ──

    #[test]
    fn test_generic_names_known() {
        // Core I/O (asm-generic numbers — NOT x86_64 numbers).
        assert_eq!(vuma_generic_name(64), Some("write"));
        assert_eq!(vuma_generic_name(63), Some("read"));
        assert_eq!(vuma_generic_name(57), Some("close"));
        assert_eq!(vuma_generic_name(56), Some("openat"));
        assert_eq!(vuma_generic_name(62), Some("lseek"));
        assert_eq!(vuma_generic_name(222), Some("mmap"));
        assert_eq!(vuma_generic_name(215), Some("munmap"));
        // Process / exit.
        assert_eq!(vuma_generic_name(93), Some("exit"));
        assert_eq!(vuma_generic_name(94), Some("exit_group"));
        assert_eq!(vuma_generic_name(172), Some("getpid"));
        // Modern asm-generic-only syscalls (NOT legacy x86_64 numbers).
        assert_eq!(vuma_generic_name(435), Some("clone3"));
        assert_eq!(vuma_generic_name(281), Some("execveat"));
        assert_eq!(vuma_generic_name(79), Some("newfstatat"));
        assert_eq!(vuma_generic_name(260), Some("wait4"));
        assert_eq!(vuma_generic_name(278), Some("getrandom"));
        // *at family.
        assert_eq!(vuma_generic_name(35), Some("unlinkat"));
        assert_eq!(vuma_generic_name(36), Some("symlinkat"));
        assert_eq!(vuma_generic_name(37), Some("linkat"));
        assert_eq!(vuma_generic_name(38), Some("renameat"));
        assert_eq!(vuma_generic_name(78), Some("readlinkat"));
        // Unknown.
        assert_eq!(vuma_generic_name(0), None);
        assert_eq!(vuma_generic_name(1), None); // not in asm-generic table
        assert_eq!(vuma_generic_name(99999), None);
        assert_eq!(vuma_generic_name(u32::MAX), None);
    }

    // ── translate: identity arches ──

    #[test]
    fn test_translate_identity_aarch64() {
        // aarch64 uses asm-generic natively → translate is identity.
        let sample = [
            64u32, 63, 57, 56, 62, 222, 215, 93, 94, 172, 435, 281, 79, 260,
            278, 35, 36, 37, 38, 78, 17, 49, 113, 169,
        ];
        for n in sample {
            assert_eq!(
                translate(BackendKind::AArch64, n),
                Some(n),
                "aarch64 identity for nr={}",
                n
            );
        }
        // Identity arches return Some(generic_nr) even for numbers not in
        // vuma_generic_name — the kernel returns -ENOSYS for genuinely
        // unknown numbers.
        assert_eq!(translate(BackendKind::AArch64, 99999), Some(99999));
    }

    #[test]
    fn test_translate_identity_other_arches() {
        // riscv64, riscv32, loongarch64, arm32 (EABI) are all identity.
        let sample = [64u32, 63, 57, 56, 93, 222, 435, 17, 49, 113];
        for backend in [
            BackendKind::RiscV64,
            BackendKind::RiscV32,
            BackendKind::LoongArch64,
            BackendKind::Arm32,
        ] {
            for &n in &sample {
                assert_eq!(
                    translate(backend, n),
                    Some(n),
                    "{:?} identity for nr={}",
                    backend,
                    n
                );
            }
        }
    }

    // ── translate: x86_64 ──

    #[test]
    fn test_translate_x86_64() {
        // Core mappings required by the task spec.
        assert_eq!(translate(BackendKind::X86_64, 64), Some(1)); // write
        assert_eq!(translate(BackendKind::X86_64, 63), Some(0)); // read
        assert_eq!(translate(BackendKind::X86_64, 57), Some(3)); // close
        assert_eq!(translate(BackendKind::X86_64, 56), Some(257)); // openat
        assert_eq!(translate(BackendKind::X86_64, 62), Some(8)); // lseek
        assert_eq!(translate(BackendKind::X86_64, 222), Some(9)); // mmap
        assert_eq!(translate(BackendKind::X86_64, 215), Some(11)); // munmap
        assert_eq!(translate(BackendKind::X86_64, 226), Some(10)); // mprotect
        assert_eq!(translate(BackendKind::X86_64, 214), Some(12)); // brk
        assert_eq!(translate(BackendKind::X86_64, 93), Some(60)); // exit
        assert_eq!(translate(BackendKind::X86_64, 94), Some(231)); // exit_group
        assert_eq!(translate(BackendKind::X86_64, 172), Some(39)); // getpid
        assert_eq!(translate(BackendKind::X86_64, 79), Some(262)); // newfstatat
        assert_eq!(translate(BackendKind::X86_64, 80), Some(5)); // fstat
        assert_eq!(translate(BackendKind::X86_64, 198), Some(41)); // socket
        assert_eq!(translate(BackendKind::X86_64, 203), Some(42)); // connect
        assert_eq!(translate(BackendKind::X86_64, 202), Some(43)); // accept
        assert_eq!(translate(BackendKind::X86_64, 200), Some(49)); // bind
        assert_eq!(translate(BackendKind::X86_64, 201), Some(50)); // listen
        assert_eq!(translate(BackendKind::X86_64, 220), Some(56)); // clone
        assert_eq!(translate(BackendKind::X86_64, 221), Some(59)); // execve
        assert_eq!(translate(BackendKind::X86_64, 129), Some(62)); // kill
        assert_eq!(translate(BackendKind::X86_64, 98), Some(202)); // futex
        assert_eq!(translate(BackendKind::X86_64, 101), Some(35)); // nanosleep
        assert_eq!(translate(BackendKind::X86_64, 113), Some(228)); // clock_gettime
        assert_eq!(translate(BackendKind::X86_64, 134), Some(13)); // rt_sigaction
        assert_eq!(translate(BackendKind::X86_64, 135), Some(14)); // rt_sigprocmask
        assert_eq!(translate(BackendKind::X86_64, 260), Some(61)); // wait4
        assert_eq!(translate(BackendKind::X86_64, 278), Some(318)); // getrandom
        assert_eq!(translate(BackendKind::X86_64, 281), Some(322)); // execveat
        assert_eq!(translate(BackendKind::X86_64, 435), Some(435)); // clone3
        assert_eq!(translate(BackendKind::X86_64, 306), Some(306)); // syncfs
        assert_eq!(translate(BackendKind::X86_64, 35), Some(263)); // unlinkat
        assert_eq!(translate(BackendKind::X86_64, 17), Some(79)); // getcwd
        assert_eq!(translate(BackendKind::X86_64, 49), Some(80)); // chdir
    }

    // ── translate: x86_32 ──

    #[test]
    fn test_translate_x86_32() {
        // Core mappings required by the task spec.
        assert_eq!(translate(BackendKind::X86_32, 64), Some(4)); // write
        assert_eq!(translate(BackendKind::X86_32, 63), Some(3)); // read
        assert_eq!(translate(BackendKind::X86_32, 57), Some(6)); // close
        assert_eq!(translate(BackendKind::X86_32, 56), Some(295)); // openat
        assert_eq!(translate(BackendKind::X86_32, 62), Some(19)); // lseek
        assert_eq!(translate(BackendKind::X86_32, 222), Some(90)); // mmap
        assert_eq!(translate(BackendKind::X86_32, 215), Some(91)); // munmap
        assert_eq!(translate(BackendKind::X86_32, 93), Some(1)); // exit
        assert_eq!(translate(BackendKind::X86_32, 94), Some(252)); // exit_group
        assert_eq!(translate(BackendKind::X86_32, 172), Some(20)); // getpid
        assert_eq!(translate(BackendKind::X86_32, 435), Some(435)); // clone3
        // i386-specific networking numbers (DIFFER from x86_64).
        assert_eq!(translate(BackendKind::X86_32, 198), Some(359)); // socket
        assert_eq!(translate(BackendKind::X86_32, 203), Some(362)); // connect
        assert_eq!(translate(BackendKind::X86_32, 202), Some(364)); // accept
        assert_eq!(translate(BackendKind::X86_32, 200), Some(361)); // bind
        assert_eq!(translate(BackendKind::X86_32, 201), Some(363)); // listen
        // i386-specific *at numbers.
        assert_eq!(translate(BackendKind::X86_32, 35), Some(296)); // unlinkat
        assert_eq!(translate(BackendKind::X86_32, 78), Some(305)); // readlinkat
        assert_eq!(translate(BackendKind::X86_32, 79), Some(300)); // newfstatat
    }

    // ── translate: wasm32 ──

    #[test]
    fn test_translate_wasm32_none() {
        // wasm32 uses host imports, not syscalls — always None.
        assert_eq!(translate(BackendKind::Wasm32, 64), None);
        assert_eq!(translate(BackendKind::Wasm32, 63), None);
        assert_eq!(translate(BackendKind::Wasm32, 57), None);
        assert_eq!(translate(BackendKind::Wasm32, 0), None);
        assert_eq!(translate(BackendKind::Wasm32, 99999), None);
    }

    // ── translate: BE wrappers delegate ──

    #[test]
    fn test_translate_be_wrappers_delegate() {
        // aarch64_be delegates to aarch64 (identity).
        assert_eq!(
            translate(BackendKind::AArch64Be, 64),
            translate(BackendKind::AArch64, 64)
        );
        assert_eq!(translate(BackendKind::AArch64Be, 64), Some(64));
        assert_eq!(
            translate(BackendKind::AArch64Be, 222),
            translate(BackendKind::AArch64, 222)
        );

        // armeb delegates to arm32 (identity per task spec).
        assert_eq!(
            translate(BackendKind::ArmEb, 64),
            translate(BackendKind::Arm32, 64)
        );
        assert_eq!(translate(BackendKind::ArmEb, 64), Some(64));

        // mips64be delegates to mips64 (returns None — TODO P1-b).
        assert_eq!(
            translate(BackendKind::Mips64Be, 64),
            translate(BackendKind::Mips64, 64)
        );
        assert_eq!(translate(BackendKind::Mips64Be, 64), None);

        // ppc64le delegates to ppc64 (returns None — TODO P1-b).
        assert_eq!(
            translate(BackendKind::PowerPC64LE, 64),
            translate(BackendKind::PowerPC64, 64)
        );
        assert_eq!(translate(BackendKind::PowerPC64LE, 64), None);
    }

    // ── translate: unknown numbers ──

    #[test]
    fn test_translate_unknown_returns_none() {
        // x86_64: numbers not in the per-arch table return None.
        assert_eq!(translate(BackendKind::X86_64, 99999), None);
        assert_eq!(translate(BackendKind::X86_64, 0), None); // 0 is not in our table
        assert_eq!(translate(BackendKind::X86_64, 1), None); // 1 is not in our table
        // x86_32: same.
        assert_eq!(translate(BackendKind::X86_32, 99999), None);
        // mips64 / ppc64 / s390x / sparc64 / alpha / hppa / m68k: TODO P1-b.
        for backend in [
            BackendKind::Mips64,
            BackendKind::PowerPC64,
            BackendKind::S390X,
            BackendKind::Sparc64,
            BackendKind::Alpha,
            BackendKind::Hppa,
            BackendKind::M68k,
        ] {
            assert_eq!(
                translate(backend, 64),
                None,
                "{:?} should return None for nr=64 (TODO P1-b)",
                backend
            );
        }
        // Identity arches return Some even for unknown numbers — the kernel
        // returns -ENOSYS for genuinely unknown numbers, which is correct.
        assert_eq!(translate(BackendKind::AArch64, 99999), Some(99999));
        assert_eq!(translate(BackendKind::RiscV64, 99999), Some(99999));
    }
}
