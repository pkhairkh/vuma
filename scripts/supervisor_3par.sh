#!/usr/bin/env bash
# Supervisor: run 3 QEMU backends at a time, relaunching until all complete.
# The resilient runner checkpoints to TSV, so it resumes on relaunch.
cd /home/z/my-project/vuma
TOTAL=5754
LOG=/tmp/vuma_supervisor.log
> "$LOG"

BACKENDS="aarch64 riscv64 arm32 mips64 ppc64 loongarch64"
MAX_PARALLEL=3

for iter in $(seq 1 100); do
    echo "[$(date +%H:%M:%S)] Iter $iter" >> "$LOG"
    # Check which backends still need work
    TODO=""
    for b in $BACKENDS; do
        rows=0
        f=/home/z/my-project/download/gold_standard_results/$b.tsv
        [ -f "$f" ] && rows=$(($(wc -l < "$f") - 1))
        if [ "$rows" -lt "$TOTAL" ]; then
            TODO="$TODO $b"
        fi
    done
    TODO=$(echo $TODO | xargs)  # trim
    if [ -z "$TODO" ]; then
        echo "[$(date +%H:%M:%S)] ALL DONE!" >> "$LOG"
        break
    fi
    echo "[$(date +%H:%M:%S)] TODO:$TODO" >> "$LOG"

    # Launch up to MAX_PARALLEL backends
    count=0
    PIDS=""
    for b in $TODO; do
        count=$((count + 1))
        if [ $count -gt $MAX_PARALLEL ]; then break; fi
        timeout 120 python3 scripts/run_backend_resilient.py $b >> /tmp/vuma_logs/$b.log 2>&1 &
        PIDS="$PIDS $!"
        echo "[$(date +%H:%M:%S)] Launched $b (PID $!)" >> "$LOG"
    done

    # Wait for all launched processes
    for p in $PIDS; do
        wait $p 2>/dev/null
        echo "[$(date +%H:%M:%S)] PID $p exited ($?)" >> "$LOG"
    done

    # Report progress
    for b in $BACKENDS; do
        rows=0
        f=/home/z/my-project/download/gold_standard_results/$b.tsv
        [ -f "$f" ] && rows=$(($(wc -l < "$f") - 1))
        echo "[$(date +%H:%M:%S)]   $b: $rows/$TOTAL" >> "$LOG"
    done
    sleep 2
done
echo "[$(date +%H:%M:%S)] Supervisor finished" >> "$LOG"
