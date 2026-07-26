//! # Memory Safety Verification Module
//!
//! Compile-time and optional runtime memory safety checks for the VUMA compiler.
//!
//! ## Checks Provided
//!
//! | #  | Check              | Code  | Stage      | Description                                          |
//! |----|--------------------|-------|------------|------------------------------------------------------|
//! | 1  | Use-after-free     | E041  | Compile    | Value live after deallocation via SCG liveness       |
//! | 2  | Double-free        | E042  | Compile    | Same allocation freed more than once                  |
//! | 3  | Memory leak        | E043  | Compile    | Heap allocation with no matching free on exit paths   |
//! | 4  | Bounds check       | E044  | Runtime    | Array index out-of-bounds (enabled by `--safe`)      |
//! | 5  | Null deref         | E045  | Compile    | Dereference of pointer that may be null               |
//! | 6  | Dangling pointer   | E046  | Compile    | Pointer to stack allocation that escapes its scope    |
//! | 7  | Uninitialized read | E047  | Compile    | Read of allocation with no reaching write             |
//! | 8  | Buffer overflow    | E048  | Runtime    | Write past allocation boundary (enabled by `--safe`)  |
//! | 9  | Use-after-scope    | E049  | Compile    | Access to stack variable after scope exit             |
//! | 10 | Invalid free       | E050  | Compile    | Free of non-heap pointer or already-freed pointer     |
//!
//! ## Integration
//!
//! The module integrates with the SCG liveness analysis from `vuma-scg` and
//! with the diagnostics system (error codes E041–E050).  The `--safe` CLI
//! flag enables runtime bounds-checking instrumentation.

use std::collections::{HashMap, HashSet};
use std::fmt;

// ─── Re-export liveness types from vuma-scg ──────────────────────────────────
// The SCG liveness analysis provides the foundation for use-after-free and
// dead-allocation detection. We depend on it through the codegen SCG bridge.

use crate::ir::BinOpKind;
#[cfg(test)]
use crate::scg_to_ir::ScgType;
use crate::scg_to_ir::{
    AccessNode, AllocationNode, CallNode, ComputationNode, ControlNode, Scg, ScgExpr, ScgFunction,
    ScgNode, ScgStatement,
};

// ═══════════════════════════════════════════════════════════════════════════
// Error codes for memory safety diagnostics
// ═══════════════════════════════════════════════════════════════════════════

/// Memory safety violation kind, mapped to E041–E050.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemorySafetyViolation {
    /// E041 — Use-after-free: value still live after deallocation.
    UseAfterFree {
        /// Name of the freed allocation.
        allocation_name: String,
        /// Line of the deallocation.
        dealloc_line: Option<u32>,
        /// Number of uses after free.
        violation_count: usize,
    },
    /// E042 — Double-free: same pointer freed twice.
    DoubleFree {
        /// Name of the allocation freed twice.
        allocation_name: String,
        /// Line of the first free.
        first_free_line: Option<u32>,
        /// Line of the second free.
        second_free_line: Option<u32>,
    },
    /// E043 — Memory leak: heap allocation never freed.
    MemoryLeak {
        /// Name of the leaked allocation.
        allocation_name: String,
        /// Line of the allocation.
        alloc_line: Option<u32>,
        /// Size of the leaked allocation in bytes.
        alloc_size: Option<u32>,
    },
    /// E044 — Bounds check failure (runtime): array index out of bounds.
    BoundsCheckFailure {
        /// Name of the array being accessed.
        array_name: String,
        /// Index used.
        index: i64,
        /// Array length.
        length: u64,
    },
    /// E045 — Null pointer dereference.
    NullDereference {
        /// Name of the pointer variable.
        pointer_name: String,
    },
    /// E046 — Dangling pointer: stack address escapes its scope.
    DanglingPointer {
        /// Name of the escaped pointer.
        pointer_name: String,
        /// Scope where the allocation was made.
        scope_name: String,
    },
    /// E047 — Uninitialized read: reading from allocation with no reaching write.
    UninitializedRead {
        /// Name of the variable being read.
        variable_name: String,
    },
    /// E048 — Buffer overflow (runtime): write past allocation boundary.
    BufferOverflow {
        /// Name of the target buffer.
        buffer_name: String,
        /// Offset of the write.
        offset: u64,
        /// Size of the buffer.
        buffer_size: u64,
    },
    /// E049 — Use after scope: access to stack variable after scope exit.
    UseAfterScope {
        /// Name of the variable.
        variable_name: String,
        /// Scope where the variable was defined.
        scope_name: String,
    },
    /// E050 — Invalid free: freeing a non-heap pointer or already-freed pointer.
    InvalidFree {
        /// Name of the pointer being freed.
        pointer_name: String,
        /// Reason for the invalid free.
        reason: String,
    },
}

impl MemorySafetyViolation {
    /// Returns the diagnostic code string for this violation.
    pub fn code(&self) -> &'static str {
        match self {
            MemorySafetyViolation::UseAfterFree { .. } => "E041",
            MemorySafetyViolation::DoubleFree { .. } => "E042",
            MemorySafetyViolation::MemoryLeak { .. } => "E043",
            MemorySafetyViolation::BoundsCheckFailure { .. } => "E044",
            MemorySafetyViolation::NullDereference { .. } => "E045",
            MemorySafetyViolation::DanglingPointer { .. } => "E046",
            MemorySafetyViolation::UninitializedRead { .. } => "E047",
            MemorySafetyViolation::BufferOverflow { .. } => "E048",
            MemorySafetyViolation::UseAfterScope { .. } => "E049",
            MemorySafetyViolation::InvalidFree { .. } => "E050",
        }
    }

    /// Returns a human-readable description of this violation.
    pub fn description(&self) -> String {
        match self {
            MemorySafetyViolation::UseAfterFree {
                allocation_name,
                dealloc_line,
                violation_count,
            } => format!(
                "use-after-free: '{}' still used after free at line {} ({} violating use(s))",
                allocation_name,
                dealloc_line.unwrap_or(0),
                violation_count
            ),
            MemorySafetyViolation::DoubleFree {
                allocation_name,
                first_free_line,
                second_free_line,
            } => format!(
                "double-free: '{}' freed at line {} and again at line {}",
                allocation_name,
                first_free_line.unwrap_or(0),
                second_free_line.unwrap_or(0)
            ),
            MemorySafetyViolation::MemoryLeak {
                allocation_name,
                alloc_line,
                alloc_size,
            } => format!(
                "memory leak: '{}' (allocated at line {}, size {} bytes) never freed",
                allocation_name,
                alloc_line.unwrap_or(0),
                alloc_size.unwrap_or(0)
            ),
            MemorySafetyViolation::BoundsCheckFailure {
                array_name,
                index,
                length,
            } => format!(
                "bounds check failed: index {} out of bounds for array '{}' (length {})",
                index, array_name, length
            ),
            MemorySafetyViolation::NullDereference { pointer_name } => {
                format!("null pointer dereference: '{}'", pointer_name)
            }
            MemorySafetyViolation::DanglingPointer {
                pointer_name,
                scope_name,
            } => format!(
                "dangling pointer: '{}' escapes scope '{}'",
                pointer_name, scope_name
            ),
            MemorySafetyViolation::UninitializedRead { variable_name } => {
                format!(
                    "uninitialized read: variable '{}' has no reaching write",
                    variable_name
                )
            }
            MemorySafetyViolation::BufferOverflow {
                buffer_name,
                offset,
                buffer_size,
            } => format!(
                "buffer overflow: write at offset {} past buffer '{}' (size {})",
                offset, buffer_name, buffer_size
            ),
            MemorySafetyViolation::UseAfterScope {
                variable_name,
                scope_name,
            } => format!(
                "use after scope: variable '{}' used after scope '{}' exits",
                variable_name, scope_name
            ),
            MemorySafetyViolation::InvalidFree {
                pointer_name,
                reason,
            } => format!("invalid free: pointer '{}' — {}", pointer_name, reason),
        }
    }
}

impl fmt::Display for MemorySafetyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code(), self.description())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MemorySafetyConfig
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for memory safety checks.
#[derive(Debug, Clone)]
pub struct MemorySafetyConfig {
    /// Enable runtime bounds checking for array accesses.
    /// When enabled, the codegen inserts bounds-check instructions before
    /// every array load/store.  This corresponds to the `--safe` CLI flag.
    pub runtime_bounds_checks: bool,

    /// Enable use-after-free detection at compile time.
    pub check_use_after_free: bool,

    /// Enable double-free detection at compile time.
    pub check_double_free: bool,

    /// Enable memory leak detection at compile time.
    pub check_memory_leaks: bool,

    /// Enable uninitialized read detection at compile time.
    pub check_uninitialized_reads: bool,

    /// Enable dangling pointer / scope escape detection.
    pub check_dangling_pointers: bool,

    /// Treat memory safety violations as errors (true) or warnings (false).
    pub errors_are_fatal: bool,
}

impl Default for MemorySafetyConfig {
    fn default() -> Self {
        Self {
            runtime_bounds_checks: false,
            check_use_after_free: true,
            check_double_free: true,
            check_memory_leaks: true,
            check_uninitialized_reads: true,
            check_dangling_pointers: true,
            errors_are_fatal: true,
        }
    }
}

impl MemorySafetyConfig {
    /// Configuration enabled by the `--safe` CLI flag.
    /// Enables runtime bounds checks in addition to all compile-time checks.
    pub fn safe_mode() -> Self {
        Self {
            runtime_bounds_checks: true,
            check_use_after_free: true,
            check_double_free: true,
            check_memory_leaks: true,
            check_uninitialized_reads: true,
            check_dangling_pointers: true,
            errors_are_fatal: true,
        }
    }

    /// Only compile-time checks, no runtime instrumentation.
    pub fn compile_time_only() -> Self {
        Self {
            runtime_bounds_checks: false,
            ..Self::default()
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MemorySafetyReport
// ═══════════════════════════════════════════════════════════════════════════

/// The result of running memory safety analysis on a program.
#[derive(Debug, Clone)]
pub struct MemorySafetyReport {
    /// All violations found during analysis.
    pub violations: Vec<MemorySafetyViolation>,

    /// Number of heap allocations analyzed.
    pub heap_allocations_analyzed: usize,

    /// Number of stack allocations analyzed.
    pub stack_allocations_analyzed: usize,

    /// Number of deallocations analyzed.
    pub deallocations_analyzed: usize,

    /// Number of access sites analyzed.
    pub access_sites_analyzed: usize,

    /// Total analysis time in microseconds.
    pub analysis_time_us: u64,
}

impl MemorySafetyReport {
    /// Create an empty report.
    pub fn empty() -> Self {
        Self {
            violations: Vec::new(),
            heap_allocations_analyzed: 0,
            stack_allocations_analyzed: 0,
            deallocations_analyzed: 0,
            access_sites_analyzed: 0,
            analysis_time_us: 0,
        }
    }

    /// Returns `true` if no violations were found.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns the number of errors (as opposed to warnings).
    pub fn error_count(&self) -> usize {
        self.violations.len()
    }

    /// Returns violations of a specific kind, identified by code.
    pub fn violations_by_code(&self, code: &str) -> Vec<&MemorySafetyViolation> {
        self.violations
            .iter()
            .filter(|v| v.code() == code)
            .collect()
    }
}

impl fmt::Display for MemorySafetyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.violations.is_empty() {
            writeln!(
                f,
                "Memory safety: CLEAN ({} heap allocs, {} stack allocs analyzed)",
                self.heap_allocations_analyzed, self.stack_allocations_analyzed
            )
        } else {
            writeln!(
                f,
                "Memory safety: {} violation(s) found",
                self.violations.len()
            )?;
            for v in &self.violations {
                writeln!(f, "  {}", v)?;
            }
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Allocation tracking (for the codegen SCG representation)
// ═══════════════════════════════════════════════════════════════════════════

/// Information about a tracked allocation within the codegen SCG.
#[derive(Debug, Clone)]
struct AllocationInfo {
    /// Whether this is a heap or stack allocation.
    is_heap: bool,
    /// Size in bytes (if known at compile time).
    size: Option<u32>,
    /// Source line number (best-effort).
    line: Option<u32>,
    /// Set of free/deallocation operations on this allocation.
    frees: Vec<FreeInfo>,
    /// Set of access operations on this allocation.
    accesses: Vec<AccessInfo>,
    /// Whether this allocation is returned from the function (escapes).
    is_returned: bool,
}

/// Information about a free/deallocation operation.
#[derive(Debug, Clone)]
struct FreeInfo {
    /// Source line number.
    line: Option<u32>,
}

/// Information about an access (load/store) to an allocation.
#[derive(Debug, Clone)]
struct AccessInfo {
    /// Whether this is a read or write.
    is_read: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// Memory Safety Analysis Engine
// ═══════════════════════════════════════════════════════════════════════════

/// The memory safety analysis engine.
///
/// Walks the codegen SCG representation to track allocations, frees, and
/// accesses, then runs compile-time checks for use-after-free, double-free,
/// memory leaks, and uninitialized reads.
///
/// When `runtime_bounds_checks` is enabled, it also marks array access sites
/// for instrumentation with bounds-check code during codegen.
pub struct MemorySafetyAnalyzer {
    config: MemorySafetyConfig,
}

impl MemorySafetyAnalyzer {
    /// Create a new analyzer with the given configuration.
    pub fn new(config: MemorySafetyConfig) -> Self {
        Self { config }
    }

    /// Create an analyzer with the default configuration.
    pub fn with_defaults() -> Self {
        Self::new(MemorySafetyConfig::default())
    }

    /// Run memory safety analysis on a codegen SCG program.
    ///
    /// This is the primary entry point. It walks all functions in the SCG,
    /// tracks allocations and frees, then runs each enabled check.
    pub fn analyze(&self, scg: &Scg) -> MemorySafetyReport {
        let start = std::time::Instant::now();

        let mut report = MemorySafetyReport::empty();
        let mut all_allocations: HashMap<String, AllocationInfo> = HashMap::new();

        for node in &scg.nodes {
            match node {
                ScgNode::Function(func) => {
                    let func_allocs = self.analyze_function(func);
                    // Check for violations within the function
                    self.check_function(func, &func_allocs, &mut report);
                    // Merge into global tracking
                    for (name, info) in func_allocs {
                        all_allocations.insert(name, info);
                    }
                }
                ScgNode::Data(_) => {
                    // Data declarations don't have memory safety issues
                }
            }
        }

        // Global leak check: heap allocations with no frees
        if self.config.check_memory_leaks {
            self.check_memory_leaks(&all_allocations, &mut report);
        }

        report.analysis_time_us = start.elapsed().as_micros() as u64;
        report
    }

    /// Analyze a single function to track allocations, frees, and accesses.
    fn analyze_function(&self, func: &ScgFunction) -> HashMap<String, AllocationInfo> {
        let mut allocations: HashMap<String, AllocationInfo> = HashMap::new();
        self.walk_statements(&func.body, &mut allocations);
        allocations
    }

    /// Recursively walk SCG statements to collect allocation/access/free info.
    fn walk_statements(
        &self,
        stmts: &[ScgStatement],
        allocations: &mut HashMap<String, AllocationInfo>,
    ) {
        for stmt in stmts {
            match stmt {
                ScgStatement::Allocation(alloc) => {
                    match alloc {
                        AllocationNode::Stack { name, size, .. } => {
                            // VUMA 2.0 PMT-only: `state_new(Layout)` zero-initialises
                            // the backing buffer, so the allocation itself counts as a
                            // (zero) write. Record a synthetic non-read access so the
                            // uninitialised-read check (`has_read && !has_write`) does
                            // not false-positive on PMT programs that read a state
                            // field without an explicit `state.field = v` first.
                            allocations.insert(
                                name.clone(),
                                AllocationInfo {
                                    is_heap: false,
                                    size: Some(*size),
                                    line: None,
                                    frees: Vec::new(),
                                    accesses: vec![AccessInfo { is_read: false }],
                                    is_returned: false,
                                },
                            );
                        }
                        AllocationNode::Heap { name, .. } => {
                            allocations.insert(
                                name.clone(),
                                AllocationInfo {
                                    is_heap: true,
                                    size: None, // Dynamic size
                                    line: None,
                                    frees: Vec::new(),
                                    accesses: vec![AccessInfo { is_read: false }],
                                    is_returned: false,
                                },
                            );
                        }
                    }
                }
                ScgStatement::Access(access) => match access {
                    AccessNode::Load { ptr, .. } => {
                        let ptr_name = expr_to_name(ptr);
                        if let Some(info) = allocations.get_mut(&ptr_name) {
                            info.accesses.push(AccessInfo { is_read: true });
                        }
                    }
                    AccessNode::Store { ptr, .. } => {
                        let ptr_name = expr_to_name(ptr);
                        if let Some(info) = allocations.get_mut(&ptr_name) {
                            info.accesses.push(AccessInfo { is_read: false });
                        }
                    }
                },
                ScgStatement::Call(call) => {
                    // Check for deallocation calls (free, __vuma_free, etc.)
                    let func_name = &call.func;
                    if is_deallocation_call(func_name) {
                        for arg in &call.args {
                            let arg_name = expr_to_name(arg);
                            if let Some(info) = allocations.get_mut(&arg_name) {
                                info.frees.push(FreeInfo { line: None });
                            }
                        }
                    }
                }
                ScgStatement::Syscall(_) => {
                    // Syscalls are side-effecting (they may read/write user
                    // buffers via the kernel), but they do not deallocate
                    // VUMA-tracked allocations, so there is nothing to
                    // record here. Treated like a non-deallocating Call.
                }
                ScgStatement::Control(ctrl) => match ctrl {
                    ControlNode::If {
                        then_body,
                        else_body,
                        ..
                    } => {
                        self.walk_statements(then_body, allocations);
                        if let Some(else_body) = else_body {
                            self.walk_statements(else_body, allocations);
                        }
                    }
                    ControlNode::Loop { body, .. } => {
                        self.walk_statements(body, allocations);
                    }
                    ControlNode::Switch {
                        arms, default_body, ..
                    } => {
                        for arm in arms {
                            self.walk_statements(&arm.body, allocations);
                        }
                        self.walk_statements(default_body, allocations);
                    }
                    ControlNode::Break | ControlNode::Continue => {}
                },
                ScgStatement::Return(values) => {
                    // Mark any returned allocations as escaping
                    for val in values {
                        let name = expr_to_name(val);
                        if let Some(info) = allocations.get_mut(&name) {
                            info.is_returned = true;
                        }
                    }
                }
                ScgStatement::Computation(_) => {}
                ScgStatement::UnaryComputation(_) => {}
                ScgStatement::Cast(_) => {}
                ScgStatement::ConstantTime(_) => {}
                ScgStatement::StructAccess(_) => {}
                ScgStatement::EnumAccess(_) => {}
                ScgStatement::GetAddress(_) => {}
                ScgStatement::ForeignConsume(_) => {}
                // Channel operations don't deallocate VUMA-tracked
                // allocations, so there is nothing to record here. Treated
                // like Syscall/ForeignConsume (no-op for memory-safety).
                ScgStatement::ChannelOpen(_) => {}
                ScgStatement::ChannelSend(_) => {}
                ScgStatement::ChannelRecv(_) => {}
                ScgStatement::ChannelClose(_) => {}
                // Fallible recv — no heap/stack allocation to track.
                ScgStatement::ChannelRecvResult(_) => {}
            }
        }
    }

    /// Check a function for memory safety violations.
    fn check_function(
        &self,
        func: &ScgFunction,
        allocations: &HashMap<String, AllocationInfo>,
        report: &mut MemorySafetyReport,
    ) {
        for (name, info) in allocations {
            // Count stats
            if info.is_heap {
                report.heap_allocations_analyzed += 1;
            } else {
                report.stack_allocations_analyzed += 1;
            }
            report.deallocations_analyzed += info.frees.len();
            report.access_sites_analyzed += info.accesses.len();

            // ── Double-free detection ──
            if self.config.check_double_free && info.frees.len() > 1 {
                // Multiple frees on the same allocation
                let frees = &info.frees;
                for i in 1..frees.len() {
                    report.violations.push(MemorySafetyViolation::DoubleFree {
                        allocation_name: name.clone(),
                        first_free_line: frees[0].line,
                        second_free_line: frees[i].line,
                    });
                }
            }

            // ── Use-after-free detection ──
            // If there are frees, check if any access occurs after a free.
            // In the codegen SCG (which is statement-order-based), we do a
            // simplified check: if an allocation has both frees and accesses
            // that come after the free (in statement order), it's a UAF.
            if self.config.check_use_after_free && !info.frees.is_empty() {
                // Walk the function body to find accesses after frees
                let uaf_count = self.count_accesses_after_free(func, name);
                if uaf_count > 0 {
                    report.violations.push(MemorySafetyViolation::UseAfterFree {
                        allocation_name: name.clone(),
                        dealloc_line: info.frees.first().and_then(|f| f.line),
                        violation_count: uaf_count,
                    });
                }
            }

            // ── Uninitialized read detection ──
            if self.config.check_uninitialized_reads {
                let has_write = info.accesses.iter().any(|a| !a.is_read);
                let has_read = info.accesses.iter().any(|a| a.is_read);
                if has_read && !has_write && !info.is_returned {
                    // Reads without any writes (and not a parameter)
                    // This is a simplified check; a full reaching-definitions
                    // analysis would be more precise.
                    report
                        .violations
                        .push(MemorySafetyViolation::UninitializedRead {
                            variable_name: name.clone(),
                        });
                }
            }
        }
    }

    /// Count accesses to an allocation that appear after a free in statement order.
    ///
    /// This is a simplified analysis that works on the linear statement order
    /// of the codegen SCG (which represents the program's control flow in
    /// statement order). For more complex control flow (if/else, loops),
    /// the full SCG liveness analysis from `vuma-scg` should be used.
    fn count_accesses_after_free(&self, func: &ScgFunction, alloc_name: &str) -> usize {
        let mut freed = false;
        let mut count = 0;
        self.count_accesses_after_free_stmts(&func.body, alloc_name, &mut freed, &mut count);
        count
    }

    fn count_accesses_after_free_stmts(
        &self,
        stmts: &[ScgStatement],
        alloc_name: &str,
        freed: &mut bool,
        count: &mut usize,
    ) {
        for stmt in stmts {
            match stmt {
                ScgStatement::Call(call) if is_deallocation_call(&call.func) => {
                    for arg in &call.args {
                        if expr_to_name(arg) == alloc_name {
                            *freed = true;
                        }
                    }
                }
                ScgStatement::Access(access) if *freed => {
                    let ptr_name = match access {
                        AccessNode::Load { ptr, .. } => expr_to_name(ptr),
                        AccessNode::Store { ptr, .. } => expr_to_name(ptr),
                    };
                    if ptr_name == alloc_name {
                        *count += 1;
                    }
                }
                ScgStatement::Control(ctrl) => {
                    match ctrl {
                        ControlNode::If {
                            then_body,
                            else_body,
                            ..
                        } => {
                            // Check both branches conservatively
                            let mut then_freed = *freed;
                            let mut else_freed = *freed;
                            let mut then_count = 0usize;
                            let mut else_count = 0usize;
                            self.count_accesses_after_free_stmts(
                                then_body,
                                alloc_name,
                                &mut then_freed,
                                &mut then_count,
                            );
                            if let Some(else_body) = else_body {
                                self.count_accesses_after_free_stmts(
                                    else_body,
                                    alloc_name,
                                    &mut else_freed,
                                    &mut else_count,
                                );
                            }
                            // If freed in either branch, conservatively mark as freed
                            *freed = *freed || then_freed || else_freed;
                            *count += then_count + else_count;
                        }
                        ControlNode::Loop { body, .. } => {
                            // In a loop, a free in the body may free on every iteration
                            self.count_accesses_after_free_stmts(body, alloc_name, freed, count);
                        }
                        ControlNode::Switch {
                            arms, default_body, ..
                        } => {
                            for arm in arms {
                                self.count_accesses_after_free_stmts(
                                    &arm.body, alloc_name, freed, count,
                                );
                            }
                            self.count_accesses_after_free_stmts(
                                default_body,
                                alloc_name,
                                freed,
                                count,
                            );
                        }
                        ControlNode::Break | ControlNode::Continue => {}
                    }
                }
                _ => {}
            }
        }
    }

    /// Check for memory leaks: heap allocations with no matching free.
    fn check_memory_leaks(
        &self,
        allocations: &HashMap<String, AllocationInfo>,
        report: &mut MemorySafetyReport,
    ) {
        for (name, info) in allocations {
            if info.is_heap && info.frees.is_empty() && !info.is_returned {
                report.violations.push(MemorySafetyViolation::MemoryLeak {
                    allocation_name: name.clone(),
                    alloc_line: info.line,
                    alloc_size: info.size,
                });
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Runtime bounds-check instrumentation
// ═══════════════════════════════════════════════════════════════════════════

/// Represents a site where a runtime bounds check should be inserted.
#[derive(Debug, Clone)]
pub struct BoundsCheckSite {
    /// The function containing the access.
    pub function_name: String,
    /// The array/pointer being accessed.
    pub array_name: String,
    /// The index expression being used.
    pub index_expr: String,
    /// The length/bounds expression (if known).
    pub length_expr: Option<String>,
    /// Source line (best-effort).
    pub line: Option<u32>,
}

/// Scan a codegen SCG for array access sites, resolving `length_expr`
/// against a pre-built `alloc_sizes` table mapping variable names to
/// allocation sizes (bytes).
///
/// The table is built in the pipeline by walking `AllocationNode::Stack`
/// statements (state-typed buffers, stack arrays) — see
/// `pipeline::build_alloc_sizes`. PMT layout `total_size` is embedded in
/// `AllocationNode::Stack.size` at AST→SCG time, so a single table covers
/// both stack arrays and state-typed buffers.
///
/// For accesses whose `ptr` does not resolve to a name present in
/// `alloc_sizes` (e.g. pointer arithmetic, extern pointers), `length_expr`
/// remains `None` and the site is skipped at IR emission time.
pub fn find_bounds_check_sites_with_bounds(
    scg: &Scg,
    alloc_sizes: &HashMap<String, u64>,
) -> Vec<BoundsCheckSite> {
    find_bounds_check_sites_inner(scg, alloc_sizes)
}

fn find_bounds_check_sites_inner(
    scg: &Scg,
    alloc_sizes: &HashMap<String, u64>,
) -> Vec<BoundsCheckSite> {
    let mut sites = Vec::new();

    for node in &scg.nodes {
        if let ScgNode::Function(func) = node {
            find_bounds_check_sites_in_stmts(&func.name, &func.body, alloc_sizes, &mut sites);
        }
    }

    sites
}

fn find_bounds_check_sites_in_stmts(
    func_name: &str,
    stmts: &[ScgStatement],
    alloc_sizes: &HashMap<String, u64>,
    sites: &mut Vec<BoundsCheckSite>,
) {
    for stmt in stmts {
        match stmt {
            ScgStatement::Access(access) => match access {
                AccessNode::Load { ptr, offset, .. } => {
                    if offset.is_some() {
                        let array_name = expr_to_name(ptr);
                        let length_expr = alloc_sizes.get(&array_name).map(|sz| format!("{}", sz));
                        sites.push(BoundsCheckSite {
                            function_name: func_name.to_string(),
                            array_name,
                            index_expr: offset
                                .as_ref()
                                .map(|e| format!("{:?}", e))
                                .unwrap_or_default(),
                            length_expr,
                            line: None,
                        });
                    }
                }
                AccessNode::Store { ptr, offset, .. } => {
                    if offset.is_some() {
                        let array_name = expr_to_name(ptr);
                        let length_expr = alloc_sizes.get(&array_name).map(|sz| format!("{}", sz));
                        sites.push(BoundsCheckSite {
                            function_name: func_name.to_string(),
                            array_name,
                            index_expr: offset
                                .as_ref()
                                .map(|e| format!("{:?}", e))
                                .unwrap_or_default(),
                            length_expr,
                            line: None,
                        });
                    }
                }
            },
            ScgStatement::Control(ctrl) => match ctrl {
                ControlNode::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    find_bounds_check_sites_in_stmts(func_name, then_body, alloc_sizes, sites);
                    if let Some(else_body) = else_body {
                        find_bounds_check_sites_in_stmts(func_name, else_body, alloc_sizes, sites);
                    }
                }
                ControlNode::Loop { body, .. } => {
                    find_bounds_check_sites_in_stmts(func_name, body, alloc_sizes, sites);
                }
                ControlNode::Switch {
                    arms, default_body, ..
                } => {
                    for arm in arms {
                        find_bounds_check_sites_in_stmts(func_name, &arm.body, alloc_sizes, sites);
                    }
                    find_bounds_check_sites_in_stmts(func_name, default_body, alloc_sizes, sites);
                }
                ControlNode::Break | ControlNode::Continue => {}
            },
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// IR emission: inject `__oob_trap` checks at SCG level (Stage 2)
// ═══════════════════════════════════════════════════════════════════════════

/// CCured-style pointer classification for selective bounds checking.
///
/// Mirrors the CCured pointer-kind taxonomy (Necula et al., CCured 2002):
///
/// * `Safe` — state-typed, no arithmetic, no FFI. The pointer's target is
///   statically known and cannot escape; no bounds check is needed.
/// * `Seq`  — array or named allocation with a known size plus a runtime
///   offset. A `UGe offset, size` check is emitted before each access.
/// * `Wild` — FFI-derived, cast-derived, or otherwise unknown pointer with
///   an offset but no resolvable allocation. No check is emitted yet; a
///   diagnostic is logged and full checking is deferred to SoftBound fat
///   pointers (Phase 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    /// State-typed, no arithmetic, no FFI. No bounds check needed.
    Safe,
    /// Array/allocation with known size. Bounds check emitted.
    Seq,
    /// FFI/cast/unknown. No bounds check (deferred to SoftBound Phase 3).
    Wild,
}

/// Per-injection counters for `PointerKind` classification.
#[derive(Debug, Default, Clone, Copy)]
struct PointerStats {
    safe: u64,
    seq: u64,
    wild: u64,
}

/// Classify a pointer expression for bounds-check purposes.
///
/// * SAFE — no offset (direct state access) → no check.
/// * SEQ  — named allocation with known size and an offset → check.
/// * WILD — FFI/cast/unknown with an offset but no resolvable allocation
///   → diagnostic only (deferred to SoftBound Phase 3).
///
/// `has_offset` corresponds to whether the access node carries a runtime
/// `offset` (i.e. `AccessNode::{Load,Store}::offset.is_some()`).
pub fn classify_pointer(
    ptr_expr: &ScgExpr,
    has_offset: bool,
    alloc_sizes: &HashMap<String, u64>,
) -> PointerKind {
    if !has_offset {
        return PointerKind::Safe;
    }
    if let Some(name) = pointer_alloc_name(ptr_expr) {
        if alloc_sizes.contains_key(&name) {
            return PointerKind::Seq;
        }
    }
    PointerKind::Wild
}

/// Best-effort extraction of an allocation name from a pointer expression,
/// reusing the same logic as [`expr_to_name`] but exposed publicly for the
/// `classify_pointer` helper.
fn pointer_alloc_name(ptr_expr: &ScgExpr) -> Option<String> {
    match ptr_expr {
        ScgExpr::Var(name) => Some(name.clone()),
        _ => None,
    }
}

/// Inject per-access bounds-check IR into the codegen SCG in place.
///
/// For every `AccessNode::Load`/`Store` whose `ptr` resolves (via
/// [`expr_to_name`]) to a name present in `alloc_sizes`, the following
/// two SCG statements are inserted **immediately before** the access:
///
/// ```text
/// __bc_tmp_N = (offset UGe alloc_size);
/// if __bc_tmp_N { call __oob_trap(); }
/// ```
///
/// This mirrors the proven `__arena_overflow` lowering pattern
/// (pipeline.rs ~10210): a `ComputationNode(UGe)` producing a boolean
/// vreg, followed by a `ControlNode::If` whose `then_body` is a single
/// `CallNode { func: "__oob_trap", is_extern: true }`. The
/// `__oob_trap` stubs (exit code 134) exist on all 19 backends.
///
/// Accesses are classified CCured-style via [`classify_pointer`]:
///
/// * `Safe` (no offset) and `Wild` (offset but unknown allocation) are
///   left uninstrumented. `Wild` accesses additionally emit a `warn`
///   diagnostic so the user can see deferred SoftBound Phase-3 sites.
/// * `Seq` accesses receive the `__oob_trap` pair above.
///
/// **Semantics of the bound:** `alloc_sizes[name]` is the allocation's
/// total size in bytes (from `AllocationNode::Stack.size`, which for
/// state-typed buffers holds `PmtLayoutSpec.total_size`). The check
/// `offset UGe size` traps when the byte offset is at or past the
/// allocation boundary. This is the SoftBound base/bound semantic.
pub fn inject_bounds_check_ir(scg: &mut Scg, alloc_sizes: &HashMap<String, u64>) {
    let mut counter: u64 = 0;
    let mut stats = PointerStats::default();
    for node in &mut scg.nodes {
        if let ScgNode::Function(func) = node {
            inject_bounds_check_ir_in_stmts(&mut func.body, alloc_sizes, &mut counter, &mut stats);
        }
    }
    vuma_log!(
        info,
        "Pointer classification: {} SAFE, {} SEQ (checked), {} WILD (deferred)",
        stats.safe,
        stats.seq,
        stats.wild
    );
}

fn inject_bounds_check_ir_in_stmts(
    stmts: &mut Vec<ScgStatement>,
    alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut PointerStats,
) {
    let mut i = 0;
    while i < stmts.len() {
        // First, recurse into any control-flow body in stmts[i].
        inject_bounds_check_ir_in_place(&mut stmts[i], alloc_sizes, counter, stats);

        // Then check if stmts[i] is an Access that needs a bounds check
        // prepended. We extract (offset, ptr) by clone so we can borrow
        // stmts mutably for `splice` afterwards.
        let maybe_pair: Option<(ScgStatement, ScgStatement)> = match &stmts[i] {
            ScgStatement::Access(AccessNode::Load { ptr, offset, .. }) => {
                bounds_check_pair_for(ptr, offset, alloc_sizes, counter, stats)
            }
            ScgStatement::Access(AccessNode::Store { ptr, offset, .. }) => {
                bounds_check_pair_for(ptr, offset, alloc_sizes, counter, stats)
            }
            _ => None,
        };

        if let Some((cond_stmt, if_stmt)) = maybe_pair {
            // Insert the two check statements before the Access.
            stmts.splice(i..i, [cond_stmt, if_stmt]);
            i += 2; // skip the inserted pair; the original Access is now at i+2
        }
        i += 1;
    }
}

/// If `ptr` classifies as [`PointerKind::Seq`] (named allocation present
/// in `alloc_sizes` with a runtime `offset`), build a `ComputationNode(UGe)`
/// + `ControlNode::If { __oob_trap }` pair, mirroring the
///   `__arena_overflow` lowering pattern.
///
/// * `Safe` (no offset) — increment `stats.safe`, no check.
/// * `Seq` (offset + known allocation) — increment `stats.seq`, emit check.
/// * `Wild` (offset but unknown allocation) — increment `stats.wild`, log
///   a `warn` diagnostic, emit no check (deferred to SoftBound Phase 3).
fn bounds_check_pair_for(
    ptr: &ScgExpr,
    offset: &Option<ScgExpr>,
    alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut PointerStats,
) -> Option<(ScgStatement, ScgStatement)> {
    let kind = classify_pointer(ptr, offset.is_some(), alloc_sizes);
    match kind {
        PointerKind::Safe => {
            stats.safe += 1;
            None
        }
        PointerKind::Seq => {
            stats.seq += 1;
            let off_expr = offset.as_ref()?;
            let name = expr_to_name(ptr);
            let size = *alloc_sizes.get(&name)?;
            if size == 0 {
                // Zero-sized allocation: skip (no meaningful bound; the
                // access itself will segfault, separate diagnostic path).
                return None;
            }
            // [UAF liveness] Skip the LIVE-flag Store itself: it writes the
            // tombstone byte at [ptr + total_size], which is at the boundary
            // of the allocation. The bounds check UGe(total_size, total_size)
            // would be true (OOB), but this is the liveness flag, not a user
            // access. The flag Store has value=1, ty=U8, offset=alloc_size.
            if let ScgExpr::Int(off_val) = off_expr {
                if *off_val as u64 == size {
                    // This is the LIVE-flag Store — skip bounds check
                    return None;
                }
            }
            let tmp = format!("__bc_tmp_{}", *counter);
            *counter += 1;
            let cond_stmt = ScgStatement::Computation(ComputationNode {
                dst: tmp.clone(),
                op: BinOpKind::UGe,
                lhs: off_expr.clone(),
                rhs: ScgExpr::Int(size as i64),
                tail_call: false,
                reassigns: None,
            });
            let if_stmt = ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Var(tmp),
                then_body: vec![ScgStatement::Call(CallNode {
                    dst: None,
                    func: "__oob_trap".to_string(),
                    args: vec![],
                    is_extern: true,
                    reassigns: None,
                })],
                else_body: None,
            });
            Some((cond_stmt, if_stmt))
        }
        PointerKind::Wild => {
            stats.wild += 1;
            vuma_log!(
                warn,
                "WILD pointer access at '{}' (offset={:?}) — bounds check deferred to SoftBound Phase 3",
                expr_to_name(ptr),
                offset
            );
            None
        }
    }
}

fn inject_bounds_check_ir_in_place(
    stmt: &mut ScgStatement,
    alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut PointerStats,
) {
    if let ScgStatement::Control(ctrl) = stmt {
        match ctrl {
            ControlNode::If {
                then_body,
                else_body,
                ..
            } => {
                inject_bounds_check_ir_in_stmts(then_body, alloc_sizes, counter, stats);
                if let Some(eb) = else_body {
                    inject_bounds_check_ir_in_stmts(eb, alloc_sizes, counter, stats);
                }
            }
            ControlNode::Loop { body, .. } => {
                inject_bounds_check_ir_in_stmts(body, alloc_sizes, counter, stats);
            }
            ControlNode::Switch {
                arms, default_body, ..
            } => {
                for arm in arms.iter_mut() {
                    inject_bounds_check_ir_in_stmts(&mut arm.body, alloc_sizes, counter, stats);
                }
                inject_bounds_check_ir_in_stmts(default_body, alloc_sizes, counter, stats);
            }
            ControlNode::Break | ControlNode::Continue => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Per-access bounds checks for arena-allocated state pointers
// ═══════════════════════════════════════════════════════════════════════════
//
// Background: `arena_alloc(arena, Layout)` (lowered at
// `pipeline.rs:10411-10513`) returns a fresh `state_ptr = arena_ptr + offset`
// where `offset` is the arena's current bump cursor (loaded from
// `[arena_ptr + 8]`). The lowering already emits a single `__arena_overflow`
// trap at allocation time comparing `new_offset = offset + layout_size`
// against `arena.capacity` (at `[arena_ptr + 16]`).
//
// What is NOT checked: every SUBSEQUENT access through `state_ptr` with a
// runtime `offset` (e.g. `state_ptr + field_offset`). Because `state_ptr`
// is a fresh temp not present in `alloc_sizes` (the table built from
// `AllocationNode::Stack`), `classify_pointer` returns `PointerKind::Wild`
// and `inject_bounds_check_ir` skips the access — only a `warn` diagnostic
// is logged. This leaves arena-allocated state buffers unbounded at
// per-access granularity under `--safe`, which is the gap closed here.
//
// Approach: scan the codegen SCG for the deterministic IR sequence emitted
// by `arena_alloc` lowering and build a `state_ptr_name → layout_size`
// table. The pipeline merges this table into `alloc_sizes` before
// `inject_bounds_check_ir` runs, so accesses through `state_ptr` classify
// as `PointerKind::Seq` and receive the standard `__oob_trap` pair.
//
// The sequence is anchored on the `__arena_overflow` runtime trap call
// (uniquely emitted by `arena_alloc` lowering — `pipeline.rs:10479` is the
// sole producer). The anchor guarantees we only register state pointers
// from actual `arena_alloc` sites, avoiding false positives from other
// `Add(Var, Var)` computations.
//
// **Bound semantics:** the bound is the state's `layout_size` (the size of
// the layout passed to `arena_alloc`), NOT the arena's capacity. The
// arena's capacity is already checked at `arena_alloc` time via
// `__arena_overflow`; the per-access check catches out-of-layout field
// accesses (e.g. `*(state_ptr + layout_size)` would trap, since
// `layout_size UGe layout_size` is true). This mirrors the SEQ bound for
// stack allocations, where `alloc_sizes[name]` is the allocation's total
// size in bytes.

/// Build a `state_ptr_name → layout_size_in_bytes` table by scanning the
/// codegen SCG for the deterministic IR sequence emitted by `arena_alloc`
/// lowering.
///
/// The scan is anchored on `__arena_overflow` trap calls (uniquely emitted
/// by `Expr::ArenaAlloc` lowering at `pipeline.rs:10479`). For each anchor,
/// the surrounding `Computation` statements are pattern-matched to extract:
///
/// * `offset_val` — the variable holding the arena's current bump cursor
///   (loaded from `[arena_ptr + 8]`)
/// * `layout_size` — the static layout size of the allocated state (from
///   `new_offset = offset_val + layout_size`)
/// * `state_ptr` — the variable holding the returned state pointer
///   (`state_ptr = arena_ptr + offset_val`)
///
/// The returned table is merged into `alloc_sizes` by the pipeline before
/// `inject_bounds_check_ir` runs, so that per-access `__oob_trap` checks
/// are emitted for accesses through arena-allocated state pointers (which
/// would otherwise classify as `PointerKind::Wild` because `state_ptr` is
/// a fresh temp not present in `alloc_sizes`).
///
/// Returns an empty table if no `arena_alloc` sites are present.
pub fn build_arena_state_sizes(scg: &Scg) -> HashMap<String, u64> {
    let mut table = HashMap::new();
    for node in &scg.nodes {
        if let ScgNode::Function(func) = node {
            collect_arena_state_sizes_in_stmts(&func.body, &mut table);
        }
    }
    table
}

fn collect_arena_state_sizes_in_stmts(stmts: &[ScgStatement], table: &mut HashMap<String, u64>) {
    // Pass 1: pattern-match arena_alloc IR sequences anchored on
    // `__arena_overflow` calls. The arena_alloc lowering emits the
    // sequence as a flat run of statements within the parent `stmts` vec
    // (no intervening control flow except for the overflow If itself), so
    // a backward/forward scan within `stmts` is sufficient.
    for (i, stmt) in stmts.iter().enumerate() {
        if let ScgStatement::Control(ControlNode::If { then_body, .. }) = stmt {
            // Verify this is the arena_alloc overflow check: then_body is
            // exactly `[Call { func: "__arena_overflow", is_extern: true }]`.
            let is_arena_overflow = then_body.len() == 1
                && matches!(
                    &then_body[0],
                    ScgStatement::Call(c) if c.func == "__arena_overflow" && c.is_extern
                );
            if is_arena_overflow {
                if let Some((state_ptr, layout_size)) = resolve_arena_alloc_pattern(stmts, i) {
                    if layout_size > 0 {
                        table.insert(state_ptr, layout_size as u64);
                    }
                }
            }
        }
    }
    // Pass 2: recurse into all control-flow bodies (the arena_alloc IR
    // sequence may be nested inside `if`/`loop`/`switch` bodies).
    for stmt in stmts {
        if let ScgStatement::Control(ctrl) = stmt {
            match ctrl {
                ControlNode::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    collect_arena_state_sizes_in_stmts(then_body, table);
                    if let Some(eb) = else_body {
                        collect_arena_state_sizes_in_stmts(eb, table);
                    }
                }
                ControlNode::Loop { body, .. } => {
                    collect_arena_state_sizes_in_stmts(body, table);
                }
                ControlNode::Switch {
                    arms, default_body, ..
                } => {
                    for arm in arms {
                        collect_arena_state_sizes_in_stmts(&arm.body, table);
                    }
                    collect_arena_state_sizes_in_stmts(default_body, table);
                }
                ControlNode::Break | ControlNode::Continue => {}
            }
        }
    }
}

/// Resolve a single `arena_alloc` IR sequence anchored at `stmts[if_idx]`
/// (the `__arena_overflow` If) into a `(state_ptr, layout_size)` pair.
///
/// Returns `None` if the surrounding pattern does not match (e.g. the If
/// is not actually an `arena_alloc` overflow check, or the IR has been
/// mutated in a way that breaks the pattern).
fn resolve_arena_alloc_pattern(stmts: &[ScgStatement], if_idx: usize) -> Option<(String, i64)> {
    let (cond, then_body) = match &stmts[if_idx] {
        ScgStatement::Control(ControlNode::If {
            cond, then_body, ..
        }) => (cond, then_body),
        _ => return None,
    };
    // Re-verify the anchor (defensive — caller already checked).
    let is_arena_overflow = then_body.len() == 1
        && matches!(
            &then_body[0],
            ScgStatement::Call(c) if c.func == "__arena_overflow" && c.is_extern
        );
    if !is_arena_overflow {
        return None;
    }
    // Step 1: find `overflow_cond = new_offset UGt cap_val` by scanning
    // backward from the If. `overflow_cond` is the If's cond variable.
    let cond_var = match cond {
        ScgExpr::Var(v) => v.as_str(),
        _ => return None,
    };
    let mut new_offset_var: Option<String> = None;
    for prev in stmts[..if_idx].iter().rev() {
        if let ScgStatement::Computation(c) = prev {
            if c.dst == cond_var && c.op == BinOpKind::UGt {
                if let ScgExpr::Var(lhs) = &c.lhs {
                    new_offset_var = Some(lhs.clone());
                    break;
                }
            }
        }
    }
    let new_offset_var = new_offset_var?;
    // Step 2: find `new_offset = offset_val + layout_size` by scanning
    // backward. This gives us `offset_val` and `layout_size`.
    let mut offset_val_and_layout: Option<(String, i64)> = None;
    for prev in stmts[..if_idx].iter().rev() {
        if let ScgStatement::Computation(c) = prev {
            if c.dst == new_offset_var && c.op == BinOpKind::Add {
                if let (ScgExpr::Var(off), ScgExpr::Int(layout)) = (&c.lhs, &c.rhs) {
                    offset_val_and_layout = Some((off.clone(), *layout));
                    break;
                }
            }
        }
    }
    let (offset_val, layout_size) = offset_val_and_layout?;
    // Step 3: find `state_ptr = arena_ptr + offset_val` by scanning forward
    // from the If. The `state_ptr` is the dst of the first `Add(Var, Var)`
    // computation whose `rhs` matches `offset_val`.
    for next in stmts[if_idx + 1..].iter() {
        if let ScgStatement::Computation(c) = next {
            if c.op == BinOpKind::Add {
                if let (ScgExpr::Var(_), ScgExpr::Var(rhs)) = (&c.lhs, &c.rhs) {
                    if rhs == &offset_val {
                        return Some((c.dst.clone(), layout_size));
                    }
                }
            }
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Runtime UAF detection via tombstone flag
// ═══════════════════════════════════════════════════════════════════════════
//
// Each state allocation (`let p = state_new(Layout)`) is grown by +1 byte
// at AST→codegen-SCG bridge time (see `pipeline.rs:bridge_stmt_to_scg`),
// and a LIVE flag (=1) is stored at `[ptr + total_size]`. This module
// scans the codegen SCG for that (Allocation + LIVE-flag Store) pattern,
// builds a `name → flag_offset` table, and injects a runtime check before
// every SEQ access through that pointer:
//
// ```text
// __lc_flag_N = *(u8*)(ptr + flag_offset);
// __lc_dead_N = (__lc_flag_N == 0);
// if __lc_dead_N { call __uaf_trap(); }   // exit 135
// ```
//
// `__uaf_trap` stubs already exist on all 19 backends (added in INV-UAF-1).
// The check is a no-op for live states (flag == 1 → eq 0 → false → no
// trap). If a future `state_consume`/drop pass flips the flag to 0, the
// next access traps with exit code 135.

/// Build a `var_name → flag_offset` table by scanning the codegen SCG for
/// the LIVE-flag-store pattern emitted at `state_new` lowering time:
///
/// ```text
/// AccessNode::Store { ptr: Var(name), offset: Some(Int(N)),
///                     value: Int(1), ty: Some(U8) }
/// ```
///
/// with `N + 1 == alloc_sizes[name]` (i.e. the Store writes the very last
/// byte of the allocation — the tombstone). Returns `name → N` so callers
/// can compute the flag address as `ptr + N`.
///
/// We do NOT require the LIVE-flag Store to be immediately adjacent to the
/// `AllocationNode::Stack`: `inject_bounds_check_ir` runs first and may
/// insert `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }`
/// between the allocation and the flag Store. Pattern-matching on the
/// Store signature alone (with the `N + 1 == alloc_sizes[name]` cross-
/// check) is robust against this interleaving.
///
/// Non-state `AllocationNode::Stack` entries (raw `allocate(N)`) do not
/// emit the LIVE-flag Store, so they are absent from the returned table —
/// `inject_liveness_check_ir` will skip them, preserving existing
/// behaviour for raw stack arrays.
pub fn build_state_liveness_offsets(
    scg: &Scg,
    alloc_sizes: &HashMap<String, u64>,
) -> HashMap<String, u64> {
    let mut table = HashMap::new();
    for node in &scg.nodes {
        if let ScgNode::Function(func) = node {
            collect_state_liveness_in_stmts(&func.body, alloc_sizes, &mut table);
        }
    }
    table
}

fn collect_state_liveness_in_stmts(
    stmts: &[ScgStatement],
    alloc_sizes: &HashMap<String, u64>,
    table: &mut HashMap<String, u64>,
) {
    for stmt in stmts {
        match stmt {
            ScgStatement::Access(AccessNode::Store {
                ptr,
                offset,
                value,
                ty,
            }) => {
                if let (ScgExpr::Var(name), Some(ScgExpr::Int(off)), ScgExpr::Int(1)) =
                    (ptr, offset, value)
                {
                    if matches!(ty, Some(crate::ir::IRType::U8)) {
                        if let Some(&alloc_sz) = alloc_sizes.get(name) {
                            if (*off as u64) + 1 == alloc_sz {
                                table.insert(name.clone(), *off as u64);
                            }
                        }
                    }
                }
            }
            ScgStatement::Control(ctrl) => match ctrl {
                ControlNode::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    collect_state_liveness_in_stmts(then_body, alloc_sizes, table);
                    if let Some(eb) = else_body {
                        collect_state_liveness_in_stmts(eb, alloc_sizes, table);
                    }
                }
                ControlNode::Loop { body, .. } => {
                    collect_state_liveness_in_stmts(body, alloc_sizes, table);
                }
                ControlNode::Switch {
                    arms, default_body, ..
                } => {
                    for arm in arms.iter() {
                        collect_state_liveness_in_stmts(&arm.body, alloc_sizes, table);
                    }
                    collect_state_liveness_in_stmts(default_body, alloc_sizes, table);
                }
                ControlNode::Break | ControlNode::Continue => {}
            },
            _ => {}
        }
    }
}

/// Inject runtime liveness (UAF) checks into the codegen SCG in place.
///
/// For every `AccessNode::Load`/`Store` whose `ptr` resolves to a name
/// present in [`build_state_liveness_offsets`] (i.e. a `state_new`
/// allocation with a tombstone flag), the following three SCG statements
/// are inserted **immediately before** the access:
///
/// ```text
/// __lc_flag_N = *(u8*)(ptr + flag_offset);   // Load tombstone
/// __lc_dead_N = (__lc_flag_N == 0);          // Compare DEAD
/// if __lc_dead_N { call __uaf_trap(); }      // Trap (exit 135)
/// ```
///
/// Mirrors the proven `__oob_trap` lowering pattern (see
/// [`inject_bounds_check_ir`]). The `__uaf_trap` stubs (exit 135) exist
/// on all 19 backends. The LIVE-flag Store emitted at allocation time is
/// skipped (otherwise the check would read uninitialised memory before
/// the flag is set). Accesses classified as `Safe` (no offset) are also
/// skipped — they correspond to direct state-typed reads where the
/// liveness invariant is enforced at compile time by the IVE.
pub fn inject_liveness_check_ir(scg: &mut Scg, alloc_sizes: &HashMap<String, u64>) {
    let state_offsets = build_state_liveness_offsets(scg, alloc_sizes);
    if state_offsets.is_empty() {
        // No state_new allocations — nothing to instrument.
        return;
    }
    let mut counter: u64 = 0;
    let mut stats = LivenessStats::default();
    for node in &mut scg.nodes {
        if let ScgNode::Function(func) = node {
            inject_liveness_check_ir_in_stmts(
                &mut func.body,
                &state_offsets,
                alloc_sizes,
                &mut counter,
                &mut stats,
            );
        }
    }
    vuma_log!(
        info,
        "Liveness check injection: {} state allocs, {} checks inserted, {} skipped (LIVE-flag store / non-seq)",
        state_offsets.len(),
        stats.inserted,
        stats.skipped
    );
}

#[derive(Default)]
struct LivenessStats {
    inserted: usize,
    skipped: usize,
}

fn inject_liveness_check_ir_in_stmts(
    stmts: &mut Vec<ScgStatement>,
    state_offsets: &HashMap<String, u64>,
    alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut LivenessStats,
) {
    let mut i = 0;
    while i < stmts.len() {
        // Recurse into control-flow bodies first.
        inject_liveness_check_ir_in_place(stmts, i, state_offsets, alloc_sizes, counter, stats);

        // Determine whether stmts[i] is an Access that needs a liveness
        // check prepended. We clone the relevant fields to avoid holding
        // an immutable borrow across the `splice` below.
        let maybe_check: Option<Vec<ScgStatement>> = match &stmts[i] {
            ScgStatement::Access(AccessNode::Load { ptr, offset, .. }) => {
                liveness_check_for(ptr, offset, state_offsets, alloc_sizes, counter, stats)
            }
            ScgStatement::Access(AccessNode::Store {
                ptr,
                offset,
                value,
                ty,
                ..
            }) => {
                // Skip the LIVE-flag Store itself: it writes the tombstone
                // at the flag offset with value 1 and ty U8. Injecting a
                // check before it would read uninitialised memory.
                if is_live_flag_store(ptr, offset, value, ty, state_offsets) {
                    stats.skipped += 1;
                    None
                } else {
                    liveness_check_for(ptr, offset, state_offsets, alloc_sizes, counter, stats)
                }
            }
            _ => None,
        };

        if let Some(check) = maybe_check {
            let n = check.len();
            stmts.splice(i..i, check);
            i += n;
        }
        i += 1;
    }
}

fn inject_liveness_check_ir_in_place(
    stmts: &mut [ScgStatement],
    i: usize,
    state_offsets: &HashMap<String, u64>,
    alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut LivenessStats,
) {
    if let ScgStatement::Control(ctrl) = &mut stmts[i] {
        match ctrl {
            ControlNode::If {
                then_body,
                else_body,
                ..
            } => {
                inject_liveness_check_ir_in_stmts(
                    then_body,
                    state_offsets,
                    alloc_sizes,
                    counter,
                    stats,
                );
                if let Some(eb) = else_body {
                    inject_liveness_check_ir_in_stmts(
                        eb,
                        state_offsets,
                        alloc_sizes,
                        counter,
                        stats,
                    );
                }
            }
            ControlNode::Loop { body, .. } => {
                inject_liveness_check_ir_in_stmts(body, state_offsets, alloc_sizes, counter, stats);
            }
            ControlNode::Switch {
                arms, default_body, ..
            } => {
                for arm in arms.iter_mut() {
                    inject_liveness_check_ir_in_stmts(
                        &mut arm.body,
                        state_offsets,
                        alloc_sizes,
                        counter,
                        stats,
                    );
                }
                inject_liveness_check_ir_in_stmts(
                    default_body,
                    state_offsets,
                    alloc_sizes,
                    counter,
                    stats,
                );
            }
            ControlNode::Break | ControlNode::Continue => {}
        }
    }
}

/// Detect the LIVE-flag Store emitted by the AST→SCG bridge at
/// `state_new` lowering time. Such a Store writes Int(1) at the flag
/// offset with type U8 — we must NOT inject a liveness check before it
/// (the flag is not yet set).
fn is_live_flag_store(
    ptr: &ScgExpr,
    offset: &Option<ScgExpr>,
    value: &ScgExpr,
    ty: &Option<crate::ir::IRType>,
    state_offsets: &HashMap<String, u64>,
) -> bool {
    if !matches!(value, ScgExpr::Int(1)) {
        return false;
    }
    if !matches!(ty, Some(crate::ir::IRType::U8)) {
        return false;
    }
    let (ScgExpr::Var(name), Some(ScgExpr::Int(off))) = (ptr, offset) else {
        return false;
    };
    state_offsets.get(name) == Some(&(*off as u64))
}

/// Build the three-statement liveness check sequence for a single Access,
/// mirroring the `ComputationNode + ControlNode::If { __oob_trap }` pair
/// used by [`inject_bounds_check_ir`]. Returns `None` if the access does
/// not need a check (Safe / WILD / non-state allocation).
fn liveness_check_for(
    ptr: &ScgExpr,
    offset: &Option<ScgExpr>,
    state_offsets: &HashMap<String, u64>,
    _alloc_sizes: &HashMap<String, u64>,
    counter: &mut u64,
    stats: &mut LivenessStats,
) -> Option<Vec<ScgStatement>> {
    // Only SEQ accesses (named allocation + runtime offset) need a
    // runtime liveness check. Direct `*p` reads (no offset) are enforced
    // at compile time by the IVE.
    let name = pointer_alloc_name(ptr)?;
    if !offset.is_some() {
        return None;
    }
    let flag_offset = state_offsets.get(&name)?;
    let flag_var = format!("__lc_flag_{}", *counter);
    let dead_var = format!("__lc_dead_{}", *counter);
    *counter += 1;
    stats.inserted += 1;
    Some(vec![
        // 1. Load liveness flag (1 byte at [ptr + flag_offset]).
        ScgStatement::Access(AccessNode::Load {
            dst: flag_var.clone(),
            ptr: ptr.clone(),
            offset: Some(ScgExpr::Int(*flag_offset as i64)),
            ty: Some(crate::ir::IRType::U8),
        }),
        // 2. Compare flag == 0 (DEAD).
        ScgStatement::Computation(ComputationNode {
            dst: dead_var.clone(),
            op: BinOpKind::Eq,
            lhs: ScgExpr::Var(flag_var),
            rhs: ScgExpr::Int(0),
            tail_call: false,
            reassigns: None,
        }),
        // 3. If dead, trap.
        ScgStatement::Control(ControlNode::If {
            cond: ScgExpr::Var(dead_var),
            then_body: vec![ScgStatement::Call(CallNode {
                dst: None,
                func: "__uaf_trap".to_string(),
                args: vec![],
                is_extern: true,
                reassigns: None,
            })],
            else_body: None,
        }),
    ])
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════════════════

/// Check if a function name is a deallocation call.
fn is_deallocation_call(name: &str) -> bool {
    matches!(
        name,
        "free" | "__vuma_free" | "dealloc" | "deallocate" | "drop" | "__builtin_free"
    )
}

/// Extract a variable name from an SCG expression (best-effort).
fn expr_to_name(expr: &ScgExpr) -> String {
    match expr {
        ScgExpr::Var(name) => name.clone(),
        ScgExpr::Int(_) => "<const>".to_string(),
        ScgExpr::Float(_) => "<const>".to_string(),
        ScgExpr::Label(name) => format!("<label:{}>", name),
        ScgExpr::BinOp { .. } => "<binop>".to_string(),
        ScgExpr::Load { .. } => "<load>".to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration with full SCG liveness analysis (vuma-scg)
// ═══════════════════════════════════════════════════════════════════════════

/// Run memory safety analysis using the full SCG liveness analysis from
/// `vuma-scg`. This provides more precise use-after-free and dead-allocation
/// detection by leveraging the graph-based SCG rather than the simplified
/// codegen SCG.
///
/// This function is called from the main pipeline when the full SCG is
/// available (after AST → SCG conversion). The results supplement the
/// codegen-level analysis.
pub fn analyze_with_scg_liveness(
    scg_liveness: &vuma_scg::liveness::LivenessAnalysis,
    scg: &vuma_scg::graph::SCG,
    config: &MemorySafetyConfig,
) -> Vec<MemorySafetyViolation> {
    let mut violations = Vec::new();

    // Use-after-free detection via liveness analysis
    if config.check_use_after_free {
        let uaf_violations = vuma_scg::liveness::find_use_after_free(scg, &scg_liveness.liveness);
        for uaf in &uaf_violations {
            violations.push(MemorySafetyViolation::UseAfterFree {
                allocation_name: format!("node_{}", uaf.allocation),
                dealloc_line: None, // Could be resolved with source mapping
                violation_count: uaf.violating_uses.len(),
            });
        }
    }

    // Double-free detection.
    //
    // Audit caveat (now resolved): previously the `check_double_free` config
    // flag was silently ignored here — double-free was detected only by the
    // codegen-level `MemorySafetyAnalyzer::analyze` at Stage 8.  This
    // SCG-level check now honours the flag.
    //
    // Algorithm: walk SCG nodes in deterministic `NodeId` order, tracking
    // which `RegionId`s are currently in the "deallocated" state.  An
    // `Allocation` node for a region clears the deallocated state (so a
    // re-allocation followed by a fresh free is not a double-free); a
    // `Deallocation` node on an already-deallocated region with no
    // intervening allocation is a double-free (E042).
    //
    // Violations are always pushed onto the returned `Vec`; whether they
    // become HARD errors depends on `config.errors_are_fatal` at the caller
    // (Stage 6b in the pipeline treats any non-empty result as fatal when
    // `errors_are_fatal == true`).
    if config.check_double_free {
        // Collect and sort nodes for deterministic iteration order.  The
        // underlying `petgraph`-style storage does not guarantee a stable
        // iteration order, so we sort by `NodeId` (which corresponds to
        // insertion order in the SCG builder).
        let mut nodes: Vec<&vuma_scg::node::NodeData> = scg.nodes().collect();
        nodes.sort_by_key(|n| n.id);

        // Region IDs that are currently in the deallocated state.
        let mut deallocated_regions: HashSet<vuma_scg::region::RegionId> = HashSet::new();
        // First deallocation `NodeId` per region, for diagnostic line info.
        let mut first_dealloc: HashMap<vuma_scg::region::RegionId, vuma_scg::node::NodeId> =
            HashMap::new();

        for node in &nodes {
            match &node.payload {
                vuma_scg::node::NodePayload::Allocation(alloc) => {
                    // An intervening allocation clears the deallocated
                    // state for the region — a subsequent free is not a
                    // double-free.
                    deallocated_regions.remove(&alloc.region_id);
                    first_dealloc.remove(&alloc.region_id);
                }
                vuma_scg::node::NodePayload::Deallocation(dealloc) => {
                    if deallocated_regions.contains(&dealloc.region_id) {
                        // Double-free: region already deallocated with no
                        // intervening allocation.
                        let first_node_id = first_dealloc.get(&dealloc.region_id).copied();
                        let first_free_line = first_node_id
                            .and_then(|nid| scg.get_node(nid))
                            .and_then(|nd| nd.program_point.line)
                            .map(|l| l as u32);
                        let second_free_line = node.program_point.line.map(|l| l as u32);
                        violations.push(MemorySafetyViolation::DoubleFree {
                            allocation_name: format!("region_{}", dealloc.region_id.as_u64()),
                            first_free_line,
                            second_free_line,
                        });
                    } else {
                        // First deallocation of this region — record it.
                        deallocated_regions.insert(dealloc.region_id);
                        first_dealloc.insert(dealloc.region_id, node.id);
                    }
                }
                _ => {}
            }
        }
    }

    // Uninitialized read detection
    if config.check_uninitialized_reads {
        let uninit_reads =
            vuma_scg::liveness::find_uninitialized_reads(scg, &scg_liveness.liveness);
        for node_id in &uninit_reads {
            violations.push(MemorySafetyViolation::UninitializedRead {
                variable_name: format!("node_{}", node_id),
            });
        }
    }

    // Dead allocation detection (potential memory leaks)
    if config.check_memory_leaks {
        let dead_allocs = vuma_scg::liveness::find_dead_allocations(scg, &scg_liveness.liveness);
        for node_id in &dead_allocs {
            violations.push(MemorySafetyViolation::MemoryLeak {
                allocation_name: format!("node_{}", node_id),
                alloc_line: None,
                alloc_size: None,
            });
        }
    }

    violations
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_violation_codes() {
        let v = MemorySafetyViolation::UseAfterFree {
            allocation_name: "buf".to_string(),
            dealloc_line: Some(10),
            violation_count: 2,
        };
        assert_eq!(v.code(), "E041");

        let v = MemorySafetyViolation::DoubleFree {
            allocation_name: "buf".to_string(),
            first_free_line: Some(10),
            second_free_line: Some(15),
        };
        assert_eq!(v.code(), "E042");

        let v = MemorySafetyViolation::MemoryLeak {
            allocation_name: "buf".to_string(),
            alloc_line: Some(5),
            alloc_size: Some(256),
        };
        assert_eq!(v.code(), "E043");

        let v = MemorySafetyViolation::BoundsCheckFailure {
            array_name: "arr".to_string(),
            index: 10,
            length: 5,
        };
        assert_eq!(v.code(), "E044");

        let v = MemorySafetyViolation::NullDereference {
            pointer_name: "ptr".to_string(),
        };
        assert_eq!(v.code(), "E045");

        let v = MemorySafetyViolation::DanglingPointer {
            pointer_name: "ptr".to_string(),
            scope_name: "inner".to_string(),
        };
        assert_eq!(v.code(), "E046");

        let v = MemorySafetyViolation::UninitializedRead {
            variable_name: "x".to_string(),
        };
        assert_eq!(v.code(), "E047");

        let v = MemorySafetyViolation::BufferOverflow {
            buffer_name: "buf".to_string(),
            offset: 1024,
            buffer_size: 256,
        };
        assert_eq!(v.code(), "E048");

        let v = MemorySafetyViolation::UseAfterScope {
            variable_name: "x".to_string(),
            scope_name: "block".to_string(),
        };
        assert_eq!(v.code(), "E049");

        let v = MemorySafetyViolation::InvalidFree {
            pointer_name: "ptr".to_string(),
            reason: "not a heap pointer".to_string(),
        };
        assert_eq!(v.code(), "E050");
    }

    #[test]
    fn test_safe_mode_config() {
        let config = MemorySafetyConfig::safe_mode();
        assert!(config.runtime_bounds_checks);
        assert!(config.check_use_after_free);
        assert!(config.check_double_free);
        assert!(config.check_memory_leaks);
    }

    #[test]
    fn test_empty_report_is_clean() {
        let report = MemorySafetyReport::empty();
        assert!(report.is_clean());
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn test_empty_scg_analysis() {
        let scg = Scg { nodes: vec![] };
        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);
        assert!(report.is_clean());
    }

    #[test]
    fn test_is_deallocation_call() {
        assert!(is_deallocation_call("free"));
        assert!(is_deallocation_call("__vuma_free"));
        assert!(is_deallocation_call("dealloc"));
        assert!(!is_deallocation_call("malloc"));
        assert!(!is_deallocation_call("alloc"));
    }

    #[test]
    fn test_double_free_detection() {
        use crate::scg_to_ir::CallNode;

        // Create a function that allocates and frees twice
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_double_free".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Heap {
                        name: "buf".to_string(),
                        size_expr: ScgExpr::Int(256),
                        ty: ScgType::Ptr,
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                        reassigns: None,
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                        reassigns: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);

        assert!(!report.is_clean());
        let double_frees = report.violations_by_code("E042");
        assert_eq!(double_frees.len(), 1);
    }

    #[test]
    fn test_memory_leak_detection() {
        // Create a function that allocates but never frees
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_leak".to_string(),
                params: vec![],
                results: vec![],
                body: vec![ScgStatement::Allocation(AllocationNode::Heap {
                    name: "buf".to_string(),
                    size_expr: ScgExpr::Int(256),
                    ty: ScgType::Ptr,
                })],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);

        let leaks = report.violations_by_code("E043");
        assert_eq!(leaks.len(), 1);
    }

    #[test]
    fn test_use_after_free_detection() {
        use crate::scg_to_ir::CallNode;

        // Create a function that frees then accesses
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_uaf".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Heap {
                        name: "buf".to_string(),
                        size_expr: ScgExpr::Int(256),
                        ty: ScgType::Ptr,
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                        reassigns: None,
                    }),
                    ScgStatement::Access(AccessNode::Load {
                        dst: "val".to_string(),
                        ptr: ScgExpr::Var("buf".to_string()),
                        offset: None,
                        ty: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);

        let uaf = report.violations_by_code("E041");
        assert_eq!(uaf.len(), 1);
    }

    #[test]
    fn test_no_violation_for_proper_usage() {
        use crate::scg_to_ir::CallNode;

        // Create a function that allocates, uses, and properly frees
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_proper".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Heap {
                        name: "buf".to_string(),
                        size_expr: ScgExpr::Int(256),
                        ty: ScgType::Ptr,
                    }),
                    ScgStatement::Access(AccessNode::Store {
                        ptr: ScgExpr::Var("buf".to_string()),
                        offset: None,
                        value: ScgExpr::Int(42),
                        ty: None,
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                        reassigns: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);

        // Should have no violations (proper usage)
        assert!(report.is_clean());
    }

    #[test]
    fn test_bounds_check_site_with_bounds_populates_length_expr() {
        // Stage 2: when an `alloc_sizes` table is
        // supplied, `find_bounds_check_sites_with_bounds` populates
        // `length_expr` for accesses whose `ptr` resolves to a known
        // allocation name.
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_bounds_with_table".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Stack {
                        name: "arr".to_string(),
                        size: 100,
                        ty: ScgType::U32,
                    }),
                    ScgStatement::Access(AccessNode::Load {
                        dst: "val".to_string(),
                        ptr: ScgExpr::Var("arr".to_string()),
                        offset: Some(ScgExpr::Var("i".to_string())),
                        ty: None,
                    }),
                    // Access to an unknown pointer — length_expr must
                    // remain None (the fallback for raw pointer
                    // arithmetic / extern pointers).
                    ScgStatement::Access(AccessNode::Store {
                        ptr: ScgExpr::Var("ext_ptr".to_string()),
                        offset: Some(ScgExpr::Int(8)),
                        value: ScgExpr::Int(0),
                        ty: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        // Backward-compat: empty table → all length_expr None.
        let empty_table: HashMap<String, u64> = HashMap::new();
        let sites_empty = find_bounds_check_sites_with_bounds(&scg, &empty_table);
        assert_eq!(sites_empty.len(), 2);
        assert!(sites_empty.iter().all(|s| s.length_expr.is_none()));

        // Populated table: known alloc → Some("100"); unknown → None.
        let mut table: HashMap<String, u64> = HashMap::new();
        table.insert("arr".to_string(), 100);
        let sites = find_bounds_check_sites_with_bounds(&scg, &table);
        assert_eq!(sites.len(), 2);
        let arr_site = sites.iter().find(|s| s.array_name == "arr").unwrap();
        assert_eq!(arr_site.length_expr.as_deref(), Some("100"));
        let ext_site = sites.iter().find(|s| s.array_name == "ext_ptr").unwrap();
        assert!(ext_site.length_expr.is_none());
    }

    #[test]
    fn test_inject_bounds_check_ir_inserts_oob_trap() {
        // Stage 2: `inject_bounds_check_ir` mutates
        // the codegen SCG in place, inserting a `ComputationNode(UGe)` +
        // `ControlNode::If { __oob_trap }` pair BEFORE every Access whose
        // `ptr` resolves to a known allocation name.
        let mut scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_inject".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Stack {
                        name: "arr".to_string(),
                        size: 64,
                        ty: ScgType::U32,
                    }),
                    ScgStatement::Access(AccessNode::Load {
                        dst: "val".to_string(),
                        ptr: ScgExpr::Var("arr".to_string()),
                        offset: Some(ScgExpr::Var("i".to_string())),
                        ty: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let mut table: HashMap<String, u64> = HashMap::new();
        table.insert("arr".to_string(), 64);
        inject_bounds_check_ir(&mut scg, &table);

        let func = match &scg.nodes[0] {
            ScgNode::Function(f) => f,
            _ => panic!("expected Function node"),
        };
        // Body should now be: [Allocation, Computation(UGe), Control(If), Access]
        assert_eq!(func.body.len(), 4);
        assert!(matches!(&func.body[1], ScgStatement::Computation(c) if c.op == BinOpKind::UGe));
        if let ScgStatement::Control(ControlNode::If { then_body, .. }) = &func.body[2] {
            assert_eq!(then_body.len(), 1);
            assert!(matches!(
                &then_body[0],
                ScgStatement::Call(c) if c.func == "__oob_trap" && c.is_extern
            ));
        } else {
            panic!("expected ControlNode::If with __oob_trap call");
        }
        assert!(matches!(&func.body[3], ScgStatement::Access(_)));
    }

    #[test]
    fn test_violation_display() {
        let v = MemorySafetyViolation::UseAfterFree {
            allocation_name: "buf".to_string(),
            dealloc_line: Some(10),
            violation_count: 3,
        };
        let s = format!("{}", v);
        assert!(s.contains("E041"));
        assert!(s.contains("use-after-free"));
        assert!(s.contains("buf"));
    }

    #[test]
    fn test_report_display() {
        let mut report = MemorySafetyReport::empty();
        report.heap_allocations_analyzed = 5;
        report.stack_allocations_analyzed = 3;

        let clean_display = format!("{}", report);
        assert!(clean_display.contains("CLEAN"));

        report.violations.push(MemorySafetyViolation::MemoryLeak {
            allocation_name: "buf".to_string(),
            alloc_line: Some(5),
            alloc_size: Some(256),
        });

        let dirty_display = format!("{}", report);
        assert!(dirty_display.contains("violation"));
    }

    // ── SCG-level double-free regression tests ─────────────────────────────
    //
    // The codegen-level `MemorySafetyAnalyzer::analyze` has always detected
    // double-free, but the SCG-liveness variant `analyze_with_scg_liveness`
    // previously silently ignored `check_double_free`.
    // These tests exercise the SCG-level check directly.

    /// Helper: build an SCG with one allocation in `region` and `n_frees`
    /// deallocations of that region.  Returns the SCG.
    fn build_scg_with_region_frees(
        region: vuma_scg::region::RegionId,
        n_frees: usize,
    ) -> vuma_scg::graph::SCG {
        use vuma_scg::{
            AllocationNode, DeallocationNode, NodeId, NodePayload, NodeType, ProgramPoint, SCG,
        };

        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(10),
            column: Some(1),
            offset: None,
        };

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: Some("Buf".to_string()),
            }),
            pp.clone(),
        );

        for i in 1..=n_frees {
            let _dealloc_id: NodeId = scg.add_node(
                NodeType::Deallocation,
                NodePayload::Deallocation(DeallocationNode {
                    allocation_node: alloc_id,
                    region_id: region,
                }),
                ProgramPoint {
                    file: Some("test.vu".to_string()),
                    line: Some(10 + i as u64),
                    column: Some(1),
                    offset: None,
                },
            );
        }
        scg
    }

    /// A single region freed twice → one DoubleFree (E042) violation.
    #[test]
    fn test_wave20_double_free_detected() {
        let region = vuma_scg::region::RegionId::new(42);
        let scg = build_scg_with_region_frees(region, 2);
        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);

        let config = MemorySafetyConfig {
            check_use_after_free: false,
            check_double_free: true,
            check_uninitialized_reads: false,
            check_memory_leaks: false,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };

        let violations = analyze_with_scg_liveness(&liveness, &scg, &config);
        let double_frees: Vec<_> = violations.iter().filter(|v| v.code() == "E042").collect();
        assert_eq!(
            double_frees.len(),
            1,
            "expected exactly one DoubleFree violation, got {:?}",
            violations
        );
        // Verify the allocation_name encodes the region id and the line
        // numbers propagate from the SCG program points.
        if let MemorySafetyViolation::DoubleFree {
            allocation_name,
            first_free_line,
            second_free_line,
        } = double_frees[0]
        {
            assert_eq!(allocation_name, "region_42");
            assert_eq!(*first_free_line, Some(11));
            assert_eq!(*second_free_line, Some(12));
        } else {
            panic!("expected DoubleFree variant");
        }
    }

    /// A single region freed once → no DoubleFree violation.
    #[test]
    fn test_wave20_single_free_no_double_free() {
        let region = vuma_scg::region::RegionId::new(7);
        let scg = build_scg_with_region_frees(region, 1);
        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);

        let config = MemorySafetyConfig {
            check_use_after_free: false,
            check_double_free: true,
            check_uninitialized_reads: false,
            check_memory_leaks: false,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };

        let violations = analyze_with_scg_liveness(&liveness, &scg, &config);
        let double_frees: Vec<_> = violations.iter().filter(|v| v.code() == "E042").collect();
        assert!(
            double_frees.is_empty(),
            "single free should not produce a DoubleFree violation; got {:?}",
            double_frees
        );
    }

    /// `check_double_free: false` suppresses the check entirely,
    /// even when the SCG has an obvious double-free pattern.
    #[test]
    fn test_wave20_double_free_flag_disabled() {
        let region = vuma_scg::region::RegionId::new(99);
        let scg = build_scg_with_region_frees(region, 2);
        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);

        let config = MemorySafetyConfig {
            check_use_after_free: false,
            check_double_free: false, // ← flag OFF
            check_uninitialized_reads: false,
            check_memory_leaks: false,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };

        let violations = analyze_with_scg_liveness(&liveness, &scg, &config);
        assert!(
            violations.iter().all(|v| v.code() != "E042"),
            "check_double_free=false must suppress DoubleFree; got {:?}",
            violations
        );
    }

    /// An intervening allocation clears the region's deallocated
    /// state, so free → alloc → free is NOT a double-free.
    #[test]
    fn test_wave20_intervening_alloc_clears_state() {
        use vuma_scg::{
            AllocationNode, DeallocationNode, NodePayload, NodeType, ProgramPoint, SCG,
        };

        let region = vuma_scg::region::RegionId::new(5);
        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        };

        // alloc R
        let alloc1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 16,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp.clone(),
        );
        // free R  (R now deallocated)
        scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc1,
                region_id: region,
            }),
            pp.clone(),
        );
        // alloc R again  (intervening allocation — clears deallocated state)
        let alloc2 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 16,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp.clone(),
        );
        // free R again  (NOT a double-free — fresh allocation since last free)
        scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc2,
                region_id: region,
            }),
            pp,
        );

        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);
        let config = MemorySafetyConfig {
            check_use_after_free: false,
            check_double_free: true,
            check_uninitialized_reads: false,
            check_memory_leaks: false,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };

        let violations = analyze_with_scg_liveness(&liveness, &scg, &config);
        let double_frees: Vec<_> = violations.iter().filter(|v| v.code() == "E042").collect();
        assert!(
            double_frees.is_empty(),
            "free → alloc → free is NOT a double-free; got {:?}",
            double_frees
        );
    }

    // ─── CCured PointerKind classification tests ──────────────────────────

    #[test]
    fn test_classify_pointer_safe_no_offset() {
        let mut sizes = HashMap::new();
        sizes.insert("buf".to_string(), 64u64);
        // No offset → SAFE regardless of allocation table.
        let kind = classify_pointer(&ScgExpr::Var("buf".to_string()), false, &sizes);
        assert_eq!(kind, PointerKind::Safe);
    }

    #[test]
    fn test_classify_pointer_seq_named_allocation() {
        let mut sizes = HashMap::new();
        sizes.insert("buf".to_string(), 64u64);
        // Named allocation with offset → SEQ.
        let kind = classify_pointer(&ScgExpr::Var("buf".to_string()), true, &sizes);
        assert_eq!(kind, PointerKind::Seq);
    }

    #[test]
    fn test_classify_pointer_wild_unknown_allocation() {
        let sizes = HashMap::new(); // empty
                                    // Offset present but no allocation entry → WILD.
        let kind = classify_pointer(&ScgExpr::Var("ffi_ptr".to_string()), true, &sizes);
        assert_eq!(kind, PointerKind::Wild);
    }

    #[test]
    fn test_classify_pointer_wild_binop_ptr() {
        let mut sizes = HashMap::new();
        sizes.insert("buf".to_string(), 64u64);
        // Pointer arithmetic (`<binop>`) cannot resolve to a name → WILD.
        let binop = ScgExpr::BinOp {
            op: BinOpKind::Add,
            lhs: Box::new(ScgExpr::Var("buf".to_string())),
            rhs: Box::new(ScgExpr::Int(8)),
        };
        let kind = classify_pointer(&binop, true, &sizes);
        assert_eq!(kind, PointerKind::Wild);
    }

    #[test]
    fn test_inject_bounds_check_ir_emits_summary_and_classifies() {
        // Build an SCG with:
        //   1. a SEQ access (named allocation with offset)
        //   2. a SAFE access (no offset)
        //   3. a WILD access (offset, no allocation entry)
        let mut sizes = HashMap::new();
        sizes.insert("arr".to_string(), 16u64);

        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_classify".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    // SEQ: load arr + offset (named, known size)
                    ScgStatement::Access(AccessNode::Load {
                        dst: "v1".to_string(),
                        ptr: ScgExpr::Var("arr".to_string()),
                        offset: Some(ScgExpr::Int(4)),
                        ty: None,
                    }),
                    // SAFE: load arr with no offset
                    ScgStatement::Access(AccessNode::Load {
                        dst: "v2".to_string(),
                        ptr: ScgExpr::Var("arr".to_string()),
                        offset: None,
                        ty: None,
                    }),
                    // WILD: load ffi_ptr + offset (no alloc_sizes entry)
                    ScgStatement::Access(AccessNode::Load {
                        dst: "v3".to_string(),
                        ptr: ScgExpr::Var("ffi_ptr".to_string()),
                        offset: Some(ScgExpr::Int(8)),
                        ty: None,
                    }),
                ],
                var_types: HashMap::new(),
            })],
        };

        let mut scg = scg;
        inject_bounds_check_ir(&mut scg, &sizes);

        // The SEQ access should have gained two prepended statements
        // (Computation + Control::If) before it.
        let func = match &scg.nodes[0] {
            ScgNode::Function(f) => f,
            _ => panic!("expected function node"),
        };
        // Original body had 3 statements; SEQ injection adds 2 → total 5.
        assert_eq!(
            func.body.len(),
            5,
            "SEQ access should be preceded by 2 check stmts; body = {:?}",
            func.body
        );

        // The first two statements should be the bounds-check pair.
        assert!(matches!(
            &func.body[0],
            ScgStatement::Computation(ComputationNode {
                op: BinOpKind::UGe,
                ..
            })
        ));
        assert!(matches!(
            &func.body[1],
            ScgStatement::Control(ControlNode::If { .. })
        ));
        // The original SEQ Load should be at index 2.
        assert!(matches!(
            &func.body[2],
            ScgStatement::Access(AccessNode::Load { .. })
        ));
    }

    // ─── Arena state-size recovery tests ────────────────────────
    //
    // These tests verify that `build_arena_state_sizes` correctly pattern-
    // matches the deterministic IR sequence emitted by `arena_alloc`
    // lowering (pipeline.rs:10411-10513) and that, when the recovered
    // `state_ptr → layout_size` pairs are merged into `alloc_sizes`,
    // `inject_bounds_check_ir` emits `__oob_trap` checks for accesses
    // through the arena-allocated state pointer.

    /// Helper: build a codegen SCG whose `main` body mirrors the exact IR
    /// sequence emitted by `Expr::ArenaAlloc` lowering. Returns the SCG
    /// and the names of the key temp variables (state_ptr, etc.) so tests
    /// can assert against them.
    fn build_arena_alloc_scg(layout_size: i64) -> Scg {
        // The sequence below mirrors pipeline.rs:10412-10512 exactly:
        //   1. off_addr   = arena_ptr + 8
        //   2. offset_val = *[off_addr]            (Load arena.offset)
        //   3. new_offset = offset_val + L         (L = layout_size)
        //   4. cap_addr   = arena_ptr + 16
        //   5. cap_val    = *[cap_addr]            (Load arena.capacity)
        //   6. overflow_cond = new_offset UGt cap_val
        //   7. if overflow_cond { call __arena_overflow() }
        //   8. state_ptr  = arena_ptr + offset_val
        //   9. off_addr2  = arena_ptr + 8
        //  10. *[off_addr2] = new_offset           (Store bumped offset)
        //
        // After the arena_alloc sequence, we emit one subsequent access
        // through `state_ptr` with an offset, which is the access that
        // `inject_bounds_check_ir` should bound with `__oob_trap`.
        let body = vec![
            // 1. off_addr = arena_ptr + 8
            ScgStatement::Computation(ComputationNode {
                dst: "off_addr".to_string(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("arena_ptr".to_string()),
                rhs: ScgExpr::Int(8),
                tail_call: false,
                reassigns: None,
            }),
            // 2. offset_val = *[off_addr]
            ScgStatement::Access(AccessNode::Load {
                dst: "offset_val".to_string(),
                ptr: ScgExpr::Var("off_addr".to_string()),
                offset: None,
                ty: Some(crate::ir::IRType::U64),
            }),
            // 3. new_offset = offset_val + layout_size
            ScgStatement::Computation(ComputationNode {
                dst: "new_offset".to_string(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("offset_val".to_string()),
                rhs: ScgExpr::Int(layout_size),
                tail_call: false,
                reassigns: None,
            }),
            // 4. cap_addr = arena_ptr + 16
            ScgStatement::Computation(ComputationNode {
                dst: "cap_addr".to_string(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("arena_ptr".to_string()),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }),
            // 5. cap_val = *[cap_addr]
            ScgStatement::Access(AccessNode::Load {
                dst: "cap_val".to_string(),
                ptr: ScgExpr::Var("cap_addr".to_string()),
                offset: None,
                ty: Some(crate::ir::IRType::U64),
            }),
            // 6. overflow_cond = new_offset UGt cap_val
            ScgStatement::Computation(ComputationNode {
                dst: "overflow_cond".to_string(),
                op: BinOpKind::UGt,
                lhs: ScgExpr::Var("new_offset".to_string()),
                rhs: ScgExpr::Var("cap_val".to_string()),
                tail_call: false,
                reassigns: None,
            }),
            // 7. if overflow_cond { call __arena_overflow() }
            ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Var("overflow_cond".to_string()),
                then_body: vec![ScgStatement::Call(CallNode {
                    dst: None,
                    func: "__arena_overflow".to_string(),
                    args: vec![],
                    is_extern: true,
                    reassigns: None,
                })],
                else_body: None,
            }),
            // 8. state_ptr = arena_ptr + offset_val
            ScgStatement::Computation(ComputationNode {
                dst: "state_ptr".to_string(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("arena_ptr".to_string()),
                rhs: ScgExpr::Var("offset_val".to_string()),
                tail_call: false,
                reassigns: None,
            }),
            // 9. off_addr2 = arena_ptr + 8
            ScgStatement::Computation(ComputationNode {
                dst: "off_addr2".to_string(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var("arena_ptr".to_string()),
                rhs: ScgExpr::Int(8),
                tail_call: false,
                reassigns: None,
            }),
            // 10. *[off_addr2] = new_offset
            ScgStatement::Access(AccessNode::Store {
                ptr: ScgExpr::Var("off_addr2".to_string()),
                offset: None,
                value: ScgExpr::Var("new_offset".to_string()),
                ty: Some(crate::ir::IRType::U64),
            }),
            // 11. Subsequent access through state_ptr with an offset — this
            //     is the access that should be bounded by __oob_trap once
            //     the state_ptr → layout_size mapping is in alloc_sizes.
            ScgStatement::Access(AccessNode::Load {
                dst: "field_val".to_string(),
                ptr: ScgExpr::Var("state_ptr".to_string()),
                offset: Some(ScgExpr::Int(8)),
                ty: None,
            }),
        ];
        Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_arena_alloc".to_string(),
                params: vec![],
                results: vec![],
                body,
                var_types: HashMap::new(),
            })],
        }
    }

    #[test]
    fn test_build_arena_state_sizes_recovers_layout_size() {
        // The arena_alloc IR pattern with layout_size = 64 should produce
        // a single `state_ptr → 64` entry.
        let scg = build_arena_alloc_scg(64);
        let table = build_arena_state_sizes(&scg);
        assert_eq!(
            table.len(),
            1,
            "expected exactly 1 arena state_ptr entry, got {:?}",
            table
        );
        assert_eq!(table.get("state_ptr"), Some(&64u64));
    }

    #[test]
    fn test_build_arena_state_sizes_empty_without_arena_overflow() {
        // An SCG without `__arena_overflow` should produce an empty table.
        // This guards against false positives from unrelated `Add(Var, Var)`
        // computations.
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "no_arena".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    // Mimics the `state_ptr = arena_ptr + offset_val`
                    // computation but WITHOUT the preceding overflow check
                    // anchor — `build_arena_state_sizes` must NOT register
                    // this as an arena state pointer.
                    ScgStatement::Computation(ComputationNode {
                        dst: "state_ptr".to_string(),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Var("arena_ptr".to_string()),
                        rhs: ScgExpr::Var("offset_val".to_string()),
                        tail_call: false,
                        reassigns: None,
                    }),
                    ScgStatement::Access(AccessNode::Load {
                        dst: "v".to_string(),
                        ptr: ScgExpr::Var("state_ptr".to_string()),
                        offset: Some(ScgExpr::Int(8)),
                        ty: None,
                    }),
                ],
                var_types: HashMap::new(),
            })],
        };
        let table = build_arena_state_sizes(&scg);
        assert!(
            table.is_empty(),
            "expected empty table (no __arena_overflow anchor), got {:?}",
            table
        );
    }

    #[test]
    fn test_build_arena_state_sizes_skips_zero_layout() {
        // A zero layout_size is degenerate (no meaningful bound). The
        // recovery function should skip such entries to avoid emitting
        // `0 UGe 0` checks (which would always trap).
        let scg = build_arena_alloc_scg(0);
        let table = build_arena_state_sizes(&scg);
        assert!(
            table.is_empty(),
            "expected empty table for zero layout_size, got {:?}",
            table
        );
    }

    #[test]
    fn test_inject_bounds_check_ir_emits_oob_trap_for_arena_state_ptr() {
        // End-to-end: when the `state_ptr → layout_size` mapping recovered
        // by `build_arena_state_sizes` is merged into `alloc_sizes`,
        // `inject_bounds_check_ir` must emit an `__oob_trap` check before
        // the subsequent access through `state_ptr`.
        let mut scg = build_arena_alloc_scg(64);

        // Step 1: recover arena state sizes (mirrors what pipeline.rs does).
        let arena_state_sizes = build_arena_state_sizes(&scg);
        assert_eq!(arena_state_sizes.get("state_ptr"), Some(&64u64));

        // Step 2: merge into alloc_sizes (mirrors pipeline.rs).
        let mut alloc_sizes: HashMap<String, u64> = HashMap::new();
        for (k, v) in &arena_state_sizes {
            alloc_sizes.insert(k.clone(), *v);
        }

        // Step 3: inject bounds checks.
        inject_bounds_check_ir(&mut scg, &alloc_sizes);

        let func = match &scg.nodes[0] {
            ScgNode::Function(f) => f,
            _ => panic!("expected Function node"),
        };

        // The original body had 11 statements. The single SEQ access
        // (statement 11, the `field_val` load through `state_ptr`) should
        // gain 2 prepended check statements → total 13.
        assert_eq!(
            func.body.len(),
            13,
            "expected 13 statements (11 original + 2 check pair), got {}: {:?}",
            func.body.len(),
            func.body
                .iter()
                .map(|s| format!("{:?}", s))
                .collect::<Vec<_>>()
        );

        // Find the __oob_trap If in the body and verify it precedes the
        // `field_val` Load (the access through `state_ptr`).
        let mut oob_trap_idx: Option<usize> = None;
        let mut field_load_idx: Option<usize> = None;
        for (i, stmt) in func.body.iter().enumerate() {
            if let ScgStatement::Control(ControlNode::If { then_body, .. }) = stmt {
                if then_body.len() == 1
                    && matches!(
                        &then_body[0],
                        ScgStatement::Call(c) if c.func == "__oob_trap" && c.is_extern
                    )
                {
                    oob_trap_idx = Some(i);
                }
            }
            if let ScgStatement::Access(AccessNode::Load { dst, .. }) = stmt {
                if dst == "field_val" {
                    field_load_idx = Some(i);
                }
            }
        }
        let oob_trap_idx = oob_trap_idx.expect("expected an __oob_trap If in the body");
        let field_load_idx = field_load_idx.expect("expected the field_val Load in the body");
        // The __oob_trap If should be immediately before the field_val Load.
        assert_eq!(
            field_load_idx,
            oob_trap_idx + 1,
            "field_val Load should immediately follow the __oob_trap If"
        );
        // And the UGe computation should immediately precede the If.
        assert!(matches!(
            &func.body[oob_trap_idx - 1],
            ScgStatement::Computation(ComputationNode {
                op: BinOpKind::UGe,
                ..
            })
        ));
    }

    #[test]
    fn test_inject_bounds_check_ir_skips_arena_state_ptr_without_mapping() {
        // Negative control: when `alloc_sizes` does NOT contain the
        // `state_ptr → layout_size` mapping (i.e. `build_arena_state_sizes`
        // was not called or returned empty), `inject_bounds_check_ir` must
        // NOT emit an `__oob_trap` check for the access through
        // `state_ptr` — it classifies as `PointerKind::Wild` and is skipped
        // (legacy behaviour, retained as the fallback path).
        let mut scg = build_arena_alloc_scg(64);
        let empty_alloc_sizes: HashMap<String, u64> = HashMap::new();
        inject_bounds_check_ir(&mut scg, &empty_alloc_sizes);
        let func = match &scg.nodes[0] {
            ScgNode::Function(f) => f,
            _ => panic!("expected Function node"),
        };
        // No __oob_trap should be present (the only `__arena_overflow` If
        // is the arena_alloc overflow check, not a per-access OOB trap).
        let has_oob_trap = func.body.iter().any(|s| {
            if let ScgStatement::Control(ControlNode::If { then_body, .. }) = s {
                then_body.len() == 1
                    && matches!(
                        &then_body[0],
                        ScgStatement::Call(c) if c.func == "__oob_trap" && c.is_extern
                    )
            } else {
                false
            }
        });
        assert!(
            !has_oob_trap,
            "expected NO __oob_trap injection without the arena state-size mapping"
        );
    }

    // ── Negative-path test ───────────────────────────────────────────
    //
    // Memory-safety negative path with `--safe` (always on per PMT):
    // an out-of-bounds WRITE (Store with offset) on a known-named
    // allocation must trigger `inject_bounds_check_ir` to insert the
    // `Computation(UGe) + Control(If { __oob_trap })` pair BEFORE the
    // Store.  The existing `test_inject_bounds_check_ir_inserts_oob_trap`
    // covers the Load (read) path; this test covers the Store (write)
    // path so a regression that bounds-checked only reads would be
    // caught.  `inject_bounds_check_ir` does not panic — it mutates
    // the SCG in place — so per the task brief this test uses
    // structural assertions on the mutated body rather than
    // `#[should_panic]`.

    /// An OOB Store (write with offset) on a known-named allocation
    /// must trigger `__oob_trap` injection — the `--safe` runtime
    /// trap contract.  Body should grow from
    /// `[Allocation, Access(Store)]` (2 stmts) to
    /// `[Allocation, Computation(UGe), Control(If), Access(Store)]`
    /// (4 stmts), and the `If`'s `then_body` must contain exactly
    /// one `Call(__oob_trap, is_extern=true)`.
    #[test]
    fn test_negative_oob_store_triggers_oob_trap_injection() {
        let mut scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_oob_store".to_string(),
                params: vec![],
                results: vec![],
                body: vec![
                    ScgStatement::Allocation(AllocationNode::Stack {
                        name: "buf".to_string(),
                        size: 32,
                        ty: ScgType::U8,
                    }),
                    // Store with a runtime-variable offset on a known
                    // allocation → classified as `PointerKind::Seq` →
                    // bounds check + __oob_trap pair inserted before.
                    ScgStatement::Access(AccessNode::Store {
                        ptr: ScgExpr::Var("buf".to_string()),
                        offset: Some(ScgExpr::Var("i".to_string())),
                        value: ScgExpr::Int(99),
                        ty: None,
                    }),
                ],
                var_types: std::collections::HashMap::new(),
            })],
        };

        let mut table: HashMap<String, u64> = HashMap::new();
        table.insert("buf".to_string(), 32);
        inject_bounds_check_ir(&mut scg, &table);

        let func = match &scg.nodes[0] {
            ScgNode::Function(f) => f,
            _ => panic!("expected Function node"),
        };
        // Body should now be:
        // [Allocation, Computation(UGe), Control(If), Access(Store)]
        assert_eq!(
            func.body.len(),
            4,
            "OOB Store must trigger insertion of 2 stmts (UGe + If) before the Store"
        );
        assert!(
            matches!(&func.body[1], ScgStatement::Computation(c) if c.op == BinOpKind::UGe),
            "stmt[1] must be the UGe bounds-check computation"
        );
        match &func.body[2] {
            ScgStatement::Control(ControlNode::If { then_body, .. }) => {
                assert_eq!(
                    then_body.len(),
                    1,
                    "If then_body must contain exactly one statement"
                );
                assert!(
                    matches!(
                        &then_body[0],
                        ScgStatement::Call(c) if c.func == "__oob_trap" && c.is_extern
                    ),
                    "If then_body must be a Call to __oob_trap (is_extern=true)"
                );
            }
            other => panic!(
                "stmt[2] must be ControlNode::If with __oob_trap; got {:?}",
                other
            ),
        }
        assert!(
            matches!(&func.body[3], ScgStatement::Access(_)),
            "stmt[3] must be the original Access(Store)"
        );
    }
}
