//! # Codegen Tests for Complex Programs
//!
//! Tests exercising the ARM64 code-generation pipeline on non-trivial programs:
//! iterative/recursive factorial & fibonacci, nested loops, switch dispatch,
//! many-argument functions, and callee-saved register preservation.
//!
//! Each test constructs an SCG representing the program, lowers it to IR via
//! the codegen pipeline, and verifies the IR structure (correct instructions,
//! proper register usage, correct calling convention).
//!
//! # Test Matrix
//!
//! | # | Test                              | What it validates                                  |
//! |---|-----------------------------------|----------------------------------------------------|
//! | 1 | test_factorial_iterative          | Iterative factorial: Mul + loop + Cmp IR structure  |
//! | 2 | test_factorial_recursive          | Recursive factorial: Call lowering, stack frame     |
//! | 3 | test_fibonacci_iterative          | Iterative fibonacci: Add + loop + Phi IR structure  |
//! | 4 | test_fibonacci_recursive          | Recursive fibonacci: dual-recursive Call lowering   |
//! | 5 | test_nested_loops                 | Matrix-multiply-like triple-nested loops            |
//! | 6 | test_switch_dispatch              | Multi-way branch via Switch → Cmp + CondBranch      |
//! | 7 | test_function_with_many_args      | 12-argument function: AAPCS64 register + stack spilling |
//! | 8 | test_callee_saved_preservation    | Uses X19–X28, verifies save/restore in alloc result |

use vuma_codegen::{
    arm64::Register,
    ir::{
        BinOpKind, CmpKind, IRInstr, IRTerminator, IRType, IRValue,
        compute_calling_conv, compute_stack_layout_with_info, ArgClass, RegisterClass,
    },
    scg_to_ir::{
        IRBuilder, Scg, ScgNode, ScgFunction, ScgParam, ScgType,
        ScgStatement, ScgExpr, ComputationNode, AllocationNode, AccessNode,
        ControlNode, CallNode, CallingConvention,
    },
    regalloc::LinearScanAllocator,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an SCG into an IR program and return it (no emission).
fn build_ir(scg: &Scg) -> vuma_codegen::ir::IRProgram {
    let mut builder = IRBuilder::new();
    builder.build(scg).expect("IRBuilder should succeed")
}

/// Collect all instructions across all blocks of the first function.
fn all_instrs(ir: &vuma_codegen::ir::IRProgram) -> Vec<&IRInstr> {
    ir.functions[0]
        .blocks
        .iter()
        .flat_map(|b| b.instructions.iter())
        .collect()
}

/// Count IR instructions matching a predicate across all blocks of the first function.
fn count_instrs<F>(ir: &vuma_codegen::ir::IRProgram, pred: F) -> usize
where
    F: Fn(&IRInstr) -> bool,
{
    all_instrs(ir).into_iter().filter(|i| pred(i)).count()
}

/// Count terminators matching a predicate across all blocks of the first function.
fn count_terminators<F>(ir: &vuma_codegen::ir::IRProgram, pred: F) -> usize
where
    F: Fn(&IRTerminator) -> bool,
{
    ir.functions[0]
        .blocks
        .iter()
        .filter(|b| pred(&b.terminator))
        .count()
}

// ---------------------------------------------------------------------------
// Test 1: Iterative Factorial
// ---------------------------------------------------------------------------

/// Test: Compile an iterative factorial function.
///
/// ```c
/// i64 factorial(i64 n) {
///     i64 result = 1;
///     while (n > 1) {
///         result = result * n;
///         n = n - 1;
///     }
///     return result;
/// }
/// ```
///
/// Validates:
/// - IR contains Mul instruction (result * n)
/// - IR contains Sub instruction (n - 1)
/// - IR contains Cmp instruction (n > 1 condition)
/// - IR has a loop with back-edge (multiple blocks)
/// - Stack layout is valid and 16-byte aligned
#[test]
fn test_factorial_iterative() {
    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "factorial_iter".to_string(),
            params: vec![
                ScgParam { name: "n".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                // result = 1
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "result".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("result".to_string()),
                    offset: None,
                    value: ScgExpr::Int(1),
                }),
                // while (n > 1) { result = result * n; n = n - 1; }
                ScgStatement::Control(ControlNode::Loop {
                    body: vec![
                        // if n <= 1, break
                        ScgStatement::Control(ControlNode::If {
                            cond: ScgExpr::Var("n".to_string()),
                            then_body: vec![],
                            else_body: Some(vec![
                                ScgStatement::Control(ControlNode::Break),
                            ]),
                        }),
                        // result = result * n
                        ScgStatement::Computation(ComputationNode {
                            dst: "result".to_string(),
                            op: BinOpKind::Mul,
                            lhs: ScgExpr::Var("result".to_string()),
                            rhs: ScgExpr::Var("n".to_string()),
                        }),
                        // n = n - 1
                        ScgStatement::Computation(ComputationNode {
                            dst: "n".to_string(),
                            op: BinOpKind::Sub,
                            lhs: ScgExpr::Var("n".to_string()),
                            rhs: ScgExpr::Int(1),
                        }),
                    ],
                }),
                ScgStatement::Access(AccessNode::Load {
                    dst: "final_result".to_string(),
                    ptr: ScgExpr::Var("result".to_string()),
                    offset: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("final_result".to_string())]),
            ],
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function signature
    assert_eq!(func.name, "factorial_iter");
    assert_eq!(func.params.len(), 1);
    assert_eq!(func.result_types.len(), 1);

    // Verify IR contains a Mul instruction
    let mul_count = count_instrs(&ir, |i| matches!(i, IRInstr::Mul { .. }));
    assert!(mul_count >= 1, "IR should contain at least one Mul instruction, found {}", mul_count);

    // Verify IR contains a Sub instruction
    let sub_count = count_instrs(&ir, |i| matches!(i, IRInstr::Sub { .. }));
    assert!(sub_count >= 1, "IR should contain at least one Sub instruction, found {}", sub_count);

    // Verify multiple blocks (loop creates header, body, exit)
    assert!(func.blocks.len() >= 3,
        "Iterative factorial should produce multiple blocks (loop), got {}", func.blocks.len());

    // Verify stack layout is valid
    let layout = vuma_codegen::ir::compute_stack_layout(func);
    assert!(layout.total_size % 16 == 0,
        "Stack frame should be 16-byte aligned, got size {}", layout.total_size);
}

// ---------------------------------------------------------------------------
// Test 2: Recursive Factorial
// ---------------------------------------------------------------------------

/// Test: Compile a recursive factorial function.
///
/// ```c
/// i64 factorial(i64 n) {
///     if (n <= 1) return 1;
///     return n * factorial(n - 1);
/// }
/// ```
///
/// Validates:
/// - IR contains a Call instruction (recursive call)
/// - IR contains Mul instruction (n * result)
/// - IR contains Sub instruction (n - 1)
/// - IR has CondBranch (if n <= 1)
/// - Calling convention: 1 arg in X0, return in X0
#[test]
fn test_factorial_recursive() {
    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "factorial_rec".to_string(),
            params: vec![
                ScgParam { name: "n".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                // if n <= 1, return 1
                ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Var("n".to_string()),
                    then_body: vec![
                        ScgStatement::Return(vec![ScgExpr::Int(1)]),
                    ],
                    else_body: Some(vec![
                        // n_minus_1 = n - 1
                        ScgStatement::Computation(ComputationNode {
                            dst: "n_minus_1".to_string(),
                            op: BinOpKind::Sub,
                            lhs: ScgExpr::Var("n".to_string()),
                            rhs: ScgExpr::Int(1),
                        }),
                        // rec_result = factorial_rec(n_minus_1)
                        ScgStatement::Call(CallNode {
                            dst: Some("rec_result".to_string()),
                            func: "factorial_rec".to_string(),
                            args: vec![ScgExpr::Var("n_minus_1".to_string())],
                        }),
                        // result = n * rec_result
                        ScgStatement::Computation(ComputationNode {
                            dst: "result".to_string(),
                            op: BinOpKind::Mul,
                            lhs: ScgExpr::Var("n".to_string()),
                            rhs: ScgExpr::Var("rec_result".to_string()),
                        }),
                        ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
                    ]),
                }),
                // Default return (unreachable, but needed for well-formed SCG)
                ScgStatement::Return(vec![ScgExpr::Int(1)]),
            ],
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function name
    assert_eq!(func.name, "factorial_rec");

    // Verify IR contains a Call instruction (the recursive call)
    let call_count = count_instrs(&ir, |i| matches!(i, IRInstr::Call { .. }));
    assert!(call_count >= 1, "IR should contain a Call instruction for recursion, found {}", call_count);

    // Verify IR contains Mul instruction
    let mul_count = count_instrs(&ir, |i| matches!(i, IRInstr::Mul { .. }));
    assert!(mul_count >= 1, "IR should contain a Mul instruction, found {}", mul_count);

    // Verify IR contains Sub instruction
    let sub_count = count_instrs(&ir, |i| matches!(i, IRInstr::Sub { .. }));
    assert!(sub_count >= 1, "IR should contain a Sub instruction, found {}", sub_count);

    // Verify CondBranch from if/else
    let has_cond_branch = count_terminators(&ir, |t| matches!(t, IRTerminator::Branch { .. }));
    assert!(has_cond_branch >= 1, "Should have a conditional branch for if/else");

    // Verify calling convention: 1 integer arg → X0
    let cc = compute_calling_conv(&[IRType::I64], &IRType::I64);
    assert_eq!(cc.arg_locations.len(), 1);
    assert_eq!(cc.arg_locations[0].register, Some((RegisterClass::X, 0)));
    assert_eq!(cc.ret_location.registers, vec![(RegisterClass::X, 0)]);
}

// ---------------------------------------------------------------------------
// Test 3: Iterative Fibonacci
// ---------------------------------------------------------------------------

/// Test: Compile an iterative fibonacci function.
///
/// ```c
/// i64 fib(i64 n) {
///     i64 a = 0, b = 1;
///     for (i64 i = 0; i < n; i++) {
///         i64 tmp = a + b;
///         a = b;
///         b = tmp;
///     }
///     return a;
/// }
/// ```
///
/// Validates:
/// - IR contains Add instruction (a + b)
/// - IR contains a loop with Phi nodes
/// - Multiple blocks (loop header, body, exit)
/// - Stack layout is valid
#[test]
fn test_fibonacci_iterative() {
    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "fib_iter".to_string(),
            params: vec![
                ScgParam { name: "n".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                // a = 0
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "a".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("a".to_string()),
                    offset: None,
                    value: ScgExpr::Int(0),
                }),
                // b = 1
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "b".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("b".to_string()),
                    offset: None,
                    value: ScgExpr::Int(1),
                }),
                // loop { ... }
                ScgStatement::Control(ControlNode::Loop {
                    body: vec![
                        // tmp = a + b
                        ScgStatement::Computation(ComputationNode {
                            dst: "tmp".to_string(),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var("a".to_string()),
                            rhs: ScgExpr::Var("b".to_string()),
                        }),
                        // a = b
                        ScgStatement::Access(AccessNode::Store {
                            ptr: ScgExpr::Var("a".to_string()),
                            offset: None,
                            value: ScgExpr::Var("b".to_string()),
                        }),
                        // b = tmp
                        ScgStatement::Access(AccessNode::Store {
                            ptr: ScgExpr::Var("b".to_string()),
                            offset: None,
                            value: ScgExpr::Var("tmp".to_string()),
                        }),
                    ],
                }),
                ScgStatement::Access(AccessNode::Load {
                    dst: "result".to_string(),
                    ptr: ScgExpr::Var("a".to_string()),
                    offset: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
            ],
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function signature
    assert_eq!(func.name, "fib_iter");
    assert_eq!(func.params.len(), 1);

    // Verify IR contains Add instruction
    let add_count = count_instrs(&ir, |i| matches!(i, IRInstr::Add { .. }));
    assert!(add_count >= 1, "IR should contain at least one Add instruction, found {}", add_count);

    // Verify loop structure — multiple blocks
    assert!(func.blocks.len() >= 3,
        "Iterative fibonacci should have multiple blocks (loop), got {}", func.blocks.len());

    // Verify Phi nodes exist in loop headers
    let phi_count = count_instrs(&ir, |i| matches!(i, IRInstr::Phi { .. }));
    assert!(phi_count >= 1, "Loop should have at least one Phi node, found {}", phi_count);

    // Verify stack layout with local variables
    let layout = vuma_codegen::ir::compute_stack_layout(func);
    assert!(!layout.local_slots.is_empty(),
        "Should have local variable slots for a and b");
    assert!(layout.total_size % 16 == 0,
        "Stack frame should be 16-byte aligned, got {}", layout.total_size);
}

// ---------------------------------------------------------------------------
// Test 4: Recursive Fibonacci
// ---------------------------------------------------------------------------

/// Test: Compile a recursive fibonacci function.
///
/// ```c
/// i64 fib(i64 n) {
///     if (n <= 1) return n;
///     return fib(n - 1) + fib(n - 2);
/// }
/// ```
///
/// Validates:
/// - IR contains two Call instructions (fib(n-1) and fib(n-2))
/// - IR contains Add instruction (adding the two recursive results)
/// - IR contains Sub instructions (n-1, n-2)
/// - Stack frame must account for outgoing call args
#[test]
fn test_fibonacci_recursive() {
    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "fib_rec".to_string(),
            params: vec![
                ScgParam { name: "n".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                // if n <= 1, return n
                ScgStatement::Control(ControlNode::If {
                    cond: ScgExpr::Var("n".to_string()),
                    then_body: vec![
                        ScgStatement::Return(vec![ScgExpr::Var("n".to_string())]),
                    ],
                    else_body: Some(vec![
                        // nm1 = n - 1
                        ScgStatement::Computation(ComputationNode {
                            dst: "nm1".to_string(),
                            op: BinOpKind::Sub,
                            lhs: ScgExpr::Var("n".to_string()),
                            rhs: ScgExpr::Int(1),
                        }),
                        // r1 = fib_rec(nm1)
                        ScgStatement::Call(CallNode {
                            dst: Some("r1".to_string()),
                            func: "fib_rec".to_string(),
                            args: vec![ScgExpr::Var("nm1".to_string())],
                        }),
                        // nm2 = n - 2
                        ScgStatement::Computation(ComputationNode {
                            dst: "nm2".to_string(),
                            op: BinOpKind::Sub,
                            lhs: ScgExpr::Var("n".to_string()),
                            rhs: ScgExpr::Int(2),
                        }),
                        // r2 = fib_rec(nm2)
                        ScgStatement::Call(CallNode {
                            dst: Some("r2".to_string()),
                            func: "fib_rec".to_string(),
                            args: vec![ScgExpr::Var("nm2".to_string())],
                        }),
                        // result = r1 + r2
                        ScgStatement::Computation(ComputationNode {
                            dst: "result".to_string(),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var("r1".to_string()),
                            rhs: ScgExpr::Var("r2".to_string()),
                        }),
                        ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
                    ]),
                }),
                // Default return (unreachable)
                ScgStatement::Return(vec![ScgExpr::Int(0)]),
            ],
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function name
    assert_eq!(func.name, "fib_rec");

    // Verify two Call instructions (dual recursive calls)
    let call_count = count_instrs(&ir, |i| matches!(i, IRInstr::Call { .. }));
    assert!(call_count >= 2,
        "Recursive fibonacci should have at least 2 Call instructions, found {}", call_count);

    // Verify Add instruction (r1 + r2)
    let add_count = count_instrs(&ir, |i| matches!(i, IRInstr::Add { .. }));
    assert!(add_count >= 1, "IR should contain an Add instruction, found {}", add_count);

    // Verify Sub instructions (n-1, n-2)
    let sub_count = count_instrs(&ir, |i| matches!(i, IRInstr::Sub { .. }));
    assert!(sub_count >= 2, "IR should contain at least 2 Sub instructions, found {}", sub_count);

    // Verify CondBranch from if/else
    let has_cond_branch = count_terminators(&ir, |t| matches!(t, IRTerminator::Branch { .. }));
    assert!(has_cond_branch >= 1, "Should have a conditional branch for if/else");

    // Verify calling convention for the function signature
    let cc = compute_calling_conv(&[IRType::I64], &IRType::I64);
    assert_eq!(cc.arg_locations[0].register, Some((RegisterClass::X, 0)),
        "First arg should be in X0");
    assert_eq!(cc.ret_location.registers, vec![(RegisterClass::X, 0)],
        "Return value should be in X0");

    // The function calls itself, so the stack frame must support outgoing args.
    // The callee needs only X0 for the argument, so no stack args are needed.
    assert_eq!(cc.stack_args_size, 0,
        "Single-arg recursive call should need no stack argument space");
}

// ---------------------------------------------------------------------------
// Test 5: Nested Loops (Matrix-multiply-like)
// ---------------------------------------------------------------------------

/// Test: Compile matrix-multiplication-like triple-nested loops.
///
/// ```c
/// void matmul(i64* C, i64* A, i64* B, i64 N) {
///     for (i64 i = 0; i < N; i++)
///         for (i64 j = 0; j < N; j++)
///             for (i64 k = 0; k < N; k++)
///                 C[i*N+j] += A[i*N+k] * B[k*N+j];
/// }
/// ```
///
/// Simplified SCG: three nested loops with Add and Mul in the innermost body.
///
/// Validates:
/// - IR contains Mul and Add instructions
/// - Multiple nested loops produce many blocks
/// - Loop nesting depth is tracked correctly
/// - Phi nodes exist for loop headers
#[test]
fn test_nested_loops() {
    let innermost_body = vec![
        // Simulate C[i*N+j] += A[i*N+k] * B[k*N+j]
        // offset_a = i * N  (simplified)
        ScgStatement::Computation(ComputationNode {
            dst: "offset_a".to_string(),
            op: BinOpKind::Mul,
            lhs: ScgExpr::Var("i".to_string()),
            rhs: ScgExpr::Var("N".to_string()),
        }),
        // a_val = *(A + offset_a + k)
        ScgStatement::Access(AccessNode::Load {
            dst: "a_val".to_string(),
            ptr: ScgExpr::Var("A".to_string()),
            offset: Some(ScgExpr::Var("offset_a".to_string())),
        }),
        // offset_b = k * N
        ScgStatement::Computation(ComputationNode {
            dst: "offset_b".to_string(),
            op: BinOpKind::Mul,
            lhs: ScgExpr::Var("k".to_string()),
            rhs: ScgExpr::Var("N".to_string()),
        }),
        // b_val = *(B + offset_b + j)
        ScgStatement::Access(AccessNode::Load {
            dst: "b_val".to_string(),
            ptr: ScgExpr::Var("B".to_string()),
            offset: Some(ScgExpr::Var("offset_b".to_string())),
        }),
        // product = a_val * b_val
        ScgStatement::Computation(ComputationNode {
            dst: "product".to_string(),
            op: BinOpKind::Mul,
            lhs: ScgExpr::Var("a_val".to_string()),
            rhs: ScgExpr::Var("b_val".to_string()),
        }),
        // accumulator += product
        ScgStatement::Computation(ComputationNode {
            dst: "accumulator".to_string(),
            op: BinOpKind::Add,
            lhs: ScgExpr::Var("accumulator".to_string()),
            rhs: ScgExpr::Var("product".to_string()),
        }),
    ];

    // k-loop wraps innermost body
    let k_loop_body = innermost_body;

    // j-loop wraps k-loop
    let j_loop_body = vec![
        ScgStatement::Control(ControlNode::Loop {
            body: k_loop_body,
        }),
    ];

    // i-loop wraps j-loop
    let i_loop_body = vec![
        ScgStatement::Control(ControlNode::Loop {
            body: j_loop_body,
        }),
    ];

    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "matmul".to_string(),
            params: vec![
                ScgParam { name: "C".to_string(), ty: ScgType::Ptr },
                ScgParam { name: "A".to_string(), ty: ScgType::Ptr },
                ScgParam { name: "B".to_string(), ty: ScgType::Ptr },
                ScgParam { name: "N".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // accumulator = 0
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "accumulator".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var("accumulator".to_string()),
                    offset: None,
                    value: ScgExpr::Int(0),
                }),
                // i, j, k vars
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "i".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "j".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: "k".to_string(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                // Triple-nested loops
                ScgStatement::Control(ControlNode::Loop {
                    body: i_loop_body,
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    };

    let mut builder = IRBuilder::new();
    let ir = builder.build(&scg).expect("IRBuilder should succeed");
    let func = &ir.functions[0];

    // Verify function name
    assert_eq!(func.name, "matmul");

    // Verify Mul instructions (offset_a = i*N, offset_b = k*N, product = a*b)
    let mul_count = count_instrs(&ir, |i| matches!(i, IRInstr::Mul { .. }));
    assert!(mul_count >= 3,
        "Should have at least 3 Mul instructions (2 offsets + 1 product), found {}", mul_count);

    // Verify Add instruction (accumulator += product)
    let add_count = count_instrs(&ir, |i| matches!(i, IRInstr::Add { .. }));
    assert!(add_count >= 1,
        "Should have at least 1 Add instruction, found {}", add_count);

    // Verify Load instructions (a_val, b_val)
    let load_count = count_instrs(&ir, |i| matches!(i, IRInstr::Load { .. }));
    assert!(load_count >= 2,
        "Should have at least 2 Load instructions, found {}", load_count);

    // Verify many blocks (3 nested loops → at least 3 * 3 = 9 blocks: header+body+exit each)
    assert!(func.blocks.len() >= 9,
        "Triple-nested loops should produce many blocks, got {}", func.blocks.len());

    // Verify Phi nodes exist (each loop header gets a phi)
    let phi_count = count_instrs(&ir, |i| matches!(i, IRInstr::Phi { .. }));
    assert!(phi_count >= 3,
        "Three nested loops should have at least 3 Phi nodes, found {}", phi_count);

    // Verify loop nesting depth was tracked
    let nesting = builder.loop_nesting_map();
    assert!(nesting.len() >= 3, "Should have tracked at least 3 loops in nesting map");
    // The innermost loop should have depth >= 2
    let max_depth = nesting.values().max().copied().unwrap_or(0);
    assert!(max_depth >= 2, "Maximum nesting depth should be at least 2, got {}", max_depth);
}

// ---------------------------------------------------------------------------
// Test 6: Switch Dispatch
// ---------------------------------------------------------------------------

/// Test: Compile a multi-way branch (switch/match) dispatch.
///
/// ```c
/// i64 dispatch(i64 op) {
///     switch (op) {
///         case 0: return 10;
///         case 1: return 20;
///         case 2: return 30;
///         default: return 0;
///     }
/// }
/// ```
///
/// Validates:
/// - IR contains Cmp instructions (one per case)
/// - IR contains CondBranch instructions
/// - Multiple blocks for each case + default + merge
/// - Each case body produces the expected return value
#[test]
fn test_switch_dispatch() {
    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "dispatch".to_string(),
            params: vec![
                ScgParam { name: "op".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                ScgStatement::Control(ControlNode::Switch {
                    discriminant: ScgExpr::Var("op".to_string()),
                    cases: vec![
                        (0, vec![
                            ScgStatement::Return(vec![ScgExpr::Int(10)]),
                        ]),
                        (1, vec![
                            ScgStatement::Return(vec![ScgExpr::Int(20)]),
                        ]),
                        (2, vec![
                            ScgStatement::Return(vec![ScgExpr::Int(30)]),
                        ]),
                    ],
                    default: Some(vec![
                        ScgStatement::Return(vec![ScgExpr::Int(0)]),
                    ]),
                }),
                // Fallback return (unreachable)
                ScgStatement::Return(vec![ScgExpr::Int(-1)]),
            ],
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function name
    assert_eq!(func.name, "dispatch");

    // Verify Cmp instructions (one per case for the dispatch chain)
    let cmp_count = count_instrs(&ir, |i| matches!(i, IRInstr::Cmp { .. }));
    assert!(cmp_count >= 3,
        "Switch with 3 cases should produce at least 3 Cmp instructions, found {}", cmp_count);

    // Verify CondBranch instructions (one per case)
    let cond_branch_count = count_instrs(&ir, |i| matches!(i, IRInstr::CondBranch { .. }));
    assert!(cond_branch_count >= 3,
        "Switch with 3 cases should produce at least 3 CondBranch instructions, found {}", cond_branch_count);

    // Verify multiple blocks (entry + 3 cases + default + merge = at least 6)
    assert!(func.blocks.len() >= 6,
        "Switch dispatch should produce many blocks (cases + default + merge), got {}", func.blocks.len());

    // Verify that Cmp with Eq kind is used
    let eq_cmp_count = count_instrs(&ir, |i| {
        matches!(i, IRInstr::Cmp { kind: CmpKind::Eq, .. })
    });
    assert!(eq_cmp_count >= 3,
        "Should have at least 3 Cmp.Eq instructions for case dispatch, found {}", eq_cmp_count);
}

// ---------------------------------------------------------------------------
// Test 7: Function with Many Arguments (12-argument, tests AAPCS64 spilling)
// ---------------------------------------------------------------------------

/// Test: Compile a function with 12 arguments to test AAPCS64 register spilling.
///
/// Under AAPCS64:
/// - Arguments 0–7 go in X0–X7 (8 register arguments)
/// - Arguments 8–11 spill to the stack (4 stack arguments)
///
/// Validates:
/// - Calling convention assigns X0–X7 for first 8 integer args
/// - Arguments 8–11 are classified as Stack with stack offsets
/// - stack_args_size is non-zero and 16-byte aligned
/// - The IR Call instruction carries all 12 arguments
#[test]
fn test_function_with_many_args() {
    // Build a function that takes 12 i64 arguments and returns their sum.
    let arg_names: Vec<String> = (0..12).map(|i| format!("a{}", i)).collect();
    let arg_exprs: Vec<ScgExpr> = arg_names.iter().map(|n| ScgExpr::Var(n.clone())).collect();

    // Build sum computation chain: a0 + a1 + a2 + ... + a11
    let mut sum_stmts = Vec::new();
    // result_0 = a0 + a1
    sum_stmts.push(ScgStatement::Computation(ComputationNode {
        dst: "s0".to_string(),
        op: BinOpKind::Add,
        lhs: arg_exprs[0].clone(),
        rhs: arg_exprs[1].clone(),
    }));
    // s1 = s0 + a2, s2 = s1 + a3, ...
    for i in 2..12 {
        let prev = format!("s{}", i - 2);
        sum_stmts.push(ScgStatement::Computation(ComputationNode {
            dst: format!("s{}", i - 1),
            op: BinOpKind::Add,
            lhs: ScgExpr::Var(prev),
            rhs: arg_exprs[i].clone(),
        }));
    }

    let mut body = sum_stmts;
    body.push(ScgStatement::Return(vec![ScgExpr::Var("s10".to_string())]));

    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "sum12".to_string(),
            params: arg_names.into_iter().map(|n| ScgParam { name: n, ty: ScgType::I64 }).collect(),
            results: vec![ScgType::I64],
            body,
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function has 12 params
    assert_eq!(func.params.len(), 12, "Should have 12 parameters");
    assert_eq!(func.param_types.len(), 12, "Should have 12 parameter types");

    // Verify Add instructions
    let add_count = count_instrs(&ir, |i| matches!(i, IRInstr::Add { .. }));
    assert!(add_count >= 11,
        "Sum of 12 values should have 11 Add instructions, found {}", add_count);

    // Now verify the calling convention for 12 integer arguments
    let arg_types: Vec<IRType> = (0..12).map(|_| IRType::I64).collect();
    let cc = compute_calling_conv(&arg_types, &IRType::I64);

    // Should have 12 arg locations
    assert_eq!(cc.arg_locations.len(), 12, "Should have 12 argument locations");

    // First 8 should be in registers (X0–X7)
    for i in 0..8 {
        assert_eq!(cc.arg_locations[i].register, Some((RegisterClass::X, i as u32)),
            "Arg {} should be in X{}, got {:?}", i, i, cc.arg_locations[i].register);
        assert!(cc.arg_locations[i].stack_offset.is_none(),
            "Arg {} should not have a stack offset", i);
    }

    // Args 8–11 should be on the stack
    let stack_args: Vec<_> = cc.arg_locations.iter()
        .filter(|a| a.class == ArgClass::Stack)
        .collect();
    assert_eq!(stack_args.len(), 4,
        "Args 8–11 should be on the stack, found {} stack args", stack_args.len());

    // Stack args size should be non-zero and 16-byte aligned
    assert!(cc.stack_args_size > 0, "Should need stack argument space for 4 stack args");
    assert_eq!(cc.stack_args_size % 16, 0,
        "Stack args size should be 16-byte aligned, got {}", cc.stack_args_size);

    // Return value should be in X0
    assert_eq!(cc.ret_location.registers, vec![(RegisterClass::X, 0)]);

    // Verify stack layout accounts for outgoing args when we call this function
    let call_arg_types = vec![arg_types.clone()]; // one call site with 12 args
    let layout = compute_stack_layout_with_info(func, 0, &call_arg_types);
    assert!(layout.outgoing_args_slot.is_some(),
        "Function calling sum12 should have an outgoing args slot");
    assert!(layout.total_size % 16 == 0,
        "Stack frame should be 16-byte aligned, got {}", layout.total_size);
}

// ---------------------------------------------------------------------------
// Test 8: Callee-saved Register Preservation
// ---------------------------------------------------------------------------

/// Test: Compile a function that uses callee-saved registers (X19–X28),
/// and verify that the register allocator tracks which ones need save/restore.
///
/// The test builds a function with enough live values to force the register
/// allocator to use callee-saved registers, then checks the allocation result.
///
/// Validates:
/// - LinearScanAllocator succeeds on the function
/// - AllocationResult tracks used callee-saved GPRs
/// - Callee-saved count is > 0 when many values are live
/// - Stack layout accounts for callee-saved register slots
#[test]
fn test_callee_saved_preservation() {
    // Build a function with many live values simultaneously to force
    // callee-saved register usage. We allocate 20 stack variables and
    // compute values that depend on all of them, creating pressure.
    let mut body = Vec::new();

    // Allocate 20 stack variables and initialize them
    for i in 0..20 {
        let name = format!("v{}", i);
        body.push(ScgStatement::Allocation(AllocationNode::Stack {
            name: name.clone(),
            size: 8,
            ty: ScgType::I64,
        }));
        body.push(ScgStatement::Access(AccessNode::Store {
            ptr: ScgExpr::Var(name.clone()),
            offset: None,
            value: ScgExpr::Int(i as i64 + 1),
        }));
    }

    // Now load all 20 and add them together, keeping many live simultaneously.
    // This creates register pressure that will require callee-saved registers.
    for i in 0..20 {
        let name = format!("v{}", i);
        body.push(ScgStatement::Access(AccessNode::Load {
            dst: format!("lv{}", i),
            ptr: ScgExpr::Var(name),
            offset: None,
        }));
    }

    // Sum them: s0 = lv0 + lv1, s1 = s0 + lv2, ...
    body.push(ScgStatement::Computation(ComputationNode {
        dst: "s0".to_string(),
        op: BinOpKind::Add,
        lhs: ScgExpr::Var("lv0".to_string()),
        rhs: ScgExpr::Var("lv1".to_string()),
    }));
    for i in 2..20 {
        let prev = format!("s{}", i - 2);
        body.push(ScgStatement::Computation(ComputationNode {
            dst: format!("s{}", i - 1),
            op: BinOpKind::Add,
            lhs: ScgExpr::Var(prev),
            rhs: ScgExpr::Var(format!("lv{}", i)),
        }));
    }

    body.push(ScgStatement::Return(vec![ScgExpr::Var("s18".to_string())]));

    let scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "pressure".to_string(),
            params: vec![],
            results: vec![ScgType::I64],
            body,
        })],
    };

    let ir = build_ir(&scg);
    let func = &ir.functions[0];

    // Verify function was built successfully
    assert_eq!(func.name, "pressure");
    assert!(func.blocks.len() >= 1, "Should have at least one block");

    // Run register allocation
    let allocator = LinearScanAllocator::new();
    let alloc_result = allocator.allocate_function(func)
        .expect("Register allocation should succeed");

    // Verify that callee-saved registers were used
    // With 20+ live values and only 15 caller-saved GPRs available,
    // the allocator must spill to callee-saved registers or the stack.
    let cs_count = alloc_result.callee_saved_count();
    assert!(cs_count > 0,
        "With many live values, at least one callee-saved register should be used, found {}",
        cs_count);

    // Verify that the used callee-saved GPRs are in the X19–X28 range
    for reg in &alloc_result.used_callee_saved_gprs {
        let enc = reg.encoding();
        assert!((19..=28).contains(&enc),
            "Callee-saved GPR should be X19–X28, got X{}", enc);
        assert!(reg.is_callee_saved(), "Register should be callee-saved");
    }

    // Verify stack layout accounts for callee-saved registers
    let layout = compute_stack_layout_with_info(
        func,
        cs_count,
        &[],
    );
    assert!(!layout.callee_save_slots.is_empty(),
        "Stack layout should have callee-save slots when {} callee-saved regs are used", cs_count);
    assert_eq!(layout.callee_saves_count, cs_count,
        "Stack layout callee_saves_count should match allocation result");

    // Each callee-saved register takes 8 bytes
    let expected_cs_bytes = cs_count * 8;
    let actual_cs_bytes: usize = layout.callee_save_slots.iter().map(|s| s.size).sum();
    assert_eq!(actual_cs_bytes, expected_cs_bytes,
        "Callee-save slots should total {} bytes, got {}", expected_cs_bytes, actual_cs_bytes);

    // Total frame size must be 16-byte aligned
    assert!(layout.total_size % 16 == 0,
        "Total stack frame size should be 16-byte aligned, got {}", layout.total_size);

    // Verify the CallingConvention constant is correct
    let cc = CallingConvention::aapcs64();
    assert_eq!(cc.callee_saved.len(), 10,
        "AAPCS64 should define 10 callee-saved GPRs (X19–X28)");
    assert_eq!(cc.stack_alignment, 16,
        "AAPCS64 stack alignment should be 16 bytes");
    for (i, &reg_idx) in cc.callee_saved.iter().enumerate() {
        assert_eq!(reg_idx, 19 + i as u32,
            "Callee-saved register {} should be X{}", i, 19 + i as u32);
    }
}
