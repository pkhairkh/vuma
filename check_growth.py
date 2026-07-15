import os
import re

d = "/tmp/inline_funcs"
results = []
for f in sorted(os.listdir(d)):
    with open(os.path.join(d, f)) as h:
        content = h.read()
    m_enter = re.search(r"INLINE ENTER func=\S+ threshold=\d+ blocks=(\d+)", content)
    m_exit = re.search(r"INLINE EXIT func=(\S+) blocks=(\d+)", content)
    if m_enter and m_exit:
        enter_blocks = int(m_enter.group(1))
        exit_blocks = int(m_exit.group(2))
        # also count inlined markers (inl0_ etc.)
        if exit_blocks > enter_blocks:
            results.append((f, enter_blocks, exit_blocks, m_exit.group(1)))

for f, eb, xb, name in results:
    print(f"{f}: enter={eb} exit={xb} name={name}")
print(f"Total with growth: {len(results)}")
