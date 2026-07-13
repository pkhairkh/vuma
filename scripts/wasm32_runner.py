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

# Global: path to the wasm module being run (set in main(), used by vuma_fork)
_current_wasm_path = None

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
    # Also tracks pipe fds for fork() to use in the child.
    _pipe_fds = []  # list of (read_fd, write_fd) tuples

    def vuma_pipe(pipefd_ptr):
        try:
            r, w = os.pipe()
            _pipe_fds.append((r, w))
            # Write fds as native 32-bit integers (the VUMA program reads
            # them with read_i32_native, which handles both LE and BE).
            # wasm32 is always little-endian; write fds as LE 32-bit ints
            write_mem(pipefd_ptr, struct.pack('<ii', r, w))
            return 0
        except OSError:
            return -1

    # ── fork() -> i32 ─────────────────────────────────────────────────
    # Returns 0 in child, child PID in parent, -1 on error.
    #
    # CRITICAL: wasmtime's internal state (threads, mutexes) does NOT survive
    # os.fork().  If the child continues executing wasm code after fork(), it
    # will crash or produce wrong results.  The fix: in the child, DON'T touch
    # wasmtime at all.  Instead, immediately do dup2 + execve to start a fresh
    # wasmtime instance with child args.
    #
    # The child's dup2/close/execve calls (which normally happen in wasm code
    # between fork and exec) are handled here at the OS level.  We use the
    # tracked pipe fds to set up stdin/stdout before exec'ing.
    def vuma_fork():
        try:
            if len(_pipe_fds) < 2:
                return -1  # need at least 2 pipes for self_exec

            pipe1_read, pipe1_write = _pipe_fds[-2]  # first pipe (parent→child)
            pipe2_read, pipe2_write = _pipe_fds[-1]  # second pipe (child→parent)

            # CRITICAL: os.fork() does NOT work with wasmtime — the child's
            # wasmtime state is corrupted and the child crashes before
            # vuma_fork can return.  Instead, use subprocess.Popen to create
            # the child process with stdin/stdout redirected to the pipe fds.
            #
            # The child branch in the wasm code (dup2, close, execve) is
            # NEVER reached because fork() returns a non-zero PID (parent
            # mode).  The child's stdin/stdout redirection is handled by
            # subprocess.Popen's stdin/stdout parameters, which duplicates
            # what the wasm code's dup2 calls would do.
            import subprocess
            runner = os.path.abspath(__file__)
            wasm_path = _current_wasm_path

            # Start the child subprocess with:
            #   stdin  = pipe1_read  (parent writes to pipe1_write)
            #   stdout = pipe2_write (parent reads from pipe2_read)
            #   stderr = inherited
            proc = subprocess.Popen(
                [sys.executable, runner, wasm_path, "child"],
                stdin=pipe1_read,
                stdout=pipe2_write,
                stderr=sys.stderr,
                pass_fds=[],
                close_fds=True,
            )

            # Return the child PID to the wasm code.  The wasm code enters
            # the PARENT branch (pid != 0): close unused pipe ends, write to
            # pipe1, read from pipe2, waitpid.
            #
            # The child branch (dup2, close, execve) is skipped — the child
            # subprocess is already running with correct stdin/stdout.
            return proc.pid
        except OSError:
            return -1
        except Exception as e:
            sys.stderr.write(f"vuma_fork error: {e}\n")
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
    global _current_wasm_path
    if len(sys.argv) < 2:
        print("Usage: wasm32_runner.py <wasm_file> [args...]", file=sys.stderr)
        sys.exit(1)

    wasm_path = sys.argv[1]
    _current_wasm_path = wasm_path  # saved for vuma_fork() to use in child
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
            ret = 0
        except OSError:
            ret = -1
        # Codegen reads return value from mem[0] (i32) for non-void externs.
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_fork():
        try:
            # Suppress the deprecation warning about multi-threaded fork.
            # wasmtime uses background threads, but fork() is safe as long
            # as the child immediately calls execve (which our VUMA code does).
            import warnings
            with warnings.catch_warnings():
                warnings.simplefilter("ignore")
                ret = os.fork()
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

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
            ret = -1
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_dup2(oldfd, newfd):
        try:
            ret = os.dup2(oldfd, newfd)
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_waitpid(pid, status_ptr, options):
        try:
            result = os.waitpid(pid, options)
            # wasm32 is always little-endian; write status as LE 32-bit int
            write_mem(status_ptr, struct.pack('<i', result[1]))
            ret = result[0]
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_strcmp(s1_ptr, s2_ptr):
        s1 = read_cstr(s1_ptr)
        s2 = read_cstr(s2_ptr)
        diff = 0
        for i in range(max(len(s1), len(s2)) + 1):
            b1 = s1[i] if i < len(s1) else 0
            b2 = s2[i] if i < len(s2) else 0
            if b1 != b2:
                diff = b1 - b2
                break
        write_mem(0, struct.pack('<i', diff))
        return diff

    # vuma_read(fd, buf_ptr, count) → nbytes
    # Uses os.read() directly — works with pipe fds that WASI fd_read
    # doesn't support (WASI only manages its own pre-opened fds).
    # Also writes return value to mem[0] for the codegen.
    def vuma_read(fd, buf_ptr, count):
        try:
            data = os.read(fd, count)
            mem = get_mem()
            if mem is not None:
                mem.write(store, data, buf_ptr)
                mem.write(store, struct.pack('<i', len(data)), 0)
            return len(data)
        except OSError:
            mem = get_mem()
            if mem is not None:
                mem.write(store, struct.pack('<i', -1), 0)
            return -1

    # vuma_write(fd, buf_ptr, count) → nbytes
    # Uses os.write() directly — works with pipe fds.
    # CRITICAL: The wasm32 codegen expects extern function return values
    # at memory address 0 (mem[0]). But wasmtime host functions return
    # values on the wasm stack. The codegen loads from mem[0] after
    # the call, so we must ALSO write the result to mem[0].
    def vuma_write(fd, buf_ptr, count):
        try:
            mem = get_mem()
            if mem is not None and count > 0:
                data = mem.read(store, buf_ptr, buf_ptr + count)
            else:
                data = b''
            n = os.write(fd, data)
            # Store return value at mem[0] for the codegen to load
            if mem is not None:
                mem.write(store, struct.pack('<i', n), 0)
            return n
        except OSError:
            if mem is not None:
                mem.write(store, struct.pack('<i', -1), 0)
            return -1

    # vuma_close(fd) → 0 on success, -1 on error
    def vuma_close(fd):
        try:
            os.close(fd)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # ── Filesystem ops (Wave 4) ──────────────────────────────────────
    # These host functions bridge POSIX filesystem syscalls to the wasm
    # sandbox.  They read path strings from wasm linear memory, call the
    # real OS, and write return values to mem[0] (the codegen's convention
    # for extern return values).

    # vuma_open(path_ptr, flags, mode) → fd
    # flags and mode are POSIX integers (O_RDONLY=0, O_WRONLY=1, O_CREAT=64,
    # O_TRUNC=512, O_APPEND=1024, etc. — same as Linux generic ABI).
    def vuma_open(path_ptr, flags, mode):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            ret = os.open(path, flags, mode)
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_stat(path_ptr, buf_ptr) → 0 on success, -1 on error
    # Writes a simplified struct stat to buf_ptr:
    #   offset  0: st_dev   (u64)
    #   offset  8: st_ino   (u64)
    #   offset 16: st_mode  (u32)
    #   offset 20: st_nlink (u32)
    #   offset 24: st_uid   (u32)
    #   offset 28: st_gid   (u32)
    #   offset 32: st_size  (u64)
    #   offset 40: st_atime (u64)
    #   offset 48: st_mtime (u64)
    #   offset 56: st_ctime (u64)
    # Total: 64 bytes.
    def _write_stat_buf(buf_ptr, st):
        """Write a simplified 64-byte struct stat to wasm memory."""
        data = struct.pack('<QQIIIIIQQQ',
                           st.st_dev, st.st_ino, st.st_mode, st.st_nlink,
                           st.st_uid, st.st_gid, 0,  # pad
                           st.st_size, int(st.st_atime), int(st.st_mtime),
                           int(st.st_ctime))
        write_mem(buf_ptr, data)

    def vuma_stat(path_ptr, buf_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            st = os.stat(path)
            _write_stat_buf(buf_ptr, st)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_fstat(fd, buf_ptr):
        try:
            st = os.fstat(fd)
            _write_stat_buf(buf_ptr, st)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    def vuma_lstat(path_ptr, buf_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            st = os.lstat(path)
            _write_stat_buf(buf_ptr, st)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_unlink(path_ptr) → 0 on success, -1 on error
    def vuma_unlink(path_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            os.unlink(path)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_mkdir(path_ptr, mode) → 0 on success, -1 on error
    def vuma_mkdir(path_ptr, mode):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            os.mkdir(path, mode)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_rmdir(path_ptr) → 0 on success, -1 on error
    def vuma_rmdir(path_ptr):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            os.rmdir(path)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_rename(oldpath_ptr, newpath_ptr) → 0 on success, -1 on error
    def vuma_rename(oldpath_ptr, newpath_ptr):
        try:
            old = read_cstr(oldpath_ptr).decode('utf-8', errors='replace')
            new = read_cstr(newpath_ptr).decode('utf-8', errors='replace')
            os.rename(old, new)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_link(oldpath_ptr, newpath_ptr) → 0 on success, -1 on error
    def vuma_link(oldpath_ptr, newpath_ptr):
        try:
            old = read_cstr(oldpath_ptr).decode('utf-8', errors='replace')
            new = read_cstr(newpath_ptr).decode('utf-8', errors='replace')
            os.link(old, new)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_symlink(target_ptr, linkpath_ptr) → 0 on success, -1 on error
    def vuma_symlink(target_ptr, linkpath_ptr):
        try:
            target = read_cstr(target_ptr).decode('utf-8', errors='replace')
            linkpath = read_cstr(linkpath_ptr).decode('utf-8', errors='replace')
            os.symlink(target, linkpath)
            ret = 0
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_readlink(path_ptr, buf_ptr, bufsize) → nbytes, or -1 on error
    def vuma_readlink(path_ptr, buf_ptr, bufsize):
        try:
            path = read_cstr(path_ptr).decode('utf-8', errors='replace')
            target = os.readlink(path).encode('utf-8')
            if len(target) >= bufsize:
                target = target[:bufsize - 1]
            write_mem(buf_ptr, target)
            ret = len(target)
        except OSError:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

    # ── Socket / send-recv / sockopt / mmap / nanosleep host functions ────
    # (Wave 5) POSIX-compatible socket family backed by the real OS.
    #
    # The fds returned by socket()/accept() are real host OS file
    # descriptors.  We use socket.socket(fileno=fd) to wrap them for
    # bind/listen/connect/send/recv/etc., then call .detach() so the
    # wrapper does NOT close the fd when garbage-collected.  This keeps
    # the fd alive for subsequent calls (send/recv/close/shutdown).
    #
    # sockaddr_in layout (AF_INET = 2, 16 bytes), little-endian on wasm32:
    #   offset  0: sin_family  (u16)
    #   offset  2: sin_port    (u16, network byte order — big endian)
    #   offset  4: sin_addr    (u32, network byte order)
    #   offset  8: sin_zero    (8 bytes padding)
    import socket as _socket_mod
    AF_INET = _socket_mod.AF_INET
    SOCK_STREAM = _socket_mod.SOCK_STREAM
    SOCK_DGRAM = _socket_mod.SOCK_DGRAM

    # Linux <asm-generic/mman-common.h> flag bits we recognize.
    MAP_ANONYMOUS = 0x20
    MAP_FAILED = -1

    # Bump-allocator state for anonymous mmap.  The mmap region lives in
    # wasm linear memory starting at MMAP_BASE (1 MiB, well above the
    # __vuma_alloc heap at 64 KiB and the print/args scratch at <4 KiB).
    # We grow the wasm memory as needed via memory.grow().
    MMAP_BASE = 0x100000  # 1 MiB
    _mmap_bump = [MMAP_BASE]  # mutable holder so closures can update it

    def _ensure_mem_size(end_addr):
        """Grow wasm linear memory so that [0, end_addr) is valid.
        Returns True on success, False if growth failed (memory max hit)."""
        mem = get_mem()
        if mem is None:
            return False
        page = 65536
        cur_pages = mem.data_len(store) // page
        need_pages = (end_addr + page - 1) // page
        if need_pages <= cur_pages:
            return True
        # Grow by the delta.  memory.grow returns the old size in pages
        # (>=0) on success or -1 (as a signed value) on failure.
        delta = need_pages - cur_pages
        try:
            rc = mem.grow(store, delta)
        except Exception:
            return False
        if rc is None or (isinstance(rc, int) and rc < 0):
            return False
        return True

    def _read_sockaddr_in(addr_ptr, addrlen):
        """Read a sockaddr_in (AF_INET) from wasm memory.
        Returns a (host_ip_string, port) tuple, or None on failure."""
        mem = get_mem()
        if mem is None or addr_ptr == 0:
            return None
        try:
            raw = mem.read(store, addr_ptr, addr_ptr + min(addrlen, 16))
        except Exception:
            return None
        if len(raw) < 8:
            return None
        family = struct.unpack('<H', raw[0:2])[0]
        if family != AF_INET:
            return None
        port = struct.unpack('>H', raw[2:4])[0]  # network byte order
        addr = struct.unpack('>I', raw[4:8])[0]  # network byte order
        host = _socket_mod.inet_ntoa(struct.pack('>I', addr))
        return (host, port)

    def _write_sockaddr_in(addr_ptr, host, port):
        """Write a sockaddr_in (16 bytes) for (host, port) to wasm memory.
        Also writes addrlen=16.  Used by accept()/recvfrom()."""
        if addr_ptr == 0:
            return
        addr = struct.unpack('>I', _socket_mod.inet_aton(host))[0]
        sa = struct.pack('<H', AF_INET) + struct.pack('>H', port) \
            + struct.pack('>I', addr) + b'\x00' * 8
        write_mem(addr_ptr, sa)

    def _wrap_fd(fd):
        """Wrap a raw OS fd in a socket object WITHOUT taking ownership.
        Caller MUST .detach() before the wrapper is GC'd to avoid closing."""
        try:
            s = _socket_mod.socket(fileno=fd)
            return s
        except OSError:
            return None

    # vuma_socket(domain, type, protocol) → fd, or -errno on error
    def vuma_socket(domain, type_, protocol):
        try:
            s = _socket_mod.socket(domain, type_, protocol)
            fd = s.fileno()
            s.detach()  # release ownership so GC doesn't close the fd
            ret = fd
        except OSError as e:
            ret = -(e.errno or 1)
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_bind(fd, addr_ptr, addrlen) → 0 on success, -errno on error
    def vuma_bind(fd, addr_ptr, addrlen):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                apa = _read_sockaddr_in(addr_ptr, addrlen)
                if apa is None:
                    ret = -97  # EAFNOSUPPORT
                else:
                    s.bind(apa)
                    ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_listen(fd, backlog) → 0 on success, -errno on error
    def vuma_listen(fd, backlog):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                s.listen(backlog)
                ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_accept(fd, addr_ptr, addrlen_ptr) → client fd, or -errno on error
    # Writes the client's sockaddr_in to addr_ptr (if non-NULL) and the
    # addrlen (16) to addrlen_ptr (if non-NULL).
    def vuma_accept(fd, addr_ptr, addrlen_ptr):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                conn, addr = s.accept()
                cfd = conn.fileno()
                conn.detach()  # release ownership of the client fd
                if addr_ptr != 0:
                    host, port = addr[0], addr[1]
                    _write_sockaddr_in(addr_ptr, host, port)
                if addrlen_ptr != 0:
                    write_mem(addrlen_ptr, struct.pack('<i', 16))
                ret = cfd
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_connect(fd, addr_ptr, addrlen) → 0 on success, -errno on error
    def vuma_connect(fd, addr_ptr, addrlen):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                apa = _read_sockaddr_in(addr_ptr, addrlen)
                if apa is None:
                    ret = -97  # EAFNOSUPPORT
                else:
                    s.connect(apa)
                    ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_send(fd, buf_ptr, len, flags) → nbytes, or -errno on error
    def vuma_send(fd, buf_ptr, length, flags):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                mem = get_mem()
                data = mem.read(store, buf_ptr, buf_ptr + length) if (mem and length > 0) else b''
                n = s.send(data, flags)
                ret = n
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_recv(fd, buf_ptr, len, flags) → nbytes, or -errno on error
    def vuma_recv(fd, buf_ptr, length, flags):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                data = s.recv(length, flags)
                mem = get_mem()
                if mem is not None and data:
                    mem.write(store, data, buf_ptr)
                ret = len(data)
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_sendto(fd, buf_ptr, len, flags, addr_ptr, addrlen) → nbytes
    def vuma_sendto(fd, buf_ptr, length, flags, addr_ptr, addrlen):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                mem = get_mem()
                data = mem.read(store, buf_ptr, buf_ptr + length) if (mem and length > 0) else b''
                apa = _read_sockaddr_in(addr_ptr, addrlen) if addr_ptr != 0 else None
                if apa is None and addr_ptr != 0:
                    ret = -97  # EAFNOSUPPORT
                elif apa is None:
                    n = s.send(data, flags)
                    ret = n
                else:
                    n = s.sendto(data, flags, apa)
                    ret = n
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_recvfrom(fd, buf_ptr, len, flags, addr_ptr, addrlen_ptr) → nbytes
    def vuma_recvfrom(fd, buf_ptr, length, flags, addr_ptr, addrlen_ptr):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                data, addr = s.recvfrom(length, flags)
                mem = get_mem()
                if mem is not None and data:
                    mem.write(store, data, buf_ptr)
                if addr_ptr != 0 and addr:
                    host, port = addr[0], addr[1]
                    _write_sockaddr_in(addr_ptr, host, port)
                if addrlen_ptr != 0:
                    write_mem(addrlen_ptr, struct.pack('<i', 16))
                ret = len(data)
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_setsockopt(fd, level, optname, optval_ptr, optlen) → 0 / -errno
    def vuma_setsockopt(fd, level, optname, optval_ptr, optlen):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                mem = get_mem()
                val = mem.read(store, optval_ptr, optval_ptr + optlen) if (mem and optlen > 0) else b''
                # setsockopt accepts an int (auto-packed) or a bytes object.
                if optlen == 4:
                    s.setsockopt(level, optname, struct.unpack('<i', val)[0])
                else:
                    s.setsockopt(level, optname, val)
                ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_getsockopt(fd, level, optname, optval_ptr, optlen_ptr) → 0 / -errno
    # Writes the option value to optval_ptr and the length to optlen_ptr.
    def vuma_getsockopt(fd, level, optname, optval_ptr, optlen_ptr):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                mem = get_mem()
                # Read the requested buffer length.
                buflen = 4
                if mem is not None and optlen_ptr != 0:
                    raw_len = mem.read(store, optlen_ptr, optlen_ptr + 4)
                    if len(raw_len) == 4:
                        buflen = struct.unpack('<i', raw_len)[0]
                if buflen <= 0:
                    buflen = 4
                val = s.getsockopt(level, optname, buflen)
                if mem is not None:
                    if optval_ptr != 0:
                        mem.write(store, val, optval_ptr)
                    if optlen_ptr != 0:
                        mem.write(store, struct.pack('<i', len(val)), optlen_ptr)
                ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_shutdown(fd, how) → 0 / -errno
    def vuma_shutdown(fd, how):
        s = _wrap_fd(fd)
        if s is None:
            ret = -9  # EBADF
        else:
            try:
                s.shutdown(how)
                ret = 0
            except OSError as e:
                ret = -(e.errno or 1)
            finally:
                try: s.detach()
                except Exception: pass
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_mmap(addr, len, prot, flags, fd, offset) → ptr, or -1 (MAP_FAILED)
    # Anonymous mmap (MAP_ANONYMOUS): bump-allocate in wasm linear memory,
    # growing memory as needed.  File-backed mmap: unsupported → MAP_FAILED.
    def vuma_mmap(addr, length, prot, flags, fd, offset):
        if length <= 0:
            ret = -1  # MAP_FAILED
        elif not (flags & MAP_ANONYMOUS):
            # File-backed mmap is not supported in the wasm32 sandbox.
            ret = -1  # MAP_FAILED (errno ENOSYS would be set by the kernel)
        else:
            page = 65536
            aligned_len = (length + page - 1) & ~(page - 1)
            ptr = _mmap_bump[0]
            end = ptr + aligned_len
            if not _ensure_mem_size(end):
                ret = -1  # MAP_FAILED (ENOMEM)
            else:
                _mmap_bump[0] = end
                ret = ptr
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_munmap(addr, len) → 0 (no-op on wasm32; bump-allocator can't free)
    def vuma_munmap(addr, length):
        ret = 0
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_mprotect(addr, len, prot) → 0 (no-op; wasm has no page protection)
    def vuma_mprotect(addr, length, prot):
        ret = 0
        write_mem(0, struct.pack('<i', ret))
        return ret

    # vuma_nanosleep(req_ptr, rem_ptr) → 0 on success, -errno on error
    # req is a struct timespec { tv_sec: i64, tv_nsec: i64 } (16 bytes).
    # rem (if non-NULL) receives the remaining time on interruption.
    def vuma_nanosleep(req_ptr, rem_ptr):
        import time as _time_mod
        try:
            mem = get_mem()
            if mem is None:
                ret = -14  # EFAULT
            else:
                raw = mem.read(store, req_ptr, req_ptr + 16)
                if len(raw) < 16:
                    ret = -14  # EFAULT
                else:
                    tv_sec, tv_nsec = struct.unpack('<qq', raw)
                    if tv_sec < 0 or tv_nsec < 0 or tv_nsec >= 1_000_000_000:
                        ret = -22  # EINVAL
                    else:
                        secs = tv_sec + tv_nsec / 1_000_000_000.0
                        _time_mod.sleep(secs)
                        if rem_ptr != 0:
                            # No remainder (slept the full duration).
                            write_mem(rem_ptr, struct.pack('<qq', 0, 0))
                        ret = 0
        except OSError as e:
            ret = -(e.errno or 1)
        except Exception:
            ret = -1
        write_mem(0, struct.pack('<i', ret))
        return ret

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
    # Filesystem ops (Wave 4) — POSIX-compatible host functions
    linker.define_func("vuma", "open", FuncType([i32, i32, i32], [i32]), vuma_open)
    linker.define_func("vuma", "stat", FuncType([i32, i32], [i32]), vuma_stat)
    linker.define_func("vuma", "fstat", FuncType([i32, i32], [i32]), vuma_fstat)
    linker.define_func("vuma", "lstat", FuncType([i32, i32], [i32]), vuma_lstat)
    linker.define_func("vuma", "unlink", FuncType([i32], [i32]), vuma_unlink)
    linker.define_func("vuma", "mkdir", FuncType([i32, i32], [i32]), vuma_mkdir)
    linker.define_func("vuma", "rmdir", FuncType([i32], [i32]), vuma_rmdir)
    linker.define_func("vuma", "rename", FuncType([i32, i32], [i32]), vuma_rename)
    linker.define_func("vuma", "link", FuncType([i32, i32], [i32]), vuma_link)
    linker.define_func("vuma", "symlink", FuncType([i32, i32], [i32]), vuma_symlink)
    linker.define_func("vuma", "readlink", FuncType([i32, i32, i32], [i32]), vuma_readlink)
    # Socket family (Wave 5) — POSIX-compatible host functions backed by the
    # real OS socket layer.  sendmsg / recvmsg are NOT defined here; they
    # resolve to the generic -ENOSYS stub in the wasm module (msghdr
    # marshaling is too complex for the wasm32 bridge).
    linker.define_func("vuma", "socket", FuncType([i32, i32, i32], [i32]), vuma_socket)
    linker.define_func("vuma", "bind", FuncType([i32, i32, i32], [i32]), vuma_bind)
    linker.define_func("vuma", "listen", FuncType([i32, i32], [i32]), vuma_listen)
    linker.define_func("vuma", "accept", FuncType([i32, i32, i32], [i32]), vuma_accept)
    linker.define_func("vuma", "connect", FuncType([i32, i32, i32], [i32]), vuma_connect)
    linker.define_func("vuma", "send", FuncType([i32, i32, i32, i32], [i32]), vuma_send)
    linker.define_func("vuma", "recv", FuncType([i32, i32, i32, i32], [i32]), vuma_recv)
    linker.define_func("vuma", "sendto", FuncType([i32, i32, i32, i32, i32, i32], [i32]), vuma_sendto)
    linker.define_func("vuma", "recvfrom", FuncType([i32, i32, i32, i32, i32, i32], [i32]), vuma_recvfrom)
    linker.define_func("vuma", "setsockopt", FuncType([i32, i32, i32, i32, i32], [i32]), vuma_setsockopt)
    linker.define_func("vuma", "getsockopt", FuncType([i32, i32, i32, i32, i32], [i32]), vuma_getsockopt)
    linker.define_func("vuma", "shutdown", FuncType([i32, i32], [i32]), vuma_shutdown)
    # Memory management (Wave 5).  mmap anonymous = bump-allocate in linear
    # memory; file-backed = MAP_FAILED (-1); munmap/mprotect = no-op 0.
    linker.define_func("vuma", "mmap", FuncType([i32, i32, i32, i32, i32, i32], [i32]), vuma_mmap)
    linker.define_func("vuma", "munmap", FuncType([i32, i32], [i32]), vuma_munmap)
    linker.define_func("vuma", "mprotect", FuncType([i32, i32, i32], [i32]), vuma_mprotect)
    # Sleep (Wave 5).  clock_gettime is already aliased to WASI
    # clock_time_get; nanosleep is a new host function (real time.sleep).
    linker.define_func("vuma", "nanosleep", FuncType([i32, i32], [i32]), vuma_nanosleep)

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
            # ExitTrap is raised when the wasm code calls proc_exit (e.g.
            # child_mode's exit(0)).  In the parent process, this means the
            # child's exit was caught — but the parent should continue and
            # _vuma_main should return the parent's exit code.  If we reach
            # here, the parent itself called proc_exit (e.g. via exit(N)),
            # so we exit with 1 to signal an abnormal termination.
            print(f"Error: {e}", file=sys.stderr)
            sys.exit(1)
    else:
        print("Error: _vuma_main export not found", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
