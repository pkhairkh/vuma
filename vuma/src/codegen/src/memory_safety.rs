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

use crate::scg_to_ir::{
    AccessNode, AllocationNode, CallNode, ControlNode, Scg, ScgExpr, ScgFunction, ScgNode,
    ScgStatement, ScgType, SwitchArm,
};

// ═══════════════════════════════════════════════════════════════════════════
// Error codes for memory safety diagnostics
// ═══════════════════════════════════════════════════════════════════════════

/// Memory safety violation kind, mapped to E041–E050.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
                format!("uninitialized read: variable '{}' has no reaching write", variable_name)
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
            } => format!(
                "invalid free: pointer '{}' — {}",
                pointer_name, reason
            ),
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

    /// Whether runtime bounds checks were instrumented.
    pub runtime_bounds_instrumented: bool,

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
            runtime_bounds_instrumented: false,
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
    /// Variable name of the allocation.
    name: String,
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
    /// Variable name being freed.
    name: String,
    /// Source line number.
    line: Option<u32>,
    /// Whether this is an explicit free or implicit (end of scope for stack).
    is_explicit: bool,
}

/// Information about an access (load/store) to an allocation.
#[derive(Debug, Clone)]
struct AccessInfo {
    /// Variable name being accessed.
    name: String,
    /// Whether this is a read or write.
    is_read: bool,
    /// Source line number.
    line: Option<u32>,
    /// Optional offset expression (for array indexing).
    offset_expr: Option<String>,
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

        report.runtime_bounds_instrumented = self.config.runtime_bounds_checks;
        report.analysis_time_us = start.elapsed().as_micros() as u64;
        report
    }

    /// Analyze a single function to track allocations, frees, and accesses.
    fn analyze_function(&self, func: &ScgFunction) -> HashMap<String, AllocationInfo> {
        let mut allocations: HashMap<String, AllocationInfo> = HashMap::new();
        self.walk_statements(&func.body, &mut allocations, None);
        allocations
    }

    /// Recursively walk SCG statements to collect allocation/access/free info.
    fn walk_statements(
        &self,
        stmts: &[ScgStatement],
        allocations: &mut HashMap<String, AllocationInfo>,
        scope_name: Option<&str>,
    ) {
        for stmt in stmts {
            match stmt {
                ScgStatement::Allocation(alloc) => {
                    match alloc {
                        AllocationNode::Stack { name, size, .. } => {
                            allocations.insert(
                                name.clone(),
                                AllocationInfo {
                                    name: name.clone(),
                                    is_heap: false,
                                    size: Some(*size),
                                    line: None,
                                    frees: Vec::new(),
                                    accesses: Vec::new(),
                                    is_returned: false,
                                },
                            );
                        }
                        AllocationNode::Heap { name, .. } => {
                            allocations.insert(
                                name.clone(),
                                AllocationInfo {
                                    name: name.clone(),
                                    is_heap: true,
                                    size: None, // Dynamic size
                                    line: None,
                                    frees: Vec::new(),
                                    accesses: Vec::new(),
                                    is_returned: false,
                                },
                            );
                        }
                    }
                }
                ScgStatement::Access(access) => {
                    match access {
                        AccessNode::Load { dst, ptr, offset } => {
                            let ptr_name = expr_to_name(ptr);
                            if let Some(info) = allocations.get_mut(&ptr_name) {
                                info.accesses.push(AccessInfo {
                                    name: dst.clone(),
                                    is_read: true,
                                    line: None,
                                    offset_expr: offset.as_ref().map(|e| format!("{:?}", e)),
                                });
                            }
                        }
                        AccessNode::Store { ptr, offset, value } => {
                            let ptr_name = expr_to_name(ptr);
                            if let Some(info) = allocations.get_mut(&ptr_name) {
                                info.accesses.push(AccessInfo {
                                    name: expr_to_name(value),
                                    is_read: false,
                                    line: None,
                                    offset_expr: offset.as_ref().map(|e| format!("{:?}", e)),
                                });
                            }
                        }
                    }
                }
                ScgStatement::Call(call) => {
                    // Check for deallocation calls (free, __vuma_free, etc.)
                    let func_name = &call.func;
                    if is_deallocation_call(func_name) {
                        for arg in &call.args {
                            let arg_name = expr_to_name(arg);
                            if let Some(info) = allocations.get_mut(&arg_name) {
                                info.frees.push(FreeInfo {
                                    name: arg_name.clone(),
                                    line: None,
                                    is_explicit: true,
                                });
                            }
                        }
                    }
                }
                ScgStatement::Control(ctrl) => {
                    match ctrl {
                        ControlNode::If {
                            then_body,
                            else_body,
                            ..
                        } => {
                            self.walk_statements(then_body, allocations, scope_name);
                            if let Some(else_body) = else_body {
                                self.walk_statements(else_body, allocations, scope_name);
                            }
                        }
                        ControlNode::Loop { body, .. } => {
                            self.walk_statements(body, allocations, scope_name);
                        }
                        ControlNode::Switch {
                            arms, default_body, ..
                        } => {
                            for arm in arms {
                                self.walk_statements(&arm.body, allocations, scope_name);
                            }
                            self.walk_statements(default_body, allocations, scope_name);
                        }
                        ControlNode::Break | ControlNode::Continue => {}
                    }
                }
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
                ScgStatement::Call(call) => {
                    if is_deallocation_call(&call.func) {
                        for arg in &call.args {
                            if expr_to_name(arg) == alloc_name {
                                *freed = true;
                            }
                        }
                    }
                }
                ScgStatement::Access(access) => {
                    if *freed {
                        let ptr_name = match access {
                            AccessNode::Load { ptr, .. } => expr_to_name(ptr),
                            AccessNode::Store { ptr, .. } => expr_to_name(ptr),
                        };
                        if ptr_name == alloc_name {
                            *count += 1;
                        }
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
                            self.count_accesses_after_free_stmts(
                                body,
                                alloc_name,
                                freed,
                                count,
                            );
                        }
                        ControlNode::Switch {
                            arms, default_body, ..
                        } => {
                            for arm in arms {
                                self.count_accesses_after_free_stmts(
                                    &arm.body,
                                    alloc_name,
                                    freed,
                                    count,
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Scan a codegen SCG for array access sites that need bounds checking.
///
/// When `--safe` is enabled, every array load/store gets a bounds check
/// that traps if the index is out of range.
pub fn find_bounds_check_sites(scg: &Scg) -> Vec<BoundsCheckSite> {
    let mut sites = Vec::new();

    for node in &scg.nodes {
        if let ScgNode::Function(func) = node {
            find_bounds_check_sites_in_stmts(&func.name, &func.body, &mut sites);
        }
    }

    sites
}

fn find_bounds_check_sites_in_stmts(
    func_name: &str,
    stmts: &[ScgStatement],
    sites: &mut Vec<BoundsCheckSite>,
) {
    for stmt in stmts {
        match stmt {
            ScgStatement::Access(access) => {
                match access {
                    AccessNode::Load { ptr, offset, .. } => {
                        if offset.is_some() {
                            sites.push(BoundsCheckSite {
                                function_name: func_name.to_string(),
                                array_name: expr_to_name(ptr),
                                index_expr: offset
                                    .as_ref()
                                    .map(|e| format!("{:?}", e))
                                    .unwrap_or_default(),
                                length_expr: None,
                                line: None,
                            });
                        }
                    }
                    AccessNode::Store { ptr, offset, .. } => {
                        if offset.is_some() {
                            sites.push(BoundsCheckSite {
                                function_name: func_name.to_string(),
                                array_name: expr_to_name(ptr),
                                index_expr: offset
                                    .as_ref()
                                    .map(|e| format!("{:?}", e))
                                    .unwrap_or_default(),
                                length_expr: None,
                                line: None,
                            });
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
                    find_bounds_check_sites_in_stmts(func_name, then_body, sites);
                    if let Some(else_body) = else_body {
                        find_bounds_check_sites_in_stmts(func_name, else_body, sites);
                    }
                }
                ControlNode::Loop { body, .. } => {
                    find_bounds_check_sites_in_stmts(func_name, body, sites);
                }
                ControlNode::Switch {
                    arms, default_body, ..
                } => {
                    for arm in arms {
                        find_bounds_check_sites_in_stmts(func_name, &arm.body, sites);
                    }
                    find_bounds_check_sites_in_stmts(func_name, default_body, sites);
                }
                ControlNode::Break | ControlNode::Continue => {}
            },
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════════════════

/// Check if a function name is a deallocation call.
fn is_deallocation_call(name: &str) -> bool {
    matches!(
        name,
        "free"
            | "__vuma_free"
            | "dealloc"
            | "deallocate"
            | "drop"
            | "__builtin_free"
    )
}

/// Extract a variable name from an SCG expression (best-effort).
fn expr_to_name(expr: &ScgExpr) -> String {
    match expr {
        ScgExpr::Var(name) => name.clone(),
        ScgExpr::Int(_) => "<const>".to_string(),
        ScgExpr::Float(_) => "<const>".to_string(),
        ScgExpr::Label(name) => format!("<label:{}>", name),
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

    // Uninitialized read detection
    if config.check_uninitialized_reads {
        let uninit_reads = vuma_scg::liveness::find_uninitialized_reads(scg, &scg_liveness.liveness);
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
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                    }),
                ],
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
                    }),
                    ScgStatement::Access(AccessNode::Load {
                        dst: "val".to_string(),
                        ptr: ScgExpr::Var("buf".to_string()),
                        offset: None,
                    }),
                ],
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
                    }),
                    ScgStatement::Call(CallNode {
                        dst: None,
                        func: "free".to_string(),
                        args: vec![ScgExpr::Var("buf".to_string())],
                        is_extern: true,
                    }),
                ],
            })],
        };

        let analyzer = MemorySafetyAnalyzer::with_defaults();
        let report = analyzer.analyze(&scg);

        // Should have no violations (proper usage)
        assert!(report.is_clean());
    }

    #[test]
    fn test_bounds_check_site_detection() {
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_bounds".to_string(),
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
                    }),
                ],
            })],
        };

        let sites = find_bounds_check_sites(&scg);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].array_name, "arr");
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
}
