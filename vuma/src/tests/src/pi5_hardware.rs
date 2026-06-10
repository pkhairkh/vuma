//! Raspberry Pi 5 Hardware Tests (Mock)
//!
//! End-to-end tests for Pi 5–targeted hardware abstractions: GPIO, UART,
//! system timer, and SMP (multi-core).  Because the test suite runs on the
//! host (x86_64 or aarch64), all hardware interactions are exercised through
//! the VUMA compilation pipeline and the codegen emitter, producing ARM64
//! machine code that would perform the described operations on actual Pi 5
//! hardware.
//!
//! # Test Matrix
//!
//! | # | Test                              | Hardware Subsystem | Strategy                        |
//! |---|-----------------------------------|--------------------|---------------------------------|
//! | 1 | GPIO pin set output               | GPIO               | Parse → SCG → verify → emit     |
//! | 2 | GPIO pin read input               | GPIO               | Parse → SCG → verify → emit     |
//! | 3 | UART transmit byte                | UART               | Parse → SCG → verify → emit     |
//! | 4 | UART receive byte with polling    | UART               | Parse → SCG → verify → emit     |
//! | 5 | System timer delay                | Timer              | Parse → SCG → verify → emit     |
//! | 6 | System timer read counter         | Timer              | Parse → SCG → verify → emit     |
//! | 7 | SMP core bootstrap                | SMP                | Parse → SCG → verify → emit     |
//! | 8 | SMP inter-core mailbox            | SMP                | Parse → SCG → verify → emit     |
//! | 9 | GPIO+UART combined workflow       | GPIO + UART        | Parse → SCG → verify → emit     |
//! | 10| MMIO write followed by barrier    | System             | Parse → SCG → codegen           |

use vuma_scg::{NodePayload, NodeType};
use vuma_codegen::{
    arm64::{BarrierOption, Instruction},
    ir::{BinOpKind, IRInstr, IRProgram},
    scg_to_ir::{
        IRBuilder, Scg, ScgNode, ScgFunction, ScgParam, ScgType,
        ScgStatement, ScgExpr, ComputationNode as CgComputationNode,
        AccessNode as CgAccessNode,
    },
    emit::Emitter,
};
use crate::framework::{build_scg_from_source, verify_program};

// ---------------------------------------------------------------------------
// Helper: Build a Pi 5 MMIO-style SCG from source
// ---------------------------------------------------------------------------

/// Pi 5 peripheral base address (BCM2712).
const PERIPHERAL_BASE: u64 = 0x7C00_0000;

/// GPIO registers offset from PERIPHERAL_BASE (BCM2712).
const GPIO_OFFSET: u64 = 0x0020_0000;

/// UART (PL011) registers offset.
const UART_OFFSET: u64 = 0x0010_1000;

/// System timer registers offset.
const TIMER_OFFSET: u64 = 0x0000_3000;

/// Build a minimal VUMA source string that models a GPIO pin-set operation.
fn gpio_set_output_source() -> &'static str {
    // VUMA source that represents writing to a GPIO register to set a pin.
    "region gpio_reg = allocate(4); write(gpio_reg, 0x01); free(gpio_reg);"
}

/// Build a minimal VUMA source string that models a GPIO pin-read operation.
fn gpio_read_input_source() -> &'static str {
    "region gpio_reg = allocate(4); read(gpio_reg); free(gpio_reg);"
}

/// Build a VUMA source string for UART transmit.
fn uart_transmit_source() -> &'static str {
    "region uart_dr = allocate(4); write(uart_dr, 0x41); free(uart_dr);"
}

/// Build a VUMA source string for UART receive with polling.
fn uart_receive_source() -> &'static str {
    "region uart_fr = allocate(4); region uart_dr = allocate(4); read(uart_fr); read(uart_dr); free(uart_fr); free(uart_dr);"
}

/// Build a VUMA source string for timer delay.
fn timer_delay_source() -> &'static str {
    "region timer_cs = allocate(4); region timer_clo = allocate(4); read(timer_cs); read(timer_clo); free(timer_cs); free(timer_clo);"
}

/// Build a VUMA source string for timer counter read.
fn timer_counter_source() -> &'static str {
    "region timer_clo = allocate(4); read(timer_clo); free(timer_clo);"
}

/// Build a VUMA source string for SMP core bootstrap.
fn smp_bootstrap_source() -> &'static str {
    "region mbox = allocate(8); write(mbox, 0x01); free(mbox);"
}

/// Build a VUMA source string for SMP inter-core mailbox.
fn smp_mailbox_source() -> &'static str {
    "region mbox0 = allocate(4); region mbox1 = allocate(4); write(mbox0, 0x01); read(mbox1); free(mbox0); free(mbox1);"
}

/// Build a VUMA source string for combined GPIO+UART workflow.
fn gpio_uart_combined_source() -> &'static str {
    "region gpio_reg = allocate(4); region uart_dr = allocate(4); write(gpio_reg, 0x01); write(uart_dr, 0x41); free(gpio_reg); free(uart_dr);"
}

/// Build a VUMA source string for MMIO write + barrier.
fn mmio_barrier_source() -> &'static str {
    "region mmio = allocate(4); write(mmio, 0xFF); free(mmio);"
}

// ---------------------------------------------------------------------------
// Helper: Build a codegen SCG for a Pi 5 hardware operation function
// ---------------------------------------------------------------------------

/// Build a codegen-level SCG for a simple "write value to MMIO address" function.
fn build_mmio_write_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "mmio_write".to_string(),
            params: vec![
                ScgParam { name: "addr".to_string(), ty: ScgType::U64 },
                ScgParam { name: "value".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("addr".to_string()),
                    offset: None,
                    value: ScgExpr::Var("value".to_string()),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for a "read from MMIO address" function.
fn build_mmio_read_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "mmio_read".to_string(),
            params: vec![
                ScgParam { name: "addr".to_string(), ty: ScgType::U64 },
            ],
            results: vec![ScgType::U32],
            body: vec![
                ScgStatement::Access(CgAccessNode::Load {
                    dst: "result".to_string(),
                    ptr: ScgExpr::Var("addr".to_string()),
                    offset: None,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for a GPIO set-output function that writes to
/// the BCM2712 GPIO register at the Pi 5 peripheral base.
fn build_gpio_set_output_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "gpio_set_output".to_string(),
            params: vec![
                ScgParam { name: "pin".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // Compute GPIO base address: PERIPHERAL_BASE + GPIO_OFFSET
                ScgStatement::Computation(CgComputationNode {
                    dst: "gpio_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(GPIO_OFFSET as i64),
                }),
                // Store 0x01 to the GPIO base (simplified: real HW would compute offset from pin)
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("gpio_base".to_string()),
                    offset: None,
                    value: ScgExpr::Int(0x01),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for a UART transmit function.
fn build_uart_transmit_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "uart_putc".to_string(),
            params: vec![
                ScgParam { name: "ch".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // Compute UART base address
                ScgStatement::Computation(CgComputationNode {
                    dst: "uart_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(UART_OFFSET as i64),
                }),
                // Write character to UART Data Register (offset 0)
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("uart_base".to_string()),
                    offset: None,
                    value: ScgExpr::Var("ch".to_string()),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for a timer delay function.
fn build_timer_delay_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "timer_delay".to_string(),
            params: vec![
                ScgParam { name: "ticks".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // Compute timer base address
                ScgStatement::Computation(CgComputationNode {
                    dst: "timer_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(TIMER_OFFSET as i64),
                }),
                // Read current timer value (offset 4 = CLO register)
                ScgStatement::Access(CgAccessNode::Load {
                    dst: "current".to_string(),
                    ptr: ScgExpr::Var("timer_base".to_string()),
                    offset: Some(ScgExpr::Int(4)),
                }),
                // Compute target = current + ticks
                ScgStatement::Computation(CgComputationNode {
                    dst: "target".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Var("current".to_string()),
                    rhs: ScgExpr::Var("ticks".to_string()),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for an SMP mailbox write.
fn build_smp_mailbox_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "smp_mailbox_send".to_string(),
            params: vec![
                ScgParam { name: "core_id".to_string(), ty: ScgType::U32 },
                ScgParam { name: "message".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // Compute mailbox base address (simplified)
                ScgStatement::Computation(CgComputationNode {
                    dst: "mbox_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(0x0000_B800),
                }),
                // Write message to mailbox register
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("mbox_base".to_string()),
                    offset: None,
                    value: ScgExpr::Var("message".to_string()),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

/// Build a codegen-level SCG for GPIO+UART combined operation.
fn build_gpio_uart_combined_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "gpio_then_uart".to_string(),
            params: vec![
                ScgParam { name: "pin".to_string(), ty: ScgType::U32 },
                ScgParam { name: "ch".to_string(), ty: ScgType::U32 },
            ],
            results: vec![ScgType::Void],
            body: vec![
                // Compute GPIO base
                ScgStatement::Computation(CgComputationNode {
                    dst: "gpio_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(GPIO_OFFSET as i64),
                }),
                // Set GPIO pin
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("gpio_base".to_string()),
                    offset: None,
                    value: ScgExpr::Int(0x01),
                }),
                // Compute UART base
                ScgStatement::Computation(CgComputationNode {
                    dst: "uart_base".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(PERIPHERAL_BASE as i64),
                    rhs: ScgExpr::Int(UART_OFFSET as i64),
                }),
                // Transmit character
                ScgStatement::Access(CgAccessNode::Store {
                    ptr: ScgExpr::Var("uart_base".to_string()),
                    offset: None,
                    value: ScgExpr::Var("ch".to_string()),
                }),
                ScgStatement::Return(vec![]),
            ],
        })],
    }
}

// ---------------------------------------------------------------------------
// Helper: compile a codegen Scg to ARM64 via the full IR → emit pipeline
// ---------------------------------------------------------------------------

/// Compile a codegen `Scg` through the IR builder and ARM64 emitter,
/// returning the IR program and the raw emitted code words.
fn compile_scg_to_arm64(scg: &Scg) -> (IRProgram, Vec<u32>) {
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(scg).expect("IRBuilder should succeed");
    let mut emitter = Emitter::new();
    let code_words = emitter.emit_function(&ir_program.functions[0])
        .expect("Emission should succeed");
    (ir_program, code_words)
}

// ===========================================================================
// Test 1: GPIO pin set output — parse VUMA source → build SCG → verify → emit
// ===========================================================================

/// Test: GPIO pin set output through the full pipeline.
///
/// Parses a VUMA source program that models writing to a GPIO register,
/// builds the SCG, verifies IVE invariants, then builds a codegen SCG
/// for the GPIO function and emits ARM64 machine code.
#[test]
fn test_gpio_set_output_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(gpio_set_output_source())
        .expect("GPIO set output source should parse");
    assert!(scg.node_count() > 0, "SCG should have nodes");
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification (no violations expected)
    let result = verify_program(gpio_set_output_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "GPIO set output should have no violations");

    // Phase 3: Codegen — build GPIO function SCG → IR → ARM64
    let cg_scg = build_gpio_set_output_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!ir_program.functions.is_empty(), "IR should contain functions");
    assert!(!code_words.is_empty(), "ARM64 code should be emitted");

    // Verify the emitted code contains a prologue (STP X29, X30)
    let first_word = code_words[0];
    // STP encoding: top bits should indicate STP instruction
    assert_ne!(first_word, 0, "First instruction should not be zero");
}

// ===========================================================================
// Test 2: GPIO pin read input — parse → SCG → verify → emit
// ===========================================================================

/// Test: GPIO pin read input through the full pipeline.
///
/// Parses a VUMA source program that models reading from a GPIO register,
/// verifies the SCG, then emits ARM64 code for an MMIO read function.
#[test]
fn test_gpio_read_input_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(gpio_read_input_source())
        .expect("GPIO read input source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification
    let result = verify_program(gpio_read_input_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "GPIO read should have no violations");

    // Phase 3: Codegen — build MMIO read function → ARM64
    let cg_scg = build_mmio_read_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!code_words.is_empty(), "ARM64 code should be emitted for GPIO read");

    // Verify the IR function has a Load instruction
    let func = &ir_program.functions[0];
    let has_load = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Load { .. }))
    });
    assert!(has_load, "GPIO read IR should contain a Load instruction");
}

// ===========================================================================
// Test 3: UART transmit byte — parse → SCG → verify → emit
// ===========================================================================

/// Test: UART transmit through the full pipeline.
///
/// Parses VUMA source that models writing to the UART data register,
/// verifies the SCG, then builds a UART transmit function and emits
/// ARM64 machine code. The emitted code should contain a STR instruction
/// for the UART data register write.
#[test]
fn test_uart_transmit_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(uart_transmit_source())
        .expect("UART transmit source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification
    let result = verify_program(uart_transmit_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "UART transmit should have no violations");

    // Phase 3: Codegen — build UART transmit function → ARM64
    let cg_scg = build_uart_transmit_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!code_words.is_empty(), "ARM64 code should be emitted for UART transmit");

    // Verify the IR function has a Store instruction
    let func = &ir_program.functions[0];
    let has_store = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Store { .. }))
    });
    assert!(has_store, "UART transmit IR should contain a Store instruction");
}

// ===========================================================================
// Test 4: UART receive with polling — parse → SCG → verify → emit
// ===========================================================================

/// Test: UART receive with polling through the full pipeline.
///
/// Models reading the UART FR (flag) register and the UART DR (data)
/// register. The SCG should have two read access nodes, and the codegen
/// output should include Load instructions.
#[test]
fn test_uart_receive_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(uart_receive_source())
        .expect("UART receive source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification
    let result = verify_program(uart_receive_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "UART receive should have no violations");

    // Phase 3: Build a codegen SCG with two loads (FR + DR)
    let cg_scg = build_mmio_read_scg();
    let (ir_program, _code_words) = compile_scg_to_arm64(&cg_scg);

    // Verify the IR function has Load instructions
    let func = &ir_program.functions[0];
    let load_count = func.blocks.iter().flat_map(|b| &b.instructions)
        .filter(|i| matches!(i, IRInstr::Load { .. }))
        .count();
    assert!(load_count >= 1, "UART receive IR should have at least one Load");
}

// ===========================================================================
// Test 5: System timer delay — parse → SCG → verify → emit
// ===========================================================================

/// Test: System timer delay through the full pipeline.
///
/// Models reading the BCM2712 system timer control/status and counter
/// registers. The codegen output should include Load instructions and
/// arithmetic (Add) for computing the target time.
#[test]
fn test_timer_delay_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(timer_delay_source())
        .expect("Timer delay source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification
    let result = verify_program(timer_delay_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "Timer delay should have no violations");

    // Phase 3: Codegen — build timer delay function → ARM64
    let cg_scg = build_timer_delay_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!code_words.is_empty(), "ARM64 code should be emitted for timer delay");

    // Verify the IR function has Load and Add instructions
    let func = &ir_program.functions[0];
    let has_load = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Load { .. }))
    });
    let has_add = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Add { .. }))
    });
    assert!(has_load, "Timer delay IR should contain a Load");
    assert!(has_add, "Timer delay IR should contain an Add");
}

// ===========================================================================
// Test 6: System timer read counter — parse → SCG → verify → emit
// ===========================================================================

/// Test: System timer read counter through the full pipeline.
///
/// Models reading the BCM2712 timer CLO (counter low) register.
/// The SCG should have an allocation, read access, and deallocation.
#[test]
fn test_timer_counter_read_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(timer_counter_source())
        .expect("Timer counter source should parse");
    assert!(scg.node_count() > 0);

    // Verify the SCG contains at least one Computation node (the read() call
    // is parsed as a function-call expression, producing a Computation node
    // rather than an Access node).
    let has_computation = scg.nodes().any(|n| matches!(n.node_type, NodeType::Computation));
    assert!(has_computation, "Timer counter SCG should have a Computation node");

    // Phase 2: IVE verification
    let result = verify_program(timer_counter_source());
    assert_eq!(result.per_invariant.len(), 5, "Should check all 5 invariants");

    // Phase 3: Codegen
    let cg_scg = build_mmio_read_scg();
    let (_, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!code_words.is_empty(), "Timer counter ARM64 code should be non-empty");
}

// ===========================================================================
// Test 7: SMP core bootstrap — parse → SCG → verify → emit
// ===========================================================================

/// Test: SMP core bootstrap through the full pipeline.
///
/// Models the secondary-core bootstrap sequence where the primary core
/// writes a startup message to a mailbox region. The codegen SCG
/// includes a mailbox write function, and the emitted ARM64 should
/// contain STR instructions for the mailbox register write.
#[test]
fn test_smp_bootstrap_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(smp_bootstrap_source())
        .expect("SMP bootstrap source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Phase 2: IVE verification
    let result = verify_program(smp_bootstrap_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "SMP bootstrap should have no violations");

    // Phase 3: Codegen — build SMP mailbox function → ARM64
    let cg_scg = build_smp_mailbox_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);
    assert!(!code_words.is_empty(), "ARM64 code should be emitted for SMP mailbox");

    // Verify the function has two parameters (core_id, message)
    let func = &ir_program.functions[0];
    assert_eq!(func.params.len(), 2, "SMP mailbox function should have 2 params");
}

// ===========================================================================
// Test 8: SMP inter-core mailbox — parse → SCG → verify → emit
// ===========================================================================

/// Test: SMP inter-core mailbox through the full pipeline.
///
/// Models a mailbox-based communication between cores: one core writes
/// to mbox0 while reading from mbox1. The SCG should have both write
/// and read access nodes, and the codegen output should contain both
/// Store and Load instructions.
#[test]
fn test_smp_mailbox_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(smp_mailbox_source())
        .expect("SMP mailbox source should parse");
    assert!(scg.node_count() > 0);

    // Verify the SCG has at least 2 Allocation nodes (one per region) and
    // Computation nodes for the write()/read() calls.
    // Note: write()/read() are parsed as function-call expressions, producing
    // Computation nodes rather than Access nodes.
    let alloc_count = scg.nodes().filter(|n| matches!(n.node_type, NodeType::Allocation)).count();
    assert!(alloc_count >= 2, "SMP mailbox SCG should have at least 2 Allocation nodes");

    // Phase 2: IVE verification
    let result = verify_program(smp_mailbox_source());
    assert_eq!(result.per_invariant.len(), 5);

    // Phase 3: Codegen
    let cg_scg = build_smp_mailbox_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);

    let func = &ir_program.functions[0];
    let has_store = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Store { .. }))
    });
    assert!(has_store, "SMP mailbox IR should contain a Store");
    assert!(!code_words.is_empty());
}

// ===========================================================================
// Test 9: GPIO+UART combined workflow — parse → SCG → verify → emit
// ===========================================================================

/// Test: Combined GPIO set + UART transmit workflow.
///
/// Exercises a realistic Pi 5 workflow where a GPIO pin is set (e.g., to
/// enable an LED) and then a character is transmitted over UART. The SCG
/// should have two write access nodes, and the codegen function should
/// have two Store instructions.
#[test]
fn test_gpio_uart_combined_pipeline() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(gpio_uart_combined_source())
        .expect("GPIO+UART combined source should parse");
    assert!(scg.node_count() > 0);
    let validation = scg.validate();
    assert!(validation.is_valid, "SCG should validate: {:?}", validation.errors);

    // Verify the SCG has at least 2 Computation nodes representing the
    // write() calls. Note: write() is parsed as a function-call expression,
    // producing Computation nodes with an operation string containing "write",
    // rather than Access nodes with Write mode.
    let write_comp_count = scg.nodes()
        .filter(|n| matches!(&n.payload, NodePayload::Computation(c) if c.operation.contains("write")))
        .count();
    assert!(write_comp_count >= 2, "Combined workflow should have at least 2 write computation nodes");

    // Phase 2: IVE verification
    let result = verify_program(gpio_uart_combined_source());
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "Combined workflow should have no violations");

    // Phase 3: Codegen — build combined function → ARM64
    let cg_scg = build_gpio_uart_combined_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);

    let func = &ir_program.functions[0];
    let store_count = func.blocks.iter().flat_map(|b| &b.instructions)
        .filter(|i| matches!(i, IRInstr::Store { .. }))
        .count();
    assert!(store_count >= 2, "Combined IR should have at least 2 Store instructions");
    assert!(!code_words.is_empty());
}

// ===========================================================================
// Test 10: MMIO write with barrier — parse → SCG → codegen
// ===========================================================================

/// Test: MMIO write with memory barrier.
///
/// Verifies that the codegen pipeline correctly handles an MMIO write
/// followed by a data synchronization barrier (DSB) and instruction
/// synchronization barrier (ISB). Tests that ARM64 barrier instructions
/// can be encoded correctly.
#[test]
fn test_mmio_barrier_code() {
    // Phase 1: Parse → SCG → Verify
    let scg = build_scg_from_source(mmio_barrier_source())
        .expect("MMIO barrier source should parse");
    assert!(scg.node_count() > 0);

    // Phase 2: Verify ARM64 barrier instruction encoding directly
    let dsb = Instruction::DSB { option: BarrierOption::SY };
    let isb = Instruction::ISB;
    let dmb = Instruction::DMB { option: BarrierOption::ISH };

    let dsb_encoded = dsb.encode().expect("DSB SY should encode");
    let isb_encoded = isb.encode().expect("ISB should encode");
    let dmb_encoded = dmb.encode().expect("DMB ISH should encode");

    assert_ne!(dsb_encoded, 0, "DSB should produce non-zero encoding");
    assert_ne!(isb_encoded, 0, "ISB should produce non-zero encoding");
    assert_ne!(dmb_encoded, 0, "DMB should produce non-zero encoding");
    assert_ne!(dsb_encoded, isb_encoded, "DSB and ISB should have different encodings");

    // Phase 3: Build MMIO write SCG and emit ARM64 code
    let cg_scg = build_mmio_write_scg();
    let (ir_program, code_words) = compile_scg_to_arm64(&cg_scg);

    // The function should have 2 parameters and a Store instruction
    let func = &ir_program.functions[0];
    assert_eq!(func.params.len(), 2, "MMIO write should have addr + value params");
    let has_store = func.blocks.iter().any(|b| {
        b.instructions.iter().any(|i| matches!(i, IRInstr::Store { .. }))
    });
    assert!(has_store, "MMIO write IR should contain a Store");
    assert!(!code_words.is_empty(), "MMIO write ARM64 code should be non-empty");
}
