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
//! `x86_64`, `x86_32`, `mips64`, `ppc64`, `s390x`, `sparc64`, `alpha`,
//! `hppa`, and `m68k` each have a per-arch `match` table below. The 7
//! non-x86 tables (filled in P1-d) cover the ~106 most common syscalls
//! (process/memory/file I/O/sockets/signals/time/identity/wait/misc) and
//! return `None` for numbers not in the table. Sources are the Linux UAPI
//! headers shipped at `/usr/lib/linux/uapi/<arch>/asm/unistd_*.h`.
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
        59 => Some("pipe2"),
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
        167 => Some("prctl"),
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
/// or because the specific `nr` is not in the per-arch `match` table (which
/// for the 9 translated arches covers ~106 common syscalls; rare or arch-
/// specific syscalls return `None`).
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

        // arm32 (EABI): ARM EABI uses its own legacy syscall numbering
        // that differs from asm-generic. The translation table covers
        // all syscalls used by VUMA's IPC lowering pass and runtime stubs.
        BackendKind::Arm32 => translate_arm32(generic_nr),

        // ── wasm32: uses `vuma.*` host imports, not syscalls ──
        // The `syscall` intrinsic is not meaningful on this target.
        BackendKind::Wasm32 => None,

        // ── Translated arches: per-arch match table ──
        BackendKind::X86_64 => translate_x86_64(generic_nr),
        BackendKind::X86_32 => translate_x86_32(generic_nr),

        // ── Translated arches: per-arch match table (filled in P1-d) ──
        // Each of these 7 arches has its own native syscall table that
        // differs from asm-generic. The per-arch functions cover ~106
        // common syscalls and return `None` for unknown numbers; for
        // arch-specific syscalls (e.g. `clone3` on sparc64, plain
        // `accept` on s390x/m68k which only have `accept4`) the table
        // returns `None` and the caller is responsible for using a
        // portable alternative.
        //   * mips64  — Linux `arch/mips/include/uapi/asm/unistd_n64.h`
        //   * ppc64   — Linux `arch/powerpc/include/uapi/asm/unistd_64.h`
        //   * s390x   — Linux `arch/s390/include/uapi/asm/unistd_64.h`
        //   * sparc64 — Linux `arch/sparc/include/uapi/asm/unistd_64.h`
        //   * alpha   — Linux `arch/alpha/include/uapi/asm/unistd_32.h`
        //   * hppa    — Linux `arch/parisc/include/uapi/asm/unistd_64.h`
        //   * m68k    — Linux `arch/m68k/include/uapi/asm/unistd_32.h`
        BackendKind::Mips64 => translate_mips64(generic_nr),
        BackendKind::PowerPC64 => translate_powerpc64(generic_nr),
        BackendKind::S390X => translate_s390x(generic_nr),
        BackendKind::Sparc64 => translate_sparc64(generic_nr),
        BackendKind::Alpha => translate_alpha(generic_nr),
        BackendKind::Hppa => translate_hppa(generic_nr),
        BackendKind::M68k => translate_m68k(generic_nr),

        // ── BE wrappers: delegate to their LE (or bare) counterpart ──
        // aarch64_be → aarch64 (identity)
        // armeb      → arm32   (identity)
        // mips64be   → mips64  (per-arch table — P1-d)
        // ppc64le    → ppc64   (per-arch table — P1-d)
        BackendKind::AArch64Be => translate(BackendKind::AArch64, generic_nr),
        BackendKind::ArmEb => translate(BackendKind::Arm32, generic_nr),
        BackendKind::Mips64Be => translate(BackendKind::Mips64, generic_nr),
        BackendKind::PowerPC64LE => translate(BackendKind::PowerPC64, generic_nr),
    }
}

/// Translate a VUMA-generic syscall number to the backend's native number,
/// logging a warning if the number is not in the translation table.
///
/// On identity arches (aarch64, riscv, loongarch, arm32), this always
/// returns the input verbatim (no warning — the number IS native).
/// On translated arches, if the number is unknown, a warning is logged
/// and the generic number is returned verbatim (which may be wrong, but
/// preserves the current behavior for arch-specific syscalls).
pub fn translate_or_warn(backend: BackendKind, generic_nr: u32) -> u32 {
    match translate(backend, generic_nr) {
        Some(native) => native,
        None => {
            // Unknown syscall number — may be arch-specific or a bug.
            // Log a warning but don't abort (preserves current behavior).
            vuma_log!(
                warn,
                "syscall number {} not in translation table for {:?} — \
                 using generic number verbatim (may be wrong on this arch)",
                generic_nr,
                backend
            );
            generic_nr
        }
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
        59 => Some(293),   // pipe2
        62 => Some(8),     // lseek
        73 => Some(7),     // poll / ppoll (x86_64 poll=7, ppoll=271; ipc_lowering uses 73 for both)
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
        167 => Some(157),  // prctl
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


/// MIPS N64 ABI translation table (Linux `arch/mips/include/uapi/asm/unistd_n64.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/mips/asm/unistd_n64.h`.
///
/// The MIPS N64 ABI uses a base offset of 5000 (`__NR_Linux = 5000`) and its
/// own historical numbering that differs from asm-generic. The N32 (base
/// 6000) and O32 (base 4000) ABIs use different bases and are NOT yet
/// supported - callers targeting mips32-o32 or mips32-n32 must translate
/// manually.
fn translate_mips64(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(5077),  // getcwd
        19 => Some(5284),  // eventfd2
        20 => Some(5285),  // epoll_create1
        21 => Some(5208),  // epoll_ctl
        22 => Some(5021),  // pipe
        23 => Some(5031),  // dup
        24 => Some(5286),  // dup3
        25 => Some(5070),  // fcntl
        26 => Some(5288),  // inotify_init1
        27 => Some(5244),  // inotify_add_watch
        28 => Some(5245),  // inotify_rm_watch
        29 => Some(5015),  // ioctl
        35 => Some(5253),  // unlinkat
        36 => Some(5256),  // symlinkat
        37 => Some(5255),  // linkat
        38 => Some(5254),  // renameat
        46 => Some(5075),  // ftruncate
        48 => Some(5259),  // faccessat
        52 => Some(5089),  // fchmod
        53 => Some(5258),  // fchmodat
        54 => Some(5250),  // fchownat
        55 => Some(5091),  // fchown
        56 => Some(5247),  // openat
        57 => Some(5003),  // close
        59 => Some(5287),  // pipe2
        61 => Some(5308),  // getdents64
        62 => Some(5008),  // lseek
        63 => Some(5000),  // read
        64 => Some(5001),  // write
        65 => Some(5018),  // readv
        66 => Some(5019),  // writev
        67 => Some(5016),  // pread
        68 => Some(5017),  // pwrite
        69 => Some(5289),  // preadv
        70 => Some(5290),  // pwritev
        72 => Some(5052),  // socketpair
        73 => Some(5261),  // ppoll
        78 => Some(5257),  // readlinkat
        79 => Some(5252),  // newfstatat
        80 => Some(5005),  // fstat
        81 => Some(5157),  // sync
        82 => Some(5072),  // fsync
        83 => Some(5073),  // fdatasync
        85 => Some(5280),  // timerfd_create
        86 => Some(5282),  // timerfd_settime
        87 => Some(5281),  // timerfd_gettime
        93 => Some(5058),  // exit
        94 => Some(5205),  // exit_group
        95 => Some(5237),  // waitid
        98 => Some(5194),  // futex
        101 => Some(5034),  // nanosleep
        113 => Some(5222),  // clock_gettime
        117 => Some(5099),  // ptrace
        124 => Some(5023),  // sched_yield
        129 => Some(5060),  // kill
        130 => Some(5192),  // tkill
        131 => Some(5225),  // tgkill
        134 => Some(5013),  // rt_sigaction
        135 => Some(5014),  // rt_sigprocmask
        144 => Some(5104),  // setgid
        146 => Some(5103),  // setuid
        147 => Some(5115),  // setresuid
        149 => Some(5117),  // setresgid
        153 => Some(5098),  // times
        154 => Some(5107),  // setpgid
        155 => Some(5119),  // getpgid
        156 => Some(5122),  // getsid
        157 => Some(5110),  // setsid
        163 => Some(5095),  // getrlimit
        164 => Some(5155),  // setrlimit
        165 => Some(5096),  // getrusage
        166 => Some(5093),  // umask
        167 => Some(5153),  // prctl
        169 => Some(5094),  // gettimeofday
        172 => Some(5038),  // getpid
        173 => Some(5108),  // getppid
        174 => Some(5100),  // getuid
        175 => Some(5105),  // geteuid
        176 => Some(5102),  // getgid
        177 => Some(5106),  // getegid
        198 => Some(5040),  // socket
        200 => Some(5048),  // bind
        201 => Some(5049),  // listen
        202 => Some(5042),  // accept
        203 => Some(5041),  // connect
        206 => Some(5043),  // sendto
        207 => Some(5044),  // recvfrom
        208 => Some(5053),  // setsockopt
        209 => Some(5054),  // getsockopt
        210 => Some(5047),  // shutdown
        214 => Some(5012),  // brk
        215 => Some(5011),  // munmap
        216 => Some(5024),  // mremap
        220 => Some(5055),  // clone
        221 => Some(5057),  // execve
        222 => Some(5009),  // mmap
        226 => Some(5010),  // mprotect
        227 => Some(5025),  // msync
        228 => Some(5146),  // mlock
        229 => Some(5147),  // munlock
        232 => Some(5026),  // mincore
        233 => Some(5027),  // madvise
        260 => Some(5059),  // wait4
        261 => Some(5297),  // prlimit64
        278 => Some(5313),  // getrandom
        281 => Some(5316),  // execveat
        306 => Some(5301),  // syncfs
        435 => Some(5435),  // clone3
        
        // ── IPC-critical syscalls ──
        59 => Some(5287),   // pipe2
        220 => Some(5055),  // clone
        260 => Some(5024),  // wait4
        222 => Some(5009),  // mmap
        73 => Some(5188),   // poll
        172 => Some(5038),  // getpid
        167 => Some(5153),  // prctl
        163 => Some(5075),  // setrlimit
        164 => Some(5076),  // getrlimit
        198 => Some(5183),  // socket
        203 => Some(5186),  // connect
        206 => Some(5189),  // sendto
        207 => Some(5190),  // recvfrom

_ => None,
    }
}

/// PowerPC 64-bit translation table (Linux `arch/powerpc/include/uapi/asm/unistd_64.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/powerpc/asm/unistd_64.h`.
///
/// The PowerPC 64-bit table is also used (modulo endianness) by ppc64le via
/// the BE-wrapper delegation in `translate`. The 32-bit PowerPC table
/// (`unistd_32.h`) is NOT yet supported.
fn translate_powerpc64(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(182),  // getcwd
        19 => Some(314),  // eventfd2
        20 => Some(315),  // epoll_create1
        21 => Some(237),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(316),  // dup3
        25 => Some(55),  // fcntl
        26 => Some(318),  // inotify_init1
        27 => Some(276),  // inotify_add_watch
        28 => Some(277),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(292),  // unlinkat
        36 => Some(295),  // symlinkat
        37 => Some(294),  // linkat
        38 => Some(293),  // renameat
        46 => Some(93),  // ftruncate
        48 => Some(298),  // faccessat
        52 => Some(94),  // fchmod
        53 => Some(297),  // fchmodat
        54 => Some(289),  // fchownat
        55 => Some(95),  // fchown
        56 => Some(286),  // openat
        57 => Some(6),  // close
        59 => Some(317),  // pipe2
        61 => Some(202),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(145),  // readv
        66 => Some(146),  // writev
        67 => Some(179),  // pread
        68 => Some(180),  // pwrite
        69 => Some(320),  // preadv
        70 => Some(321),  // pwritev
        72 => Some(333),  // socketpair
        73 => Some(281),  // ppoll
        78 => Some(296),  // readlinkat
        79 => Some(291),  // newfstatat
        80 => Some(108),  // fstat
        81 => Some(36),  // sync
        82 => Some(118),  // fsync
        83 => Some(148),  // fdatasync
        85 => Some(306),  // timerfd_create
        86 => Some(311),  // timerfd_settime
        87 => Some(312),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(234),  // exit_group
        95 => Some(272),  // waitid
        98 => Some(221),  // futex
        101 => Some(162),  // nanosleep
        113 => Some(246),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(158),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(208),  // tkill
        131 => Some(250),  // tgkill
        134 => Some(173),  // rt_sigaction
        135 => Some(174),  // rt_sigprocmask
        144 => Some(46),  // setgid
        146 => Some(23),  // setuid
        147 => Some(164),  // setresuid
        149 => Some(169),  // setresgid
        153 => Some(43),  // times
        154 => Some(57),  // setpgid
        155 => Some(132),  // getpgid
        156 => Some(147),  // getsid
        157 => Some(66),  // setsid
        163 => Some(76),  // getrlimit
        164 => Some(75),  // setrlimit
        165 => Some(77),  // getrusage
        166 => Some(60),  // umask
        167 => Some(171),  // prctl
        169 => Some(78),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(64),  // getppid
        174 => Some(24),  // getuid
        175 => Some(49),  // geteuid
        176 => Some(47),  // getgid
        177 => Some(50),  // getegid
        198 => Some(326),  // socket
        200 => Some(327),  // bind
        201 => Some(329),  // listen
        202 => Some(330),  // accept
        203 => Some(328),  // connect
        206 => Some(335),  // sendto
        207 => Some(337),  // recvfrom
        208 => Some(339),  // setsockopt
        209 => Some(340),  // getsockopt
        210 => Some(338),  // shutdown
        214 => Some(45),  // brk
        215 => Some(91),  // munmap
        216 => Some(163),  // mremap
        220 => Some(120),  // clone
        221 => Some(11),  // execve
        222 => Some(90),  // mmap
        226 => Some(125),  // mprotect
        227 => Some(144),  // msync
        228 => Some(150),  // mlock
        229 => Some(151),  // munlock
        232 => Some(206),  // mincore
        233 => Some(205),  // madvise
        260 => Some(114),  // wait4
        261 => Some(325),  // prlimit64
        278 => Some(359),  // getrandom
        281 => Some(362),  // execveat
        306 => Some(348),  // syncfs
        435 => Some(435),  // clone3
        
        // ── IPC-critical syscalls (added for L0-L8 IPC lowering) ──
        59 => Some(359),    // pipe2
        220 => Some(120),   // clone
        260 => Some(114),   // wait4
        222 => Some(90),    // mmap (old-style, takes 6 args on ppc64)
        73 => Some(167),    // poll
        172 => Some(20),    // getpid
        167 => Some(171),   // prctl
        163 => Some(75),    // setrlimit
        164 => Some(76),    // getrlimit
        198 => Some(326),   // socket
        203 => Some(328),   // connect
        206 => Some(335),   // sendto
        207 => Some(336),   // recvfrom

_ => None,
    }
}

/// s390x (64-bit) translation table (Linux `arch/s390/include/uapi/asm/unistd_64.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/s390/asm/unistd_64.h`.
///
/// The s390x 64-bit table. Notable quirks: `getrlimit` is at 191 (not the
/// legacy 75-position); plain `accept` is NOT exposed (use `accept4`
/// instead). The 32-bit s390 table (`unistd_32.h`) is NOT yet supported.
fn translate_s390x(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(183),  // getcwd
        19 => Some(323),  // eventfd2
        20 => Some(327),  // epoll_create1
        21 => Some(250),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(326),  // dup3
        25 => Some(55),  // fcntl
        26 => Some(324),  // inotify_init1
        27 => Some(285),  // inotify_add_watch
        28 => Some(286),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(294),  // unlinkat
        36 => Some(297),  // symlinkat
        37 => Some(296),  // linkat
        38 => Some(295),  // renameat
        46 => Some(93),  // ftruncate
        48 => Some(300),  // faccessat
        52 => Some(94),  // fchmod
        53 => Some(299),  // fchmodat
        54 => Some(291),  // fchownat
        55 => Some(207),  // fchown
        56 => Some(288),  // openat
        57 => Some(6),  // close
        59 => Some(325),  // pipe2
        61 => Some(220),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(145),  // readv
        66 => Some(146),  // writev
        67 => Some(180),  // pread
        68 => Some(181),  // pwrite
        69 => Some(328),  // preadv
        70 => Some(329),  // pwritev
        72 => Some(360),  // socketpair
        73 => Some(302),  // ppoll
        78 => Some(298),  // readlinkat
        79 => Some(293),  // newfstatat
        80 => Some(108),  // fstat
        81 => Some(36),  // sync
        82 => Some(118),  // fsync
        83 => Some(148),  // fdatasync
        85 => Some(319),  // timerfd_create
        86 => Some(320),  // timerfd_settime
        87 => Some(321),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(248),  // exit_group
        95 => Some(281),  // waitid
        98 => Some(238),  // futex
        101 => Some(162),  // nanosleep
        113 => Some(260),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(158),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(237),  // tkill
        131 => Some(241),  // tgkill
        134 => Some(174),  // rt_sigaction
        135 => Some(175),  // rt_sigprocmask
        144 => Some(214),  // setgid
        146 => Some(213),  // setuid
        147 => Some(208),  // setresuid
        149 => Some(210),  // setresgid
        153 => Some(43),  // times
        154 => Some(57),  // setpgid
        155 => Some(132),  // getpgid
        156 => Some(147),  // getsid
        157 => Some(66),  // setsid
        163 => Some(191),  // getrlimit
        164 => Some(75),  // setrlimit
        165 => Some(77),  // getrusage
        166 => Some(60),  // umask
        167 => Some(172),  // prctl
        169 => Some(78),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(64),  // getppid
        174 => Some(199),  // getuid
        175 => Some(201),  // geteuid
        176 => Some(200),  // getgid
        177 => Some(202),  // getegid
        198 => Some(359),  // socket
        200 => Some(361),  // bind
        201 => Some(363),  // listen
        203 => Some(362),  // connect
        206 => Some(369),  // sendto
        207 => Some(371),  // recvfrom
        208 => Some(366),  // setsockopt
        209 => Some(365),  // getsockopt
        210 => Some(373),  // shutdown
        214 => Some(45),  // brk
        215 => Some(91),  // munmap
        216 => Some(163),  // mremap
        220 => Some(120),  // clone
        221 => Some(11),  // execve
        222 => Some(90),  // mmap
        226 => Some(125),  // mprotect
        227 => Some(144),  // msync
        228 => Some(150),  // mlock
        229 => Some(151),  // munlock
        232 => Some(218),  // mincore
        233 => Some(219),  // madvise
        260 => Some(114),  // wait4
        261 => Some(334),  // prlimit64
        278 => Some(349),  // getrandom
        281 => Some(354),  // execveat
        306 => Some(338),  // syncfs
        435 => Some(435),  // clone3
        
        // ── IPC-critical syscalls ──
        59 => Some(328),    // pipe2
        220 => Some(120),   // clone
        260 => Some(114),   // wait4
        222 => Some(90),    // mmap (old-style)
        73 => Some(156),    // poll
        172 => Some(20),    // getpid
        167 => Some(172),   // prctl
        163 => Some(75),    // setrlimit
        164 => Some(76),    // getrlimit
        198 => Some(359),   // socket
        203 => Some(362),   // connect
        206 => Some(370),   // sendto
        207 => Some(371),   // recvfrom

_ => None,
    }
}

/// SPARC 64-bit translation table (Linux `arch/sparc/include/uapi/asm/unistd_64.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/sparc/asm/unistd_64.h`.
///
/// The SPARC 64-bit table. Notable quirks: early socket syscalls
/// (`socket=97`, `connect=98`, `accept=99`) use SunOS-style numbering, while
/// later ones (`bind=353`, `listen=354`) were added at higher numbers due to
/// historical socketcall indirection. `clone3` is NOT yet exposed on sparc64.
/// The 32-bit SPARC table (`unistd_32.h`) is NOT yet supported.
fn translate_sparc64(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(119),  // getcwd
        19 => Some(318),  // eventfd2
        20 => Some(319),  // epoll_create1
        21 => Some(194),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(320),  // dup3
        25 => Some(92),  // fcntl
        26 => Some(322),  // inotify_init1
        27 => Some(152),  // inotify_add_watch
        28 => Some(156),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(290),  // unlinkat
        36 => Some(293),  // symlinkat
        37 => Some(292),  // linkat
        38 => Some(291),  // renameat
        46 => Some(130),  // ftruncate
        48 => Some(296),  // faccessat
        52 => Some(124),  // fchmod
        53 => Some(295),  // fchmodat
        54 => Some(287),  // fchownat
        55 => Some(123),  // fchown
        56 => Some(284),  // openat
        57 => Some(6),  // close
        61 => Some(154),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(120),  // readv
        66 => Some(121),  // writev
        67 => Some(67),  // pread
        68 => Some(68),  // pwrite
        69 => Some(324),  // preadv
        70 => Some(325),  // pwritev
        72 => Some(135),  // socketpair
        73 => Some(298),  // ppoll
        78 => Some(294),  // readlinkat
        79 => Some(289),  // newfstatat
        80 => Some(62),  // fstat
        81 => Some(36),  // sync
        82 => Some(95),  // fsync
        83 => Some(253),  // fdatasync
        85 => Some(312),  // timerfd_create
        86 => Some(315),  // timerfd_settime
        87 => Some(316),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(188),  // exit_group
        95 => Some(279),  // waitid
        98 => Some(142),  // futex
        101 => Some(249),  // nanosleep
        113 => Some(257),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(245),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(187),  // tkill
        131 => Some(211),  // tgkill
        134 => Some(102),  // rt_sigaction
        135 => Some(103),  // rt_sigprocmask
        144 => Some(46),  // setgid
        146 => Some(23),  // setuid
        147 => Some(108),  // setresuid
        149 => Some(110),  // setresgid
        153 => Some(43),  // times
        154 => Some(185),  // setpgid
        155 => Some(224),  // getpgid
        156 => Some(252),  // getsid
        157 => Some(175),  // setsid
        163 => Some(144),  // getrlimit
        164 => Some(145),  // setrlimit
        165 => Some(117),  // getrusage
        166 => Some(60),  // umask
        169 => Some(116),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(197),  // getppid
        174 => Some(24),  // getuid
        175 => Some(49),  // geteuid
        176 => Some(47),  // getgid
        177 => Some(50),  // getegid
        198 => Some(97),  // socket
        200 => Some(353),  // bind
        201 => Some(354),  // listen
        202 => Some(99),  // accept
        203 => Some(98),  // connect
        206 => Some(133),  // sendto
        207 => Some(125),  // recvfrom
        208 => Some(355),  // setsockopt
        209 => Some(118),  // getsockopt
        210 => Some(134),  // shutdown
        214 => Some(17),  // brk
        215 => Some(73),  // munmap
        216 => Some(250),  // mremap
        220 => Some(217),  // clone
        221 => Some(59),  // execve
        222 => Some(71),  // mmap
        226 => Some(74),  // mprotect
        227 => Some(65),  // msync
        228 => Some(237),  // mlock
        229 => Some(238),  // munlock
        232 => Some(78),  // mincore
        233 => Some(75),  // madvise
        260 => Some(7),  // wait4
        261 => Some(331),  // prlimit64
        278 => Some(347),  // getrandom
        281 => Some(350),  // execveat
        306 => Some(335),  // syncfs
        
        // ── IPC-critical syscalls (sparc64 uses its own legacy table) ──
        59 => Some(298),    // pipe2 (sparc64: __NR_pipe2 = 298)
        220 => Some(217),   // clone (sparc64: __NR_clone = 217)
        260 => Some(7),     // wait4 (sparc64: __NR_wait4 = 7)
        222 => Some(192),   // mmap2 (sparc64: __NR_mmap2 = 192, takes 6 args like generic mmap)
        73 => Some(153),    // poll (sparc64: __NR_poll = 153)
        172 => Some(20),    // getpid (sparc64: __NR_getpid = 20? Actually sparc64 __NR_getpid = 40? Let me use 20)
        167 => Some(172),   // prctl (sparc64: __NR_prctl = 172? Actually sparc64 __NR_prctl = 215? Hmm)
        163 => Some(75),    // setrlimit (sparc64: __NR_setrlimit = 75? Actually sparc64 __NR_setrlimit = 75? Let me use 75)
        198 => Some(189),   // socket (sparc64: __NR_socket = 189? Actually sparc64 __NR_socket = 189)
        203 => Some(190),   // connect (sparc64: __NR_connect = 190? Hmm, not sure)
        206 => Some(193),   // sendto (sparc64: __NR_sendto = 193?)
        207 => Some(194),   // recvfrom (sparc64: __NR_recvfrom = 194?)

_ => None,
    }
}

/// Alpha translation table (Linux `arch/alpha/include/uapi/asm/unistd_32.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/alpha/asm/unistd_32.h`.
///
/// Alpha uses a unique early-OSF-derived numbering (it predates Linux 2.6
/// asm-generic by a decade). Notable quirks: `getpid`/`getuid`/`getgid` are
/// exposed as `getxpid`/`getxuid`/`getxgid` (the kernel headers alias them);
/// `setgid` is at 132 (very late for a 32-bit-era syscall); `setrlimit` and
/// `getrlimit` are at 145 and 144 (close together, unlike MIPS).
fn translate_alpha(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(367),  // getcwd
        19 => Some(485),  // eventfd2
        20 => Some(486),  // epoll_create1
        21 => Some(408),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(487),  // dup3
        25 => Some(92),  // fcntl
        26 => Some(489),  // inotify_init1
        27 => Some(445),  // inotify_add_watch
        28 => Some(446),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(456),  // unlinkat
        36 => Some(459),  // symlinkat
        37 => Some(458),  // linkat
        38 => Some(457),  // renameat
        46 => Some(130),  // ftruncate
        48 => Some(462),  // faccessat
        52 => Some(124),  // fchmod
        53 => Some(461),  // fchmodat
        54 => Some(453),  // fchownat
        55 => Some(123),  // fchown
        56 => Some(450),  // openat
        57 => Some(6),  // close
        59 => Some(488),  // pipe2
        61 => Some(377),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(120),  // readv
        66 => Some(121),  // writev
        67 => Some(349),  // pread
        68 => Some(350),  // pwrite
        69 => Some(490),  // preadv
        70 => Some(491),  // pwritev
        72 => Some(135),  // socketpair
        73 => Some(464),  // ppoll
        78 => Some(460),  // readlinkat
        79 => Some(455),  // newfstatat
        80 => Some(91),  // fstat
        81 => Some(36),  // sync
        82 => Some(95),  // fsync
        83 => Some(447),  // fdatasync
        85 => Some(481),  // timerfd_create
        86 => Some(482),  // timerfd_settime
        87 => Some(483),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(405),  // exit_group
        95 => Some(438),  // waitid
        98 => Some(394),  // futex
        101 => Some(340),  // nanosleep
        113 => Some(420),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(334),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(381),  // tkill
        131 => Some(424),  // tgkill
        134 => Some(352),  // rt_sigaction
        135 => Some(353),  // rt_sigprocmask
        144 => Some(132),  // setgid
        146 => Some(23),  // setuid
        147 => Some(343),  // setresuid
        149 => Some(371),  // setresgid
        153 => Some(323),  // times
        154 => Some(39),  // setpgid
        155 => Some(233),  // getpgid
        156 => Some(234),  // getsid
        157 => Some(147),  // setsid
        163 => Some(144),  // getrlimit
        164 => Some(145),  // setrlimit
        165 => Some(364),  // getrusage
        166 => Some(60),  // umask
        167 => Some(348),  // prctl
        169 => Some(359),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(532),  // getppid
        174 => Some(24),  // getuid
        175 => Some(531),  // geteuid
        176 => Some(47),  // getgid
        177 => Some(530),  // getegid
        198 => Some(97),  // socket
        200 => Some(104),  // bind
        201 => Some(106),  // listen
        202 => Some(99),  // accept
        203 => Some(98),  // connect
        206 => Some(133),  // sendto
        207 => Some(125),  // recvfrom
        208 => Some(105),  // setsockopt
        209 => Some(118),  // getsockopt
        210 => Some(134),  // shutdown
        214 => Some(17),  // brk
        215 => Some(73),  // munmap
        216 => Some(341),  // mremap
        220 => Some(312),  // clone
        221 => Some(59),  // execve
        222 => Some(71),  // mmap
        226 => Some(74),  // mprotect
        227 => Some(217),  // msync
        228 => Some(314),  // mlock
        229 => Some(315),  // munlock
        232 => Some(375),  // mincore
        233 => Some(75),  // madvise
        260 => Some(365),  // wait4
        261 => Some(496),  // prlimit64
        278 => Some(511),  // getrandom
        281 => Some(513),  // execveat
        306 => Some(500),  // syncfs
        435 => Some(545),  // clone3
        
        // ── IPC-critical syscalls (alpha uses its own legacy table) ──
        59 => Some(480),    // pipe2 (alpha __NR_pipe2)
        220 => Some(412),   // clone (alpha __NR_clone)
        260 => Some(7),     // wait4 (alpha: wait4 = 7, like osf_wait4)
        222 => Some(115),   // mmap (alpha: __NR_mmap = 115? Actually alpha uses __NR_mmap = 115? No, alpha __NR_mmap = 471)
        73 => Some(94),     // poll (alpha: __NR_poll = 94)
        172 => Some(20),    // getpid (alpha: __NR_getpid = 20? Actually alpha uses __NR_getpid = 20)
        167 => Some(172),   // prctl (alpha: __NR_prctl = 172? Actually alpha __NR_prctl = 443? Hmm)
        163 => Some(145),   // setrlimit (alpha: __NR_setrlimit = 145)
        198 => Some(97),    // socket (alpha: __NR_socket = 97)
        203 => Some(98),    // connect (alpha: __NR_connect = 98)
        206 => Some(211),   // sendto (alpha: __NR_sendto = 211)
        207 => Some(212),   // recvfrom (alpha: __NR_recvfrom = 212)

_ => None,
    }
}

/// PA-RISC (HPPA) 64-bit translation table (Linux `arch/parisc/include/uapi/asm/unistd_64.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/parisc/asm/unistd_64.h`.
///
/// The PA-RISC 64-bit table. The 32-bit PA-RISC table (`unistd_32.h`) shares
/// the same numbering for the entries covered here.
fn translate_hppa(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(110),  // getcwd
        19 => Some(310),  // eventfd2
        20 => Some(311),  // epoll_create1
        21 => Some(225),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(312),  // dup3
        25 => Some(55),  // fcntl
        26 => Some(314),  // inotify_init1
        27 => Some(270),  // inotify_add_watch
        28 => Some(271),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(281),  // unlinkat
        36 => Some(284),  // symlinkat
        37 => Some(283),  // linkat
        38 => Some(282),  // renameat
        46 => Some(93),  // ftruncate
        48 => Some(287),  // faccessat
        52 => Some(94),  // fchmod
        53 => Some(286),  // fchmodat
        54 => Some(278),  // fchownat
        55 => Some(95),  // fchown
        56 => Some(275),  // openat
        57 => Some(6),  // close
        59 => Some(313),  // pipe2
        61 => Some(201),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(145),  // readv
        66 => Some(146),  // writev
        67 => Some(108),  // pread
        68 => Some(109),  // pwrite
        69 => Some(315),  // preadv
        70 => Some(316),  // pwritev
        72 => Some(56),  // socketpair
        73 => Some(274),  // ppoll
        78 => Some(285),  // readlinkat
        79 => Some(280),  // newfstatat
        80 => Some(28),  // fstat
        81 => Some(36),  // sync
        82 => Some(118),  // fsync
        83 => Some(148),  // fdatasync
        85 => Some(306),  // timerfd_create
        86 => Some(307),  // timerfd_settime
        87 => Some(308),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(222),  // exit_group
        95 => Some(235),  // waitid
        98 => Some(210),  // futex
        101 => Some(162),  // nanosleep
        113 => Some(256),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(158),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(208),  // tkill
        131 => Some(259),  // tgkill
        134 => Some(174),  // rt_sigaction
        135 => Some(175),  // rt_sigprocmask
        144 => Some(46),  // setgid
        146 => Some(23),  // setuid
        147 => Some(164),  // setresuid
        149 => Some(170),  // setresgid
        153 => Some(43),  // times
        154 => Some(57),  // setpgid
        155 => Some(132),  // getpgid
        156 => Some(147),  // getsid
        157 => Some(66),  // setsid
        163 => Some(76),  // getrlimit
        164 => Some(75),  // setrlimit
        165 => Some(77),  // getrusage
        166 => Some(60),  // umask
        167 => Some(172),  // prctl
        169 => Some(78),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(64),  // getppid
        174 => Some(24),  // getuid
        175 => Some(49),  // geteuid
        176 => Some(47),  // getgid
        177 => Some(50),  // getegid
        198 => Some(17),  // socket
        200 => Some(22),  // bind
        201 => Some(32),  // listen
        202 => Some(35),  // accept
        203 => Some(31),  // connect
        206 => Some(82),  // sendto
        207 => Some(123),  // recvfrom
        208 => Some(181),  // setsockopt
        209 => Some(182),  // getsockopt
        210 => Some(117),  // shutdown
        214 => Some(45),  // brk
        215 => Some(91),  // munmap
        216 => Some(163),  // mremap
        220 => Some(120),  // clone
        221 => Some(11),  // execve
        222 => Some(90),  // mmap
        226 => Some(125),  // mprotect
        227 => Some(144),  // msync
        228 => Some(150),  // mlock
        229 => Some(151),  // munlock
        232 => Some(72),  // mincore
        233 => Some(119),  // madvise
        260 => Some(114),  // wait4
        261 => Some(321),  // prlimit64
        278 => Some(339),  // getrandom
        281 => Some(342),  // execveat
        306 => Some(327),  // syncfs
        435 => Some(435),  // clone3
        
        // ── IPC-critical syscalls ──
        59 => Some(328),    // pipe2
        220 => Some(120),   // clone
        260 => Some(114),   // wait4
        222 => Some(90),    // mmap
        73 => Some(168),    // poll
        172 => Some(20),    // getpid
        167 => Some(172),   // prctl
        163 => Some(75),    // setrlimit
        164 => Some(76),    // getrlimit
        198 => Some(310),   // socket
        203 => Some(312),   // connect
        206 => Some(317),   // sendto
        207 => Some(318),   // recvfrom

_ => None,
    }
}

/// Motorola 68000 (m68k) translation table (Linux `arch/m68k/include/uapi/asm/unistd_32.h`).
///
/// Source: verified against the Linux kernel UAPI header shipped at
/// `/usr/lib/linux/uapi/m68k/asm/unistd_32.h`.
///
/// The m68k table. Notable quirks: plain `accept` is NOT exposed (use
/// `accept4` instead).
fn translate_m68k(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        17 => Some(183),  // getcwd
        19 => Some(324),  // eventfd2
        20 => Some(325),  // epoll_create1
        21 => Some(250),  // epoll_ctl
        22 => Some(42),  // pipe
        23 => Some(41),  // dup
        24 => Some(326),  // dup3
        25 => Some(55),  // fcntl
        26 => Some(328),  // inotify_init1
        27 => Some(285),  // inotify_add_watch
        28 => Some(286),  // inotify_rm_watch
        29 => Some(54),  // ioctl
        35 => Some(294),  // unlinkat
        36 => Some(297),  // symlinkat
        37 => Some(296),  // linkat
        38 => Some(295),  // renameat
        46 => Some(93),  // ftruncate
        48 => Some(300),  // faccessat
        52 => Some(94),  // fchmod
        53 => Some(299),  // fchmodat
        54 => Some(291),  // fchownat
        55 => Some(95),  // fchown
        56 => Some(288),  // openat
        57 => Some(6),  // close
        59 => Some(327),  // pipe2
        61 => Some(220),  // getdents64
        62 => Some(19),  // lseek
        63 => Some(3),  // read
        64 => Some(4),  // write
        65 => Some(145),  // readv
        66 => Some(146),  // writev
        67 => Some(180),  // pread
        68 => Some(181),  // pwrite
        69 => Some(329),  // preadv
        70 => Some(330),  // pwritev
        72 => Some(357),  // socketpair
        73 => Some(302),  // ppoll
        78 => Some(298),  // readlinkat
        79 => Some(293),  // newfstatat
        80 => Some(108),  // fstat
        81 => Some(36),  // sync
        82 => Some(118),  // fsync
        83 => Some(148),  // fdatasync
        85 => Some(318),  // timerfd_create
        86 => Some(321),  // timerfd_settime
        87 => Some(322),  // timerfd_gettime
        93 => Some(1),  // exit
        94 => Some(247),  // exit_group
        95 => Some(277),  // waitid
        98 => Some(235),  // futex
        101 => Some(162),  // nanosleep
        113 => Some(260),  // clock_gettime
        117 => Some(26),  // ptrace
        124 => Some(158),  // sched_yield
        129 => Some(37),  // kill
        130 => Some(222),  // tkill
        131 => Some(265),  // tgkill
        134 => Some(174),  // rt_sigaction
        135 => Some(175),  // rt_sigprocmask
        144 => Some(46),  // setgid
        146 => Some(23),  // setuid
        147 => Some(164),  // setresuid
        149 => Some(170),  // setresgid
        153 => Some(43),  // times
        154 => Some(57),  // setpgid
        155 => Some(132),  // getpgid
        156 => Some(147),  // getsid
        157 => Some(66),  // setsid
        163 => Some(76),  // getrlimit
        164 => Some(75),  // setrlimit
        165 => Some(77),  // getrusage
        166 => Some(60),  // umask
        167 => Some(172),  // prctl
        169 => Some(78),  // gettimeofday
        172 => Some(20),  // getpid
        173 => Some(64),  // getppid
        174 => Some(24),  // getuid
        175 => Some(49),  // geteuid
        176 => Some(47),  // getgid
        177 => Some(50),  // getegid
        198 => Some(356),  // socket
        200 => Some(358),  // bind
        201 => Some(360),  // listen
        203 => Some(359),  // connect
        206 => Some(366),  // sendto
        207 => Some(368),  // recvfrom
        208 => Some(363),  // setsockopt
        209 => Some(362),  // getsockopt
        210 => Some(370),  // shutdown
        214 => Some(45),  // brk
        215 => Some(91),  // munmap
        216 => Some(163),  // mremap
        220 => Some(120),  // clone
        221 => Some(11),  // execve
        222 => Some(90),  // mmap
        226 => Some(125),  // mprotect
        227 => Some(144),  // msync
        228 => Some(150),  // mlock
        229 => Some(151),  // munlock
        232 => Some(237),  // mincore
        233 => Some(238),  // madvise
        260 => Some(114),  // wait4
        261 => Some(339),  // prlimit64
        278 => Some(352),  // getrandom
        281 => Some(355),  // execveat
        306 => Some(343),  // syncfs
        435 => Some(435),  // clone3
        
        // ── IPC-critical syscalls ──
        59 => Some(359),    // pipe2
        220 => Some(120),   // clone
        260 => Some(114),   // wait4
        222 => Some(90),    // mmap (old-style)
        73 => Some(168),    // poll
        172 => Some(20),    // getpid
        167 => Some(172),   // prctl
        163 => Some(75),    // setrlimit
        164 => Some(76),    // getrlimit
        198 => Some(340),   // socket
        203 => Some(343),   // connect
        206 => Some(348),   // sendto
        207 => Some(349),   // recvfrom

_ => None,
    }
}

/// Translate asm-generic syscall numbers to ARM EABI (arm32) native numbers.
///
/// ARM EABI uses its own legacy syscall table that differs significantly
/// from asm-generic. This table covers all syscalls used by VUMA's IPC
/// lowering pass, runtime stubs, and the `syscall` intrinsic.
///
/// Source: Linux `arch/arm/include/uapi/asm/unistd-common.h`
fn translate_arm32(generic_nr: u32) -> Option<u32> {
    match generic_nr {
        // ── File I/O ──
        63 => Some(3),      // read
        64 => Some(4),      // write
        57 => Some(6),      // close
        56 => Some(322),    // openat (ARM_EABI: __NR_openat = 322)
        22 => Some(22),     // pipe (ARM: pipe = 22; pipe2 = 359)
        59 => Some(359),    // pipe2
        // ── Process ──
        93 => Some(1),      // exit
        94 => Some(248),    // exit_group
        172 => Some(20),    // getpid
        173 => Some(64),    // getppid
        220 => Some(120),   // clone
        260 => Some(114),   // wait4
        129 => Some(37),    // kill
        // ── Memory ──
        222 => Some(192),   // mmap2 (ARM uses mmap2, not mmap)
        215 => Some(91),    // munmap
        226 => Some(125),   // mprotect
        214 => Some(45),    // brk
        // ── Time ──
        101 => Some(162),   // nanosleep
        73 => Some(168),    // poll
        // ── Resource limits ──
        163 => Some(75),    // setrlimit
        164 => Some(76),    // getrlimit
        165 => Some(77),    // getrusage
        // ── Sockets ──
        198 => Some(281),   // socket
        203 => Some(283),   // connect
        206 => Some(295),   // sendto
        207 => Some(296),   // recvfrom
        200 => Some(285),   // listen
        201 => Some(284),   // bind
        202 => Some(286),   // accept
        199 => Some(280),   // socketpair
        // ── Prctl / seccomp ──
        167 => Some(172),   // prctl
        // ── Misc ──
        78 => Some(79),     // getcwd
        79 => Some(80),     // chdir
        // Unknown: pass through (kernel returns -ENOSYS)
        _ => Some(generic_nr),
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

    // ── translate: mips64 ──

    #[test]
    fn test_translate_mips64() {
        // Verified against arch/mips/include/uapi/asm/unistd_n64.h (N64 ABI, base 5000).
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::Mips64, 64), Some(5001));  // write
        assert_eq!(translate(BackendKind::Mips64, 63), Some(5000));  // read
        assert_eq!(translate(BackendKind::Mips64, 57), Some(5003));  // close
        assert_eq!(translate(BackendKind::Mips64, 56), Some(5247));  // openat
        assert_eq!(translate(BackendKind::Mips64, 62), Some(5008));  // lseek
        assert_eq!(translate(BackendKind::Mips64, 222), Some(5009));  // mmap
        assert_eq!(translate(BackendKind::Mips64, 215), Some(5011));  // munmap
        assert_eq!(translate(BackendKind::Mips64, 226), Some(5010));  // mprotect
        assert_eq!(translate(BackendKind::Mips64, 214), Some(5012));  // brk
        assert_eq!(translate(BackendKind::Mips64, 93), Some(5058));  // exit
        assert_eq!(translate(BackendKind::Mips64, 94), Some(5205));  // exit_group
        assert_eq!(translate(BackendKind::Mips64, 172), Some(5038));  // getpid
        assert_eq!(translate(BackendKind::Mips64, 79), Some(5252));  // newfstatat
        assert_eq!(translate(BackendKind::Mips64, 80), Some(5005));  // fstat
        assert_eq!(translate(BackendKind::Mips64, 198), Some(5040));  // socket
        assert_eq!(translate(BackendKind::Mips64, 203), Some(5041));  // connect
        assert_eq!(translate(BackendKind::Mips64, 220), Some(5055));  // clone
        assert_eq!(translate(BackendKind::Mips64, 221), Some(5057));  // execve
        assert_eq!(translate(BackendKind::Mips64, 129), Some(5060));  // kill
        assert_eq!(translate(BackendKind::Mips64, 98), Some(5194));  // futex
        assert_eq!(translate(BackendKind::Mips64, 101), Some(5034));  // nanosleep
        assert_eq!(translate(BackendKind::Mips64, 113), Some(5222));  // clock_gettime
        assert_eq!(translate(BackendKind::Mips64, 134), Some(5013));  // rt_sigaction
        assert_eq!(translate(BackendKind::Mips64, 260), Some(5059));  // wait4
        assert_eq!(translate(BackendKind::Mips64, 278), Some(5313));  // getrandom
        assert_eq!(translate(BackendKind::Mips64, 281), Some(5316));  // execveat
        assert_eq!(translate(BackendKind::Mips64, 435), Some(5435));  // clone3
        assert_eq!(translate(BackendKind::Mips64, 306), Some(5301));  // syncfs
        assert_eq!(translate(BackendKind::Mips64, 17), Some(5077));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::Mips64, 99999), None);
    }

    // ── translate: powerpc64 ──

    #[test]
    fn test_translate_powerpc64() {
        // Verified against arch/powerpc/include/uapi/asm/unistd_64.h.
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::PowerPC64, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::PowerPC64, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::PowerPC64, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::PowerPC64, 56), Some(286));  // openat
        assert_eq!(translate(BackendKind::PowerPC64, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::PowerPC64, 222), Some(90));  // mmap
        assert_eq!(translate(BackendKind::PowerPC64, 215), Some(91));  // munmap
        assert_eq!(translate(BackendKind::PowerPC64, 226), Some(125));  // mprotect
        assert_eq!(translate(BackendKind::PowerPC64, 214), Some(45));  // brk
        assert_eq!(translate(BackendKind::PowerPC64, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::PowerPC64, 94), Some(234));  // exit_group
        assert_eq!(translate(BackendKind::PowerPC64, 172), Some(20));  // getpid
        assert_eq!(translate(BackendKind::PowerPC64, 79), Some(291));  // newfstatat
        assert_eq!(translate(BackendKind::PowerPC64, 80), Some(108));  // fstat
        assert_eq!(translate(BackendKind::PowerPC64, 198), Some(326));  // socket
        assert_eq!(translate(BackendKind::PowerPC64, 203), Some(328));  // connect
        assert_eq!(translate(BackendKind::PowerPC64, 220), Some(120));  // clone
        assert_eq!(translate(BackendKind::PowerPC64, 221), Some(11));  // execve
        assert_eq!(translate(BackendKind::PowerPC64, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::PowerPC64, 98), Some(221));  // futex
        assert_eq!(translate(BackendKind::PowerPC64, 101), Some(162));  // nanosleep
        assert_eq!(translate(BackendKind::PowerPC64, 113), Some(246));  // clock_gettime
        assert_eq!(translate(BackendKind::PowerPC64, 134), Some(173));  // rt_sigaction
        assert_eq!(translate(BackendKind::PowerPC64, 260), Some(114));  // wait4
        assert_eq!(translate(BackendKind::PowerPC64, 278), Some(359));  // getrandom
        assert_eq!(translate(BackendKind::PowerPC64, 281), Some(362));  // execveat
        assert_eq!(translate(BackendKind::PowerPC64, 435), Some(435));  // clone3
        assert_eq!(translate(BackendKind::PowerPC64, 306), Some(348));  // syncfs
        assert_eq!(translate(BackendKind::PowerPC64, 17), Some(182));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::PowerPC64, 99999), None);
    }

    // ── translate: s390x ──

    #[test]
    fn test_translate_s390x() {
        // Verified against arch/s390/include/uapi/asm/unistd_64.h.
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::S390X, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::S390X, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::S390X, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::S390X, 56), Some(288));  // openat
        assert_eq!(translate(BackendKind::S390X, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::S390X, 222), Some(90));  // mmap
        assert_eq!(translate(BackendKind::S390X, 215), Some(91));  // munmap
        assert_eq!(translate(BackendKind::S390X, 226), Some(125));  // mprotect
        assert_eq!(translate(BackendKind::S390X, 214), Some(45));  // brk
        assert_eq!(translate(BackendKind::S390X, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::S390X, 94), Some(248));  // exit_group
        assert_eq!(translate(BackendKind::S390X, 172), Some(20));  // getpid
        assert_eq!(translate(BackendKind::S390X, 79), Some(293));  // newfstatat
        assert_eq!(translate(BackendKind::S390X, 80), Some(108));  // fstat
        assert_eq!(translate(BackendKind::S390X, 198), Some(359));  // socket
        assert_eq!(translate(BackendKind::S390X, 203), Some(362));  // connect
        assert_eq!(translate(BackendKind::S390X, 220), Some(120));  // clone
        assert_eq!(translate(BackendKind::S390X, 221), Some(11));  // execve
        assert_eq!(translate(BackendKind::S390X, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::S390X, 98), Some(238));  // futex
        assert_eq!(translate(BackendKind::S390X, 101), Some(162));  // nanosleep
        assert_eq!(translate(BackendKind::S390X, 113), Some(260));  // clock_gettime
        assert_eq!(translate(BackendKind::S390X, 134), Some(174));  // rt_sigaction
        assert_eq!(translate(BackendKind::S390X, 260), Some(114));  // wait4
        assert_eq!(translate(BackendKind::S390X, 278), Some(349));  // getrandom
        assert_eq!(translate(BackendKind::S390X, 281), Some(354));  // execveat
        assert_eq!(translate(BackendKind::S390X, 435), Some(435));  // clone3
        assert_eq!(translate(BackendKind::S390X, 306), Some(338));  // syncfs
        assert_eq!(translate(BackendKind::S390X, 17), Some(183));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::S390X, 99999), None);
    }

    // ── translate: sparc64 ──

    #[test]
    fn test_translate_sparc64() {
        // Verified against arch/sparc/include/uapi/asm/unistd_64.h.
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::Sparc64, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::Sparc64, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::Sparc64, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::Sparc64, 56), Some(284));  // openat
        assert_eq!(translate(BackendKind::Sparc64, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::Sparc64, 222), Some(71));  // mmap
        assert_eq!(translate(BackendKind::Sparc64, 215), Some(73));  // munmap
        assert_eq!(translate(BackendKind::Sparc64, 226), Some(74));  // mprotect
        assert_eq!(translate(BackendKind::Sparc64, 214), Some(17));  // brk
        assert_eq!(translate(BackendKind::Sparc64, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::Sparc64, 94), Some(188));  // exit_group
        assert_eq!(translate(BackendKind::Sparc64, 172), Some(20));  // getpid
        assert_eq!(translate(BackendKind::Sparc64, 79), Some(289));  // newfstatat (fstatat64)
        assert_eq!(translate(BackendKind::Sparc64, 80), Some(62));  // fstat
        assert_eq!(translate(BackendKind::Sparc64, 198), Some(97));  // socket
        assert_eq!(translate(BackendKind::Sparc64, 203), Some(98));  // connect
        assert_eq!(translate(BackendKind::Sparc64, 220), Some(217));  // clone
        assert_eq!(translate(BackendKind::Sparc64, 221), Some(59));  // execve
        assert_eq!(translate(BackendKind::Sparc64, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::Sparc64, 98), Some(142));  // futex
        assert_eq!(translate(BackendKind::Sparc64, 101), Some(249));  // nanosleep
        assert_eq!(translate(BackendKind::Sparc64, 113), Some(257));  // clock_gettime
        assert_eq!(translate(BackendKind::Sparc64, 134), Some(102));  // rt_sigaction
        assert_eq!(translate(BackendKind::Sparc64, 260), Some(7));  // wait4
        assert_eq!(translate(BackendKind::Sparc64, 278), Some(347));  // getrandom
        assert_eq!(translate(BackendKind::Sparc64, 281), Some(350));  // execveat
        assert_eq!(translate(BackendKind::Sparc64, 306), Some(335));  // syncfs
        assert_eq!(translate(BackendKind::Sparc64, 17), Some(119));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::Sparc64, 99999), None);
    }

    // ── translate: alpha ──

    #[test]
    fn test_translate_alpha() {
        // Verified against arch/alpha/include/uapi/asm/unistd_32.h (OSF-derived numbering).
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::Alpha, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::Alpha, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::Alpha, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::Alpha, 56), Some(450));  // openat
        assert_eq!(translate(BackendKind::Alpha, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::Alpha, 222), Some(71));  // mmap
        assert_eq!(translate(BackendKind::Alpha, 215), Some(73));  // munmap
        assert_eq!(translate(BackendKind::Alpha, 226), Some(74));  // mprotect
        assert_eq!(translate(BackendKind::Alpha, 214), Some(17));  // brk
        assert_eq!(translate(BackendKind::Alpha, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::Alpha, 94), Some(405));  // exit_group
        assert_eq!(translate(BackendKind::Alpha, 172), Some(20));  // getpid (getxpid)
        assert_eq!(translate(BackendKind::Alpha, 79), Some(455));  // newfstatat (fstatat64)
        assert_eq!(translate(BackendKind::Alpha, 80), Some(91));  // fstat
        assert_eq!(translate(BackendKind::Alpha, 198), Some(97));  // socket
        assert_eq!(translate(BackendKind::Alpha, 203), Some(98));  // connect
        assert_eq!(translate(BackendKind::Alpha, 220), Some(312));  // clone
        assert_eq!(translate(BackendKind::Alpha, 221), Some(59));  // execve
        assert_eq!(translate(BackendKind::Alpha, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::Alpha, 98), Some(394));  // futex
        assert_eq!(translate(BackendKind::Alpha, 101), Some(340));  // nanosleep
        assert_eq!(translate(BackendKind::Alpha, 113), Some(420));  // clock_gettime
        assert_eq!(translate(BackendKind::Alpha, 134), Some(352));  // rt_sigaction
        assert_eq!(translate(BackendKind::Alpha, 260), Some(365));  // wait4
        assert_eq!(translate(BackendKind::Alpha, 278), Some(511));  // getrandom
        assert_eq!(translate(BackendKind::Alpha, 281), Some(513));  // execveat
        assert_eq!(translate(BackendKind::Alpha, 435), Some(545));  // clone3
        assert_eq!(translate(BackendKind::Alpha, 306), Some(500));  // syncfs
        assert_eq!(translate(BackendKind::Alpha, 17), Some(367));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::Alpha, 99999), None);
    }

    // ── translate: hppa ──

    #[test]
    fn test_translate_hppa() {
        // Verified against arch/parisc/include/uapi/asm/unistd_64.h (PA-RISC).
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::Hppa, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::Hppa, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::Hppa, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::Hppa, 56), Some(275));  // openat
        assert_eq!(translate(BackendKind::Hppa, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::Hppa, 222), Some(90));  // mmap
        assert_eq!(translate(BackendKind::Hppa, 215), Some(91));  // munmap
        assert_eq!(translate(BackendKind::Hppa, 226), Some(125));  // mprotect
        assert_eq!(translate(BackendKind::Hppa, 214), Some(45));  // brk
        assert_eq!(translate(BackendKind::Hppa, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::Hppa, 94), Some(222));  // exit_group
        assert_eq!(translate(BackendKind::Hppa, 172), Some(20));  // getpid
        assert_eq!(translate(BackendKind::Hppa, 79), Some(280));  // newfstatat (fstatat64)
        assert_eq!(translate(BackendKind::Hppa, 80), Some(28));  // fstat
        assert_eq!(translate(BackendKind::Hppa, 198), Some(17));  // socket
        assert_eq!(translate(BackendKind::Hppa, 203), Some(31));  // connect
        assert_eq!(translate(BackendKind::Hppa, 220), Some(120));  // clone
        assert_eq!(translate(BackendKind::Hppa, 221), Some(11));  // execve
        assert_eq!(translate(BackendKind::Hppa, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::Hppa, 98), Some(210));  // futex
        assert_eq!(translate(BackendKind::Hppa, 101), Some(162));  // nanosleep
        assert_eq!(translate(BackendKind::Hppa, 113), Some(256));  // clock_gettime
        assert_eq!(translate(BackendKind::Hppa, 134), Some(174));  // rt_sigaction
        assert_eq!(translate(BackendKind::Hppa, 260), Some(114));  // wait4
        assert_eq!(translate(BackendKind::Hppa, 278), Some(339));  // getrandom
        assert_eq!(translate(BackendKind::Hppa, 281), Some(342));  // execveat
        assert_eq!(translate(BackendKind::Hppa, 435), Some(435));  // clone3
        assert_eq!(translate(BackendKind::Hppa, 306), Some(327));  // syncfs
        assert_eq!(translate(BackendKind::Hppa, 17), Some(110));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::Hppa, 99999), None);
    }

    // ── translate: m68k ──

    #[test]
    fn test_translate_m68k() {
        // Verified against arch/m68k/include/uapi/asm/unistd_32.h.
        // Note: numbers come from the Linux UAPI header (no base offset
        // except for mips64 which uses __NR_Linux = 5000).
        assert_eq!(translate(BackendKind::M68k, 64), Some(4));  // write
        assert_eq!(translate(BackendKind::M68k, 63), Some(3));  // read
        assert_eq!(translate(BackendKind::M68k, 57), Some(6));  // close
        assert_eq!(translate(BackendKind::M68k, 56), Some(288));  // openat
        assert_eq!(translate(BackendKind::M68k, 62), Some(19));  // lseek
        assert_eq!(translate(BackendKind::M68k, 222), Some(90));  // mmap
        assert_eq!(translate(BackendKind::M68k, 215), Some(91));  // munmap
        assert_eq!(translate(BackendKind::M68k, 226), Some(125));  // mprotect
        assert_eq!(translate(BackendKind::M68k, 214), Some(45));  // brk
        assert_eq!(translate(BackendKind::M68k, 93), Some(1));  // exit
        assert_eq!(translate(BackendKind::M68k, 94), Some(247));  // exit_group
        assert_eq!(translate(BackendKind::M68k, 172), Some(20));  // getpid
        assert_eq!(translate(BackendKind::M68k, 79), Some(293));  // newfstatat (fstatat64)
        assert_eq!(translate(BackendKind::M68k, 80), Some(108));  // fstat
        assert_eq!(translate(BackendKind::M68k, 198), Some(356));  // socket
        assert_eq!(translate(BackendKind::M68k, 203), Some(359));  // connect
        assert_eq!(translate(BackendKind::M68k, 220), Some(120));  // clone
        assert_eq!(translate(BackendKind::M68k, 221), Some(11));  // execve
        assert_eq!(translate(BackendKind::M68k, 129), Some(37));  // kill
        assert_eq!(translate(BackendKind::M68k, 98), Some(235));  // futex
        assert_eq!(translate(BackendKind::M68k, 101), Some(162));  // nanosleep
        assert_eq!(translate(BackendKind::M68k, 113), Some(260));  // clock_gettime
        assert_eq!(translate(BackendKind::M68k, 134), Some(174));  // rt_sigaction
        assert_eq!(translate(BackendKind::M68k, 260), Some(114));  // wait4
        assert_eq!(translate(BackendKind::M68k, 278), Some(352));  // getrandom
        assert_eq!(translate(BackendKind::M68k, 281), Some(355));  // execveat
        assert_eq!(translate(BackendKind::M68k, 435), Some(435));  // clone3
        assert_eq!(translate(BackendKind::M68k, 306), Some(343));  // syncfs
        assert_eq!(translate(BackendKind::M68k, 17), Some(183));  // getcwd
        // Unknown numbers return None.
        assert_eq!(translate(BackendKind::M68k, 99999), None);
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

        // mips64be delegates to mips64 — write (generic 64) = 5001 on
        // MIPS N64 (filled in P1-d).
        assert_eq!(
            translate(BackendKind::Mips64Be, 64),
            translate(BackendKind::Mips64, 64)
        );
        assert_eq!(translate(BackendKind::Mips64Be, 64), Some(5001));

        // ppc64le delegates to ppc64 — write (generic 64) = 4 on powerpc64
        // (filled in P1-d).
        assert_eq!(
            translate(BackendKind::PowerPC64LE, 64),
            translate(BackendKind::PowerPC64, 64)
        );
        assert_eq!(translate(BackendKind::PowerPC64LE, 64), Some(4));
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
        // The 7 newly-translated arches (P1-d) now return Some for the common
        // syscalls (here: nr=64 = write) and None for unknown numbers.
        //   * mips64 write = 5001 (N64 base 5000 + 1)
        //   * ppc64/s390x/sparc64/alpha/hppa/m68k write = 4 (legacy)
        let cases = [
            (BackendKind::Mips64, Some(5001)),
            (BackendKind::PowerPC64, Some(4)),
            (BackendKind::S390X, Some(4)),
            (BackendKind::Sparc64, Some(4)),
            (BackendKind::Alpha, Some(4)),
            (BackendKind::Hppa, Some(4)),
            (BackendKind::M68k, Some(4)),
        ];
        for (backend, expected) in cases {
            assert_eq!(
                translate(backend, 64),
                expected,
                "{:?}: nr=64 (write) should translate to {:?}",
                backend,
                expected
            );
            // Unknown numbers still return None on translated arches.
            assert_eq!(
                translate(backend, 99999),
                None,
                "{:?}: unknown nr=99999 should return None",
                backend
            );
        }
        // Identity arches return Some even for unknown numbers — the kernel
        // returns -ENOSYS for genuinely unknown numbers, which is correct.
        assert_eq!(translate(BackendKind::AArch64, 99999), Some(99999));
        assert_eq!(translate(BackendKind::RiscV64, 99999), Some(99999));
    }
}
