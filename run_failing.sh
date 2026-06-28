#!/bin/bash
# Run the 6 failing tests and report results
export PATH="$HOME/qemu/usr/bin:$HOME/.cargo/bin:$PATH"
cd /home/z/my-project/inspect/vuma

COMPILE=./target/release/compile_dump
mkdir -p /tmp/vuma_tests

run_test() {
    local test="$1"
    local backend="$2"
    local expected="$3"
    local out="/tmp/vuma_tests/${test%.vuma}_${backend}.bin"
    local extra_args=""
    if [ "$backend" = "wasm32" ]; then
        $COMPILE "examples/$test" "$out" "$backend" 2>/dev/null
        chmod 644 "$out"
        # Check if print test
        if [[ "$test" == *print* ]]; then
            actual=$(wasmtime run "$out" 2>/dev/null; echo $?)
        else
            actual=$(wasmtime run --invoke _vuma_main "$out" 2>/dev/null)
            if [ -z "$actual" ]; then actual=1; fi
        fi
    else
        $COMPILE "examples/$test" "$out" "$backend" 2>/dev/null
        chmod +x "$out"
        local qemu=""
        case "$backend" in
            arm32) qemu="qemu-arm";;
            mips64) qemu="qemu-mips64el";;
            ppc64) qemu="qemu-ppc64";;
            x86_32) qemu="qemu-i386";;
            riscv32) qemu="qemu-riscv32";;
            aarch64) qemu="qemu-aarch64";;
            x86_64) qemu="qemu-x86_64";;
            riscv64) qemu="qemu-riscv64";;
            loongarch64) qemu="qemu-loongarch64";;
        esac
        timeout 5 $qemu "$out" 2>/dev/null
        actual=$?
    fi
    if [ "$actual" -eq "$expected" ]; then
        echo "PASS  $test $backend: got=$actual expected=$expected"
    else
        echo "FAIL  $test $backend: got=$actual expected=$expected"
    fi
}

# Run the 6 failing tests
run_test mmap_sha256d.vuma arm32 229
run_test lock_free_queue.vuma mips64 0
run_test lock_free_queue.vuma x86_32 0
run_test lock_free_queue.vuma wasm32 0
run_test epoll_echo.vuma wasm32 0
run_test enum_demo.vuma ppc64 141
