import os
import re

path = "/tmp/inline_dump.txt"
out_dir = "/tmp/inline_funcs"
os.makedirs(out_dir, exist_ok=True)

with open(path) as f:
    lines = f.readlines()

# Find ENTER positions.
enters = []
for i, line in enumerate(lines):
    if "INLINE ENTER" in line:
        enters.append(i)

# Each function spans from ENTER[i] to ENTER[i+1] (or EOF).
count = 0
for idx, start in enumerate(enters):
    end = enters[idx + 1] if idx + 1 < len(enters) else len(lines)
    buf = [l.rstrip() for l in lines[start:end]]
    m = re.match(r"=== INLINE ENTER func=(\S+) threshold=(\d+) blocks=(\d+) ===", buf[0])
    if not m:
        continue
    name = m.group(1)
    fname = f"{count:03d}_{name}.txt"
    count += 1
    with open(os.path.join(out_dir, fname), "w") as f:
        f.write("\n".join(buf))
print(f"Split {count} functions")
