//! Cross-function constant propagation tests (Wave 11 LTO).
//!
//! These tests prove that the LTO pass propagates constants from call sites
//! into function bodies — a whole-program optimization that requires seeing
//! all call sites (only possible at link time).

use vuma_codegen::ir::{BinOpKind, IRFunction, IRInstr, IRProgram, IRTerminator, IRType, IRValue};
use vuma_codegen::opt::{cross_function_constant_prop, whole_program_dce};

/// Build a program:
///   fn double(x) { return x * 2; }
///   fn main() { return double(5); }  // double always called with 5
fn make_program() -> IRProgram {
    let mut double = IRFunction::new("double");
    double.params = vec![IRValue::Register(0)]; // x
    double.param_types = vec![IRType::I64];
    double.blocks[0].label = "entry".to_string();
    double.blocks[0].instructions = vec![IRInstr::Mul {
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(2),
        ty: None,
    }];
    double.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
    double.results = vec![IRValue::Register(1)];
    double.result_types = vec![IRType::I64];

    let mut main = IRFunction::new("main");
    main.blocks[0].label = "entry".to_string();
    main.blocks[0].instructions = vec![IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "double".to_string(),
        args: vec![IRValue::Immediate(5)], // always called with 5
        is_extern: false,
    }];
    main.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
    main.results = vec![IRValue::Register(0)];
    main.result_types = vec![IRType::I64];

    IRProgram {
        functions: vec![double, main],
        data_sections: vec![],
    }
}

#[test]
fn wave11_constant_propagation_into_function() {
    // double is always called with 5. After cross_function_constant_prop,
    // the parameter x (reg 0) should be replaced with Immediate(5) in the
    // body of double.
    let program = make_program();
    let result = cross_function_constant_prop(program);

    let double = result.functions.iter().find(|f| f.name == "double").unwrap();
    // The Mul should now use Immediate(5) instead of Register(0).
    let mul = &double.blocks[0].instructions[0];
    match mul {
        IRInstr::Mul { lhs, .. } => {
            assert!(
                matches!(lhs, IRValue::Immediate(5)),
                "after constant prop, x should be replaced with Immediate(5), got {:?}",
                lhs
            );
        }
        other => panic!("expected Mul, got {:?}", other),
    }
}

#[test]
fn wave11_no_propagation_when_arg_varies() {
    // If a function is called with different arguments, no propagation.
    let mut program = make_program();
    // Add a second call site with a different argument.
    let main = program.functions.iter_mut().find(|f| f.name == "main").unwrap();
    main.blocks[0].instructions.push(IRInstr::Call {
        dst: Some(IRValue::Register(1)),
        func: "double".to_string(),
        args: vec![IRValue::Immediate(7)], // different arg!
        is_extern: false,
    });

    let result = cross_function_constant_prop(program);
    let double = result.functions.iter().find(|f| f.name == "double").unwrap();
    let mul = &double.blocks[0].instructions[0];
    match mul {
        IRInstr::Mul { lhs, .. } => {
            // Should NOT be replaced — args vary across call sites.
            assert!(
                matches!(lhs, IRValue::Register(0)),
                "when args vary, x should stay as Register(0), got {:?}",
                lhs
            );
        }
        other => panic!("expected Mul, got {:?}", other),
    }
}

#[test]
fn wave11_dce_removes_unreachable_functions() {
    // A function that is never called should be removed by whole_program_dce.
    let mut program = make_program();
    // Add a dead function that nobody calls.
    let mut dead = IRFunction::new("dead_func");
    dead.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(0),
        lhs: IRValue::Immediate(1),
        rhs: IRValue::Immediate(2),
        ty: None,
    }];
    dead.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
    program.functions.push(dead);

    let result = whole_program_dce(program);
    assert!(
        !result.functions.iter().any(|f| f.name == "dead_func"),
        "dead_func should be removed by whole_program_dce"
    );
    // main and double should still be present.
    assert!(result.functions.iter().any(|f| f.name == "main"));
    assert!(result.functions.iter().any(|f| f.name == "double"));
}

#[test]
fn wave11_dce_keeps_runtime_stubs() {
    // Functions starting with __vuma should always be kept (runtime stubs).
    let mut program = make_program();
    let mut stub = IRFunction::new("__vuma_alloc");
    stub.blocks[0].terminator = IRTerminator::Return(vec![]);
    program.functions.push(stub);

    let result = whole_program_dce(program);
    assert!(
        result.functions.iter().any(|f| f.name == "__vuma_alloc"),
        "__vuma_alloc should be kept (runtime stub)"
    );
}

#[test]
fn wave11_production_compile_with_lto() {
    // End-to-end: compile a multi-function program through the production
    // pipeline (LTO passes are now live) and verify it succeeds.
    use vuma::pipeline::{compile, CompileConfig, OptLevel};

    let source = r#"
        fn helper(x) -> i64 { return x + 1; }
        fn main() -> i64 {
            a = helper(10);
            b = helper(20);
            return a + b;
        }
    "#;

    let config = CompileConfig {
        opt_level: OptLevel::O2,
        ..CompileConfig::default()
    };

    let result = compile(source, &config);
    assert!(result.is_ok(), "LTO compile failed: {:?}", result.err());
    let output = result.unwrap();
    assert!(!output.binary.is_empty());
}
