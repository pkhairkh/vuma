# VUMA Showcase Website — Worklog

## Project Context

The user asked to "check out this project in detail" and provided the GitHub repository
`https://github.com/pkhairkh/vuma.git`.

### What VUMA is
VUMA (Verified-Unsafe Memory Access) is an AI-native programming language framework written in Rust.
Its radical premise: **unsafe memory operations should not be forbidden — they should be made verifiable.**
Instead of a borrow checker rejecting programs, VUMA constructs a formal model of every memory
operation and verifies five global invariants against it. Programs that pass run with zero overhead;
programs that fail receive precise counterexamples.

Key facts gathered from the repo:
- 12 workspace crates (`scg`, `bd`, `vuma`, `ive`, `cor`, `projection`, `parser`, `codegen`, `proof`, `std`, `tests`, `package`)
- 8 backend architectures: x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32
- 5 VUMA invariants: Liveness, Exclusivity, Interpretation, Origin, Cleanup
- Core abstractions: SCG (Semantic Computation Graph), BD (Behavioral Descriptor = RepD+CapD+RelD), MSG (Memory State Graph), IVE (Inference & Verification Engine), COR (Continuous Optimization Runtime)
- AI-native: VumaForLLM API, LSP server, REPL, Wasm32 sandbox for LLM agents
- 20+ example programs (hello_memory, doubly_linked_list, arena_allocator, lock_free_queue, fibonacci, etc.)
- 15 formal specification documents (~9,800 lines)
- 65 diagnostic codes, 73 math functions, 14 fmt functions, 19 Linux syscalls
- Standard library: math, fmt, crypto, string, alloc, io, sync, collections modules

### Decision
Build a comprehensive, single-page Next.js showcase website that documents and presents the VUMA
project in rich detail. Dark theme with emerald/cyan/amber accents (avoiding indigo/blue per rules).
Sections: hero, three innovations, five invariants, 6-layer architecture, compilation pipeline,
8 backends, code examples gallery, key concepts, standard library, wave timeline, LLM integration,
footer.

---
Task ID: 1
Agent: main
Task: Initialize worklog and gather all VUMA project data into structured data files

Work Log:
- Cloned the vuma repository to /tmp/vuma
- Read the full README.md (~830 lines) covering overview, features, 8 backends, architecture, pipeline, key concepts, examples, API, known limitations
- Read example programs: hello_memory.vuma, doubly_linked_list.vuma, arena_allocator.vuma, lock_free_queue.vuma, fibonacci.vuma
- Read the existing worklog.md from the repo (history of compiler backend fixes across 8 architectures)
- Examined the Next.js project scaffold and confirmed available shadcn/ui components and dependencies (framer-motion, react-syntax-highlighter, recharts, lucide-react)

Stage Summary:
- Complete understanding of VUMA project obtained
- Ready to build the showcase website with accurate, detailed content
- Data to be embedded directly in React components as structured objects
