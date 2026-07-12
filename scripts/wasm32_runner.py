#!/usr/bin/env python3
"""VUMA wasm32 custom wasmtime runner.

Provides WASI functions plus custom "vuma" host functions for POSIX
operations that WASI does not support (pipe, fork, execve, dup2, waitpid,
strcmp).  The host functions use the real OS, bridging the wasm sandbox
and the host process model so that programs like self_exec.vuma (which
fork+exec+pipe) work correctly on wasm32.

Usage:
    python3 wasm32_runner.py <wasm_file> [args...]

The return value of _vuma_main is used as the process exit code.
"""

import sys
import os
import struct
import ctypes

try:
    from wasmtime import (
        Engine, Store, Module, Linker, Func, FuncType, ValType,
        Trap, WasiConfig,
    )
except ImportError:
    print("Error: wasmtime Python package not installed", file=sys.stderr)
    sys.exit(1)


def make_host_functions(store, memory):
    """Create the custom 'vuma' host functions that access wasm linear memory."""

    def read_mem(ptr, length):
        """Read `length` bytes from wasm memory at `ptr`."""
        buf = memory.read(store, ptr, ptr + length)
        return buf

    def write_mem(ptr, data):
        """Write `data` bytes to wasm memory at `ptr`."""
        memory.write(store, data, ptr)

    def read_cstr(ptr):
        """Read a NUL-terminated string from wasm memory at `ptr`."""
        result = bytearray()
        addr = ptr
        while True:
            b = memory.read(store, addr, addr + 1)
            if not b or b[0] == 0:
                break
            result.append(b[0])
            addr += 1
        return bytes(result)

    def read_ptr_array(ptr):
        """Read a NUL-terminated array of pointers (32-bit LE) from wasm memory."""
        ptrs = []
        addr = ptr
        while True:
            raw = memory.read(store, addr, addr + 4)
            if len(raw) < 4:
                break
            val = struct.unpack('<I', raw)[0]
            if val == 0:
                break
            ptrs.append(val)
            addr += 4
        return ptrs

    # ── pipe(pipefd_ptr: i32) -> i32 ──────────────────────────────────
    # Creates a pipe.  Writes two 32-bit fds (native byte order) to the
    # buffer at pipefd_ptr.  Returns 0 on success, -1 on error.
    def vuma_pipe(pipefd_ptr):
        try:
            r, w = os.pipe()
            # Write fds as native 32-bit integers (the VUMA program reads
            # them with read_i32_native, which handles both LE and BE).
            # wasm32 is always little-endian; write fds as LE 32-bit ints
            write_mem(pipefd_ptr, struct.pack('<ii', r, w))
            return 0
        except OSError:
            return -1

    # ── fork() -> i32 ─────────────────────────────────────────────────
    # Returns 0 in child, child PID in parent, -1 on error.
    def vuma_fork():
        try:
            # Suppress the deprecation warning about multi-threaded fork.
            # wasmtime uses background threads, but fork() is safe as long
            # as the child immediately calls execve (which our VUMA code does).
            import warnings
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                return os.fork()
        except OSError:
            return -1

    # ── execve(path_ptr, argv_ptr, envp_ptr) -> i32 ───────────────────
    # Replaces the current process.  Re-invokes this runner with the wasm
    # module and new argv so the child runs in a fresh wasmtime instance.
    # Does not return on success.
    def vuma_execve(path_ptr, argv_ptr, envp_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            argv_ptrs = read_ptr_array(argv_ptr)
            envp_ptrs = read_ptr_array(envp_ptr)
            argv = [read_cstr(p).decode('utf-8', errors='replace') for p in argv_ptrs]
            envp = [read_cstr(p).decode('utf-8', errors='replace') for p in envp_ptrs]
            # The path points to the .wasm file.  We re-invoke this runner
            # so the child gets a fresh wasmtime instance with the host
            # functions.  argv[0] is the wasm path; argv[1:] are passed as
            # WASI args to the child module.
            runner = os.path.abspath(__file__)
            new_argv = [sys.executable, runner, path] + argv[1:]
            env = os.environ.copy()
            # Update env with any envp entries
            for e in envp:
                if '=' in e:
                    k, v = e.split('=', 1)
                    env[k] = v
            os.execvpe(sys.executable, new_argv, env)
            return -1  # unreachable on success
        except OSError:
            return -1

    # ── dup2(oldfd, newfd) -> i32 ─────────────────────────────────────
    def vuma_dup2(oldfd, newfd):
        try:
            return os.dup2(oldfd, newfd)
        except OSError:
            return -1

    # ── waitpid(pid, status_ptr, options) -> i32 ──────────────────────
    # Waits for child.  Writes 32-bit status to status_ptr.  Returns child PID.
    def vuma_waitpid(pid, status_ptr, options):
        try:
            result = os.waitpid(pid, options)
            # wasm32 is always little-endian; write status as LE 32-bit int
            write_mem(status_ptr, struct.pack('<i', result[1]))
            return result[0]
        except OSError:
            return -1

    # ── strcmp(s1_ptr, s2_ptr) -> i32 ─────────────────────────────────
    # Compares two NUL-terminated strings.  Returns difference of first
    # differing byte (0 if equal).
    def vuma_strcmp(s1_ptr, s2_ptr):
        s1 = read_cstr(s1_ptr)
        s2 = read_cstr(s2_ptr)
        for i in range(max(len(s1), len(s2)) + 1):
            b1 = s1[i] if i < len(s1) else 0
            b2 = s2[i] if i < len(s2) else 0
            if b1 != b2:
                return b1 - b2
        return 0

    # Build Func objects with correct types
    i32 = ValType.i32()

    funcs = {
        'pipe':    Func(store, FuncType([i32], [i32]), vuma_pipe),
        'fork':    Func(store, FuncType([], [i32]), vuma_fork),
        'execve':  Func(store, FuncType([i32, i32, i32], [i32]), vuma_execve),
        'dup2':    Func(store, FuncType([i32, i32], [i32]), vuma_dup2),
        'waitpid': Func(store, FuncType([i32, i32, i32], [i32]), vuma_waitpid),
        'strcmp':  Func(store, FuncType([i32, i32], [i32]), vuma_strcmp),
    }
    return funcs


def main():
    if len(sys.argv) < 2:
        print("Usage: wasm32_runner.py <wasm_file> [args...]", file=sys.stderr)
        sys.exit(1)

    wasm_path = sys.argv[1]
    wasi_args = sys.argv[1:]  # argv[0] = wasm path, argv[1:] = extra args

    engine = Engine()
    store = Store(engine)

    # Configure WASI with command-line arguments
    wasi = WasiConfig()
    wasi.argv = wasi_args
    # Inherit stdin/stdout/stderr from the host
    wasi.inherit_stdout()
    wasi.inherit_stderr()
    wasi.inherit_stdin()
    store.set_wasi(wasi)

    # Load the wasm module
    module = Module.from_file(engine, wasm_path)

    # Create a linker with WASI support
    linker = Linker(engine)
    linker.define_wasi()

    # We need the memory to create host functions, but memory is only
    # available after instantiation.  Use a two-pass approach: define
    # placeholder functions that will be replaced, or use a closure
    # that captures a mutable reference.
    #
    # Actually, wasmtime Func objects can capture state via closures.
    # We create a "memory holder" that gets filled after instantiation.
    mem_holder = [None]

    def get_mem():
        return mem_holder[0]

    # Recreate host functions using the holder pattern
    def read_cstr(ptr):
        mem = get_mem()
        if mem is None:
            return b''
        result = bytearray()
        addr = ptr
        while True:
            b = mem.read(store, addr, addr + 1)
            if not b or b[0] == 0:
                break
            result.append(b[0])
            addr += 1
        return bytes(result)

    def write_mem(ptr, data):
        mem = get_mem()
        if mem is not None:
            mem.write(store, data, ptr)

    def read_ptr_array(ptr):
        mem = get_mem()
        if mem is None:
            return []
        ptrs = []
        addr = ptr
        while True:
            raw = mem.read(store, addr, addr + 4)
            if len(raw) < 4:
                break
            val = struct.unpack('<I', raw)[0]
            if val == 0:
                break
            ptrs.append(val)
            addr += 4
        return ptrs

    def vuma_pipe(pipefd_ptr):
        try:
            r, w = os.pipe()
            write_mem(pipefd_ptr, struct.pack('<ii', r, w))
            return 0
        except OSError:
            return -1

    def vuma_fork():
        try:
            # Suppress the deprecation warning about multi-threaded fork.
            # wasmtime uses background threads, but fork() is safe as long
            # as the child immediately calls execve (which our VUMA code does).
            import warnings
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                return os.fork()
        except OSError:
            return -1

    def vuma_execve(path_ptr, argv_ptr, envp_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            argv_ptrs = read_ptr_array(argv_ptr)
            envp_ptrs = read_ptr_array(envp_ptr)
            argv = [read_cstr(p).decode('utf-8', errors='replace') for p in argv_ptrs]
            envp = [read_cstr(p).decode('utf-8', errors='replace') for p in envp_ptrs]
            runner = os.path.abspath(__file__)
            new_argv = [sys.executable, runner, path] + argv[1:]
            env = os.environ.copy()
            for e in envp:
                if '=' in e:
                    k, v = e.split('=', 1)
                    env[k] = v
            os.execvpe(sys.executable, new_argv, env)
            return -1
        except OSError:
            return -1

    def vuma_dup2(oldfd, newfd):
        try:
            return os.dup2(oldfd, newfd)
        except OSError:
            return -1

    def vuma_waitpid(pid, status_ptr, options):
        try:
            result = os.waitpid(pid, options)
            # wasm32 is always little-endian; write status as LE 32-bit int
            write_mem(status_ptr, struct.pack('<i', result[1]))
            return result[0]
        except OSError:
            return -1

    def vuma_strcmp(s1_ptr, s2_ptr):
        s1 = read_cstr(s1_ptr)
        s2 = read_cstr(s2_ptr)
        for i in range(max(len(s1), len(s2)) + 1):
            b1 = s1[i] if i < len(s1) else 0
            b2 = s2[i] if i < len(s2) else 0
            if b1 != b2:
                return b1 - b2
        return 0

    # vuma_read(fd, buf_ptr, count) → nbytes
    # Uses os.read() directly — works with pipe fds that WASI fd_read
    # doesn't support (WASI only manages its own pre-opened fds).
    def vuma_read(fd, buf_ptr, count):
        try:
            data = os.read(fd, count)
            mem = get_mem()
            if mem is not None:
                mem.write(store, data, buf_ptr)
            return len(data)
        except OSError:
            return -1

    # vuma_write(fd, buf_ptr, count) → nbytes
    # Uses os.write() directly — works with pipe fds.
    def vuma_write(fd, buf_ptr, count):
        try:
            mem = get_mem()
            if mem is not None and count > 0:
                data = mem.read(store, buf_ptr, buf_ptr + count)
            else:
                data = b''
            return os.write(fd, data)
        except OSError:
            return -1

    # vuma_close(fd) → 0 on success, -1 on error
    def vuma_close(fd):
        try:
            os.close(fd)
            return 0
        except OSError:
            return -1

    i32 = ValType.i32()
    # Define the custom "vuma" module host functions in the linker
    linker.define_func("vuma", "pipe", FuncType([i32], [i32]), vuma_pipe)
    linker.define_func("vuma", "fork", FuncType([], [i32]), vuma_fork)
    linker.define_func("vuma", "execve", FuncType([i32, i32, i32], [i32]), vuma_execve)
    linker.define_func("vuma", "dup2", FuncType([i32, i32], [i32]), vuma_dup2)
    linker.define_func("vuma", "waitpid", FuncType([i32, i32, i32], [i32]), vuma_waitpid)
    linker.define_func("vuma", "strcmp", FuncType([i32, i32], [i32]), vuma_strcmp)
    # read/write/close use direct OS syscalls (bypass WASI for pipe fd support)
    linker.define_func("vuma", "read", FuncType([i32, i32, i32], [i32]), vuma_read)
    linker.define_func("vuma", "write", FuncType([i32, i32, i32], [i32]), vuma_write)
    linker.define_func("vuma", "close", FuncType([i32], [i32]), vuma_close)

    # Instantiate the module
    instance = linker.instantiate(store, module)

    # Get the memory export
    mem_holder[0] = instance.exports(store)["memory"]

    # Try to call _vuma_main
    exports = instance.exports(store)
    if "_vuma_main" in exports:
        main_func = exports["_vuma_main"]
        try:
            result = main_func(store)
            if result is not None:
                sys.exit(result & 0xFF)
            sys.exit(0)
        except SystemExit:
            raise
        except Exception as e:
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    else:
        print("Error: _vuma_main export not found", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
