#!/usr/bin/env bash
# qemu-env.sh — source this to put the 18 VUMA-required `qemu-<isa>-static`
# user-mode emulation binaries on PATH.
# Installed by wave 0, task 0-c-install; re-verified by follow-up F0-c-install.
#
# Caveat §4.2 requires QEMU user-mode ≥ 10.0 for the 18 ISAs listed below.
# Debian 13 (trixie) ships `qemu-user` 1:10.0.11+ds-0+deb13u1, whose binaries
# are statically linked (static-PIE ELF). They were extracted from the .deb
# (no root in sandbox — `apt-get install` is blocked by the dpkg lock) into
# `$HOME/.local/bin/` and exposed under both `qemu-<isa>` and
# `qemu-<isa>-static` names (the latter via symlinks, mirroring Debian's own
# `qemu-user-static` transitional package).
#
# Follow-up F0-c-install (2026-07-30) re-queried the latest stable:
#   - apt trixie/main     : 1:10.0.11+ds-0+deb13u1  <- matches installed
#   - apt trixie-security : 1:10.0.2+ds-2+deb13u1    (older)
#   - upstream qemu.org   : 11.0.3 stable + 11.1.0-rc2 (RC)
# Per protocol step 5, since the apt repo's latest stable == 10.0.11 (already
# installed), the upgrade is a NO-OP. Upstream 11.0.3 would require a source
# build for 18 targets (out of scope: 10-min budget, no ninja/meson/cross-dev,
# no root). All 18 binaries re-verified reporting 10.0.11; runtime smoke
# (`qemu-x86_64-static /bin/true` -> 0) and env-shim smoke test both PASS.
#
# ISAs covered (18):
#   aarch64 aarch64_be alpha arm armeb hppa i386 loongarch64 m68k
#   mips64 mips64el ppc64 ppc64le riscv32 riscv64 s390x sparc64 x86_64
#
# Usage:
#   source scripts/env/qemu-env.sh
#   qemu-x86_64-static --version | head -1   # -> qemu-x86_64 version 10.0.11 (...)
#   which qemu-aarch64-static                 # -> /home/z/.local/bin/qemu-aarch64-static

# Idempotent: only prepend $HOME/.local/bin if not already on PATH.
case ":${PATH}:" in
    *":${HOME}/.local/bin:"*)
        # Already on PATH; nothing to do.
        ;;
    *)
        export PATH="${HOME}/.local/bin:${PATH}"
        ;;
esac

# Sanity-check that at least the x86_64 binary is reachable. This is a
# no-op if everything is in place; it prints a one-line warning otherwise.
if ! command -v qemu-x86_64-static >/dev/null 2>&1; then
    echo "qemu-env.sh: WARNING: qemu-x86_64-static not found on PATH" \
         "(expected in $HOME/.local/bin)" >&2
fi
