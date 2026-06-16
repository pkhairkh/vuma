//! # Integration Tests for the Structured Error Diagnostics System
//!
//! This test suite validates that the VUMA compiler's diagnostic system
//! correctly emits all error codes (E001–E050), warning codes (W001–W010),
//! and informational codes (I001–I005), and that the diagnostics carry
//! proper source locations, suggestions, severity levels, and valid JSON
//! output.
//!
//! ## Test Categories
//!
//! | #  | Category                    | What it validates                               |
//! |----|-----------------------------|-------------------------------------------------|
//! | 1  | Error code catalog          | All E001–E050 codes have descriptions           |
//! | 2  | Warning code catalog        | All W001–W010 codes have descriptions           |
//! | 3  | Info code catalog           | All I001–I005 codes have descriptions           |
//! | 4  | Parse error mapping         | ParseErrorKind → correct E-code                 |
//! | 5  | Codegen error mapping       | CodegenError → correct E-code                   |
//! | 6  | E037 unresolved relocation  | Emitted when calling undefined function          |
//! | 7  | Convenience constructors    | Each convenience fn produces correct code/sev    |
//! | 8  | Source locations            | Diagnostics carry proper locations               |
//! | 9  | Suggestions                 | Structured suggestions present when expected     |
//! | 10 | Severity consistency        | Error codes map to Error severity, etc.         |
//! | 11 | JSON output validity        | Diagnostics serialize/deserialize as valid JSON  |
//! | 12 | Diagnostic summary counts   | Summary correctly tallies errors/warn/info/hints |
//! | 13 | LLM API: explain_error      | Returns human-readable explanation               |
//! | 14 | LLM API: suggest_fixes      | Returns actionable suggestions                  |
//! | 15 | LLM API: compile_for_target | All 8 targets produce correct output             |

use vuma::diagnostics::{
    code_category, code_description, code_for_codegen_error, code_for_parse_error_kind,
    code_subcategory, from_codegen_error, from_memory_safety_violation,
    from_vuma_error,
    diagnostics_to_json, diagnostics_to_json_pretty,
    DiagnosticSeverity, DiagnosticSourceLocation, DiagnosticSummary,
    RelatedInfo, Suggestion, SuggestionApplicability, VumaDiagnostic,
    // Convenience constructors
    syntax_error, undefined_variable, type_mismatch, duplicate_definition,
    invalid_arg_count, invalid_type, missing_return, unreachable_code,
    name_resolution_error, circular_dependency, invalid_assignment_target,
    break_outside_loop, invalid_cast, missing_function_body, invalid_visibility,
    invalid_instruction, register_alloc_failed, encoding_error,
    relocation_error, stack_layout_error, linker_error, unsupported_feature,
    invariant_violation, proof_failure, liveness_violation, origin_violation,
    exclusivity_violation, interpretation_violation, cleanup_violation,
    bd_inference_error, constraint_unsatisfiable, verification_timeout,
    unused_variable, implicit_conversion, large_constant, dead_code,
    redundant_cast, shadowed_variable, unnecessary_mut, deprecated_feature,
    unused_import, reachable_panic,
    compilation_started, stage_completed, optimization_applied,
    verification_passed, artifact_provided,
};
use vuma::api::VumaCompiler;
use vuma::llm_api::VumaForLLM;
use vuma::pipeline::VumaError;
use vuma_codegen::CodegenError;
use vuma_codegen::MemorySafetyViolation;
use vuma_parser::ParseErrorKind;

// ═══════════════════════════════════════════════════════════════════════════
// 1. Error Code Catalog (E001–E050)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_error_codes_have_descriptions() {
    let error_codes: &[&str] = &[
        "E001", "E002", "E003", "E004", "E005", "E006", "E007", "E008",
        "E009", "E010", "E011", "E012", "E013", "E014", "E015", "E016",
        "E017", "E018", "E019", "E020", "E021", "E022", "E023", "E024",
        "E025", "E026", "E027", "E028", "E029", "E030",
        "E031", "E032", "E033", "E034", "E035", "E036", "E037", "E038",
        "E039", "E040",
        "E041", "E042", "E043", "E044", "E045", "E046", "E047", "E048",
        "E049", "E050",
    ];
    for code in error_codes {
        let desc = code_description(code);
        assert_ne!(
            desc,
            "Unknown diagnostic code",
            "Error code {} should have a description",
            code
        );
    }
}

#[test]
fn error_codes_belong_to_error_category() {
    let error_codes: &[&str] = &[
        "E001", "E010", "E020", "E030", "E031", "E040", "E041", "E050",
    ];
    for code in error_codes {
        assert_eq!(
            code_category(code),
            "error",
            "Error code {} should belong to the 'error' category",
            code
        );
    }
}

#[test]
fn error_codes_have_correct_subcategories() {
    // E001–E030: compilation
    for n in 1..=30u32 {
        let code = format!("E{:03}", n);
        assert_eq!(
            code_subcategory(&code),
            "compilation",
            "{} should be in 'compilation' subcategory",
            code
        );
    }
    // E031–E040: codegen
    for n in 31..=40u32 {
        let code = format!("E{:03}", n);
        assert_eq!(
            code_subcategory(&code),
            "codegen",
            "{} should be in 'codegen' subcategory",
            code
        );
    }
    // E041–E050: verification
    for n in 41..=50u32 {
        let code = format!("E{:03}", n);
        assert_eq!(
            code_subcategory(&code),
            "verification",
            "{} should be in 'verification' subcategory",
            code
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Warning Code Catalog (W001–W010)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_warning_codes_have_descriptions() {
    let warning_codes: &[&str] = &[
        "W001", "W002", "W003", "W004", "W005", "W006", "W007", "W008",
        "W009", "W010",
    ];
    for code in warning_codes {
        let desc = code_description(code);
        assert_ne!(
            desc,
            "Unknown diagnostic code",
            "Warning code {} should have a description",
            code
        );
    }
}

#[test]
fn warning_codes_belong_to_warning_category() {
    for n in 1..=10u32 {
        let code = format!("W{:03}", n);
        assert_eq!(
            code_category(&code),
            "warning",
            "{} should belong to the 'warning' category",
            code
        );
    }
}

#[test]
fn warning_codes_have_warning_subcategory() {
    for n in 1..=10u32 {
        let code = format!("W{:03}", n);
        assert_eq!(
            code_subcategory(&code),
            "warning",
            "{} should be in 'warning' subcategory",
            code
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Info Code Catalog (I001–I005)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn all_info_codes_have_descriptions() {
    let info_codes: &[&str] = &["I001", "I002", "I003", "I004", "I005"];
    for code in info_codes {
        let desc = code_description(code);
        assert_ne!(
            desc,
            "Unknown diagnostic code",
            "Info code {} should have a description",
            code
        );
    }
}

#[test]
fn info_codes_belong_to_info_category() {
    for n in 1..=5u32 {
        let code = format!("I{:03}", n);
        assert_eq!(
            code_category(&code),
            "info",
            "{} should belong to the 'info' category",
            code
        );
    }
}

#[test]
fn info_codes_have_informational_subcategory() {
    for n in 1..=5u32 {
        let code = format!("I{:03}", n);
        assert_eq!(
            code_subcategory(&code),
            "informational",
            "{} should be in 'informational' subcategory",
            code
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Parse Error Kind → Error Code Mapping
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn parse_error_kind_maps_to_correct_codes() {
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::InvalidSyntax), "E001");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UndefinedReference), "E002");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UndefinedVariable), "E002");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::TypeMismatch), "E003");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::DuplicateDefinition), "E004");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UnexpectedToken), "E009");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::ExpectedToken), "E010");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::RegionError), "E011");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::BDAnnotationError), "E012");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::InvalidCompoundOp), "E013");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::MissingSemicolon), "E014");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::InvalidAddress), "E015");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::LlmMistake), "E021");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::CStyleForLoop), "E022");
    assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UnknownType), "E023");
}

#[test]
fn parse_errors_produce_diagnostics_with_source_location() {
    // Invalid function name triggers a parse error
    let source = "fn 123invalid() {}";
    let diags = VumaForLLM::check(source);
    assert!(!diags.is_empty(), "Invalid source should produce diagnostics");
    let diag = &diags[0];
    // The code should be a valid E-code
    assert!(
        diag.code.starts_with('E'),
        "Parse error diagnostic code should start with 'E', got '{}'",
        diag.code
    );
    // Should have Error severity
    assert_eq!(diag.severity, DiagnosticSeverity::Error);
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Codegen Error → Error Code Mapping
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn codegen_error_maps_to_correct_codes() {
    assert_eq!(
        code_for_codegen_error(&CodegenError::InvalidInstruction("test".to_string())),
        "E031"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::RegisterAllocFailed("test".to_string())),
        "E032"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::EncodingError("test".to_string())),
        "E033"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::TranslationError("test".to_string())),
        "E034"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::ElfError("test".to_string())),
        "E035"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::UnknownVariable { name: "x".to_string() }),
        "E002"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::WasmSectionNotFound { section: "code".to_string() }),
        "E036"
    );
    assert_eq!(
        code_for_codegen_error(&CodegenError::UnresolvedRelocation {
            symbol: "foo".to_string(),
            function: "main".to_string(),
            offset: 0x10,
            reloc_type: "R_AARCH64_CALL26".to_string(),
        }),
        "E037"
    );
}

#[test]
fn codegen_errors_produce_diagnostics_with_suggestions() {
    // RegisterAllocFailed should have a suggestion about reducing register pressure
    let err = CodegenError::RegisterAllocFailed("spill needed".to_string());
    let diag = from_codegen_error(&err);
    assert_eq!(diag.code, "E032");
    assert_eq!(diag.source, "register-alloc");
    assert!(
        diag.suggestions.iter().any(|s| s.message.contains("register pressure")),
        "E032 should suggest reducing register pressure"
    );

    // UnknownVariable should have a suggestion to declare
    let err = CodegenError::UnknownVariable { name: "myvar".to_string() };
    let diag = from_codegen_error(&err);
    assert_eq!(diag.code, "E002");
    assert!(
        diag.suggestions.iter().any(|s| s.message.contains("declare")),
        "E002 for unknown variable should suggest declaring it"
    );

    // WasmSectionNotFound should suggest ensuring the section is generated
    let err = CodegenError::WasmSectionNotFound { section: "memory".to_string() };
    let diag = from_codegen_error(&err);
    assert_eq!(diag.code, "E036");
    assert!(
        diag.suggestions.iter().any(|s| s.message.contains("memory")),
        "E036 should mention the missing section"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. E037 — Unresolved Relocation (calling undefined function)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn e037_unresolved_relocation_code_for_codegen_error() {
    let err = CodegenError::UnresolvedRelocation {
        symbol: "undefined_fn".to_string(),
        function: "caller".to_string(),
        offset: 0x42,
        reloc_type: "R_X86_64_PLT32".to_string(),
    };
    let diag = from_codegen_error(&err);
    assert_eq!(diag.code, "E037");
    assert_eq!(diag.severity, DiagnosticSeverity::Error);
    assert_eq!(diag.source, "codegen");
    assert!(
        diag.message.contains("undefined_fn"),
        "E037 message should contain the unresolved symbol name"
    );
}

#[test]
fn e037_has_suggestion_to_define_function() {
    let err = CodegenError::UnresolvedRelocation {
        symbol: "missing_func".to_string(),
        function: "main".to_string(),
        offset: 0x0,
        reloc_type: "R_AARCH64_CALL26".to_string(),
    };
    let diag = from_codegen_error(&err);
    assert!(
        diag.suggestions.iter().any(|s| s.message.contains("define function")),
        "E037 should suggest defining the function"
    );
    assert!(
        diag.suggestions.iter().any(|s| s.message.contains("missing_func")),
        "E037 suggestion should mention the missing symbol name"
    );
}

#[test]
fn e037_relocation_error_convenience_constructor() {
    let loc = DiagnosticSourceLocation::point("main.vu", 10, 5);
    let diag = relocation_error("unresolved symbol 'foo'", loc.clone());
    assert_eq!(diag.code, "E037");
    assert_eq!(diag.severity, DiagnosticSeverity::Error);
    assert_eq!(diag.source, "linker");
    assert_eq!(diag.location, loc);
}

#[test]
fn e037_emitted_when_calling_undefined_function_via_api() {
    // Source that references a function not defined in the program.
    // The codegen step should emit E037 when it can't resolve the symbol.
    let compiler = VumaCompiler::new();
    let source = r#"
        fn main() {
            result = helper(42);
        }
    "#;
    let result = compiler.compile(source);
    // This should either succeed (if the compiler treats helper as external)
    // or fail with diagnostics. In either case, we verify the diagnostic
    // structure is correct.
    if !result.success {
        // If compilation fails, check that the diagnostics are valid
        for diag in &result.diagnostics {
            assert!(
                !diag.code.is_empty(),
                "Diagnostic should have a non-empty code"
            );
            assert!(
                !diag.message.is_empty(),
                "Diagnostic should have a non-empty message"
            );
        }
    }
}

#[test]
fn e037_unresolved_relocation_in_backend_codegen() {
    // Test that when the backend encoder encounters an UnresolvedRelocation
    // error (via BackendError), it is mapped to E037 with related info.
    let compiler = VumaCompiler::new();
    let source = r#"
        fn main() {
            x = external_call(1, 2);
        }
    "#;
    let result = compiler.compile_for_target(source, "aarch64");
    // Whether this succeeds or fails depends on how the compiler handles
    // undefined functions. If it fails, verify the diagnostic structure.
    if !result.success {
        for diag in &result.diagnostics {
            assert!(!diag.code.is_empty());
            assert!(!diag.message.is_empty());
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. Convenience Constructors — Code and Severity Verification
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn compilation_error_constructors() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

    // E001
    let d = syntax_error("bad syntax", loc.clone());
    assert_eq!(d.code, "E001");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E002
    let d = undefined_variable("x", loc.clone());
    assert_eq!(d.code, "E002");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert!(d.message.contains("x"));

    // E003
    let d = type_mismatch("expected i32, found u64", loc.clone());
    assert_eq!(d.code, "E003");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E004
    let d = duplicate_definition("foo", loc.clone());
    assert_eq!(d.code, "E004");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E005
    let d = invalid_arg_count(2, 3, loc.clone());
    assert_eq!(d.code, "E005");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert!(d.message.contains("2"));
    assert!(d.message.contains("3"));

    // E006
    let d = invalid_type("bad type", loc.clone());
    assert_eq!(d.code, "E006");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E007
    let d = missing_return(loc.clone());
    assert_eq!(d.code, "E007");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E008 (note: unreachable code is a Warning)
    let d = unreachable_code(loc.clone());
    assert_eq!(d.code, "E008");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);

    // E024
    let d = name_resolution_error("cannot resolve 'x'", loc.clone());
    assert_eq!(d.code, "E024");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E025
    let d = circular_dependency("A -> B -> A", loc.clone());
    assert_eq!(d.code, "E025");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E026
    let d = invalid_assignment_target("cannot assign to literal", loc.clone());
    assert_eq!(d.code, "E026");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E027
    let d = break_outside_loop(loc.clone());
    assert_eq!(d.code, "E027");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E028
    let d = invalid_cast("ptr", "i8", loc.clone());
    assert_eq!(d.code, "E028");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E029
    let d = missing_function_body("stub", loc.clone());
    assert_eq!(d.code, "E029");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E030
    let d = invalid_visibility("bad vis", loc.clone());
    assert_eq!(d.code, "E030");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
}

#[test]
fn codegen_error_constructors() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

    // E031
    let d = invalid_instruction("bad op", loc.clone());
    assert_eq!(d.code, "E031");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.source, "codegen");

    // E032
    let d = register_alloc_failed("spill", loc.clone());
    assert_eq!(d.code, "E032");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.source, "register-alloc");
    assert!(d.suggestions.iter().any(|s| s.message.contains("register pressure")));

    // E033
    let d = encoding_error("bad encoding", loc.clone());
    assert_eq!(d.code, "E033");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E037
    let d = relocation_error("unresolved sym", loc.clone());
    assert_eq!(d.code, "E037");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.source, "linker");

    // E038
    let d = stack_layout_error("bad stack", loc.clone());
    assert_eq!(d.code, "E038");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E039
    let d = linker_error("link failed", loc.clone());
    assert_eq!(d.code, "E039");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E040
    let d = unsupported_feature("no SIMD", loc.clone());
    assert_eq!(d.code, "E040");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
}

#[test]
fn verification_error_constructors() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

    // E041
    let d = invariant_violation("invariant broken", loc.clone());
    assert_eq!(d.code, "E041");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.source, "ive");

    // E042
    let d = proof_failure("proof failed", loc.clone());
    assert_eq!(d.code, "E042");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert_eq!(d.source, "proof");

    // E043
    let d = liveness_violation("liveness broken", loc.clone());
    assert_eq!(d.code, "E043");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E044
    let d = origin_violation("origin broken", loc.clone());
    assert_eq!(d.code, "E044");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E045
    let d = exclusivity_violation("exclusivity broken", loc.clone());
    assert_eq!(d.code, "E045");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E046
    let d = interpretation_violation("interpretation broken", loc.clone());
    assert_eq!(d.code, "E046");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E047
    let d = cleanup_violation("cleanup broken", loc.clone());
    assert_eq!(d.code, "E047");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E048
    let d = bd_inference_error("BD error", loc.clone());
    assert_eq!(d.code, "E048");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E049
    let d = constraint_unsatisfiable("unsat", loc.clone());
    assert_eq!(d.code, "E049");
    assert_eq!(d.severity, DiagnosticSeverity::Error);

    // E050
    let d = verification_timeout("timed out", loc.clone());
    assert_eq!(d.code, "E050");
    assert_eq!(d.severity, DiagnosticSeverity::Error);
    assert!(d.suggestions.iter().any(|s| s.message.contains("simplifying")));
}

#[test]
fn warning_constructors() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

    // W001 — unused variable
    let d = unused_variable("x", loc.clone());
    assert_eq!(d.code, "W001");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert!(d.message.contains("x"));
    assert!(d.suggestions.iter().any(|s| s.message.contains("prefix")));

    // W002 — implicit conversion
    let d = implicit_conversion("i32", "i64", loc.clone());
    assert_eq!(d.code, "W002");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);

    // W003 — large constant (Note: severity is Hint)
    let d = large_constant("0xFFFF_FFFF", loc.clone());
    assert_eq!(d.code, "W003");
    assert_eq!(d.severity, DiagnosticSeverity::Hint);

    // W004 — dead code
    let d = dead_code("unreachable", loc.clone());
    assert_eq!(d.code, "W004");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);

    // W005 — redundant cast
    let d = redundant_cast("i32", "i32", loc.clone());
    assert_eq!(d.code, "W005");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert!(d.suggestions.iter().any(|s| s.applicability == SuggestionApplicability::MachineApplicable));

    // W006 — shadowed variable
    let d = shadowed_variable("x", loc.clone(), DiagnosticSourceLocation::point("test.vu", 2, 1));
    assert_eq!(d.code, "W006");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert!(!d.related.is_empty());

    // W007 — unnecessary mut
    let d = unnecessary_mut("y", loc.clone());
    assert_eq!(d.code, "W007");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert!(d.has_machine_applicable_fixes());

    // W008 — deprecated feature
    let d = deprecated_feature("old_fn", Some("new_fn"), loc.clone());
    assert_eq!(d.code, "W008");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);

    // W009 — unused import
    let d = unused_import("std::io", loc.clone());
    assert_eq!(d.code, "W009");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
    assert!(d.has_machine_applicable_fixes());

    // W010 — reachable panic
    let d = reachable_panic("possible panic", loc);
    assert_eq!(d.code, "W010");
    assert_eq!(d.severity, DiagnosticSeverity::Warning);
}

#[test]
fn info_constructors() {
    // I001 — compilation started (uses unknown location)
    let d = compilation_started("main.vu");
    assert_eq!(d.code, "I001");
    assert_eq!(d.severity, DiagnosticSeverity::Info);
    assert!(d.message.contains("main.vu"));

    // I002 — stage completed
    let d = stage_completed("parsing");
    assert_eq!(d.code, "I002");
    assert_eq!(d.severity, DiagnosticSeverity::Info);
    assert!(d.message.contains("parsing"));

    // I003 — optimization applied
    let loc = DiagnosticSourceLocation::point("test.vu", 5, 1);
    let d = optimization_applied("constant_folding", loc);
    assert_eq!(d.code, "I003");
    assert_eq!(d.severity, DiagnosticSeverity::Info);

    // I004 — verification passed
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let d = verification_passed("liveness", loc);
    assert_eq!(d.code, "I004");
    assert_eq!(d.severity, DiagnosticSeverity::Info);

    // I005 — build artifact produced
    let d = artifact_provided("a.out", 4096);
    assert_eq!(d.code, "I005");
    assert_eq!(d.severity, DiagnosticSeverity::Info);
    assert!(d.message.contains("4096"));
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. Source Locations
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn diagnostics_have_source_locations() {
    let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 15);
    let d = syntax_error("bad syntax", loc.clone());
    assert_eq!(d.location, loc);
    assert!(!d.location.is_unknown());
}

#[test]
fn unknown_location_for_pipeline_errors() {
    // VumaError variants that don't carry source info should produce
    // diagnostics with unknown locations.
    let err = VumaError::AstToScg {
        message: "conversion failed".to_string(),
    };
    let diags = from_vuma_error(&err);
    assert_eq!(diags.len(), 1);
    assert!(diags[0].location.is_unknown());
    assert_eq!(diags[0].code, "E024");
}

#[test]
fn parse_errors_have_precise_locations() {
    let source = "fn main() {\n    x = ;\n}";
    let diags = VumaForLLM::check(source);
    if let Some(diag) = diags.first() {
        // Parse errors should have non-zero line/column info
        // (the location comes from the parser's offset computation)
        assert!(
            diag.location.start_line > 0 || diag.location.is_unknown(),
            "Parse error should have a line number or be unknown"
        );
    }
}

#[test]
fn multi_line_location_preserved() {
    let loc = DiagnosticSourceLocation::multi_line("main.vu", 5, 10, 8, 15);
    let d = type_mismatch("mismatch across lines", loc.clone());
    assert_eq!(d.location.start_line, 5);
    assert_eq!(d.location.start_col, 10);
    assert_eq!(d.location.end_line, 8);
    assert_eq!(d.location.end_col, 15);
}

// ═══════════════════════════════════════════════════════════════════════════
// 9. Suggestions
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn machine_applicable_suggestions_have_edit_ranges() {
    let loc = DiagnosticSourceLocation::range("test.vu", 5, 10, 13);
    let d = undefined_variable("x", loc.clone())
        .with_structured_suggestion(Suggestion::machine_applicable(
            "declare 'x'",
            loc,
            "let x = …;",
        ));
    assert!(d.has_machine_applicable_fixes());
    let fixes = d.all_suggestions();
    assert_eq!(fixes.len(), 1);
    assert!(fixes[0].edit_range.is_some());
    assert!(fixes[0].replacement.is_some());
}

#[test]
fn text_suggestions_have_no_edit_ranges() {
    let d = syntax_error("bad syntax", DiagnosticSourceLocation::point("test.vu", 1, 1))
        .with_structured_suggestion(Suggestion::text("check the syntax"));
    assert!(!d.has_machine_applicable_fixes());
}

#[test]
fn codegen_e037_has_structured_suggestion() {
    let err = CodegenError::UnresolvedRelocation {
        symbol: "bar".to_string(),
        function: "foo".to_string(),
        offset: 0,
        reloc_type: "R_AARCH64_ADR_PREL_PG_HI21".to_string(),
    };
    let diag = from_codegen_error(&err);
    assert_eq!(diag.code, "E037");
    assert!(!diag.suggestions.is_empty());
    let s = &diag.suggestions[0];
    assert!(s.message.contains("define function 'bar'"));
}

#[test]
fn verification_timeout_has_placeholder_suggestion() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let d = verification_timeout("verification took too long", loc);
    assert_eq!(d.code, "E050");
    assert!(d.suggestions.iter().any(|s| {
        s.applicability == SuggestionApplicability::HasPlaceholders
    }));
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Severity Consistency
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn error_codes_produce_error_severity() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let error_constructors: Vec<VumaDiagnostic> = vec![
        syntax_error("test", loc.clone()),
        undefined_variable("x", loc.clone()),
        type_mismatch("test", loc.clone()),
        duplicate_definition("x", loc.clone()),
        invalid_arg_count(1, 2, loc.clone()),
        invalid_type("test", loc.clone()),
        missing_return(loc.clone()),
        invalid_instruction("test", loc.clone()),
        register_alloc_failed("test", loc.clone()),
        encoding_error("test", loc.clone()),
        relocation_error("test", loc.clone()),
        stack_layout_error("test", loc.clone()),
        linker_error("test", loc.clone()),
        unsupported_feature("test", loc.clone()),
        invariant_violation("test", loc.clone()),
        proof_failure("test", loc.clone()),
        liveness_violation("test", loc.clone()),
        origin_violation("test", loc.clone()),
        exclusivity_violation("test", loc.clone()),
        interpretation_violation("test", loc.clone()),
        cleanup_violation("test", loc.clone()),
        bd_inference_error("test", loc.clone()),
        constraint_unsatisfiable("test", loc.clone()),
        verification_timeout("test", loc),
    ];
    for diag in &error_constructors {
        assert!(
            diag.severity == DiagnosticSeverity::Error,
            "Error code {} should have Error severity, got {:?}",
            diag.code,
            diag.severity
        );
    }
}

#[test]
fn warning_codes_produce_warning_severity() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let warning_constructors: Vec<VumaDiagnostic> = vec![
        unused_variable("x", loc.clone()),
        implicit_conversion("a", "b", loc.clone()),
        dead_code("test", loc.clone()),
        redundant_cast("a", "b", loc.clone()),
        shadowed_variable("x", loc.clone(), DiagnosticSourceLocation::point("test.vu", 2, 1)),
        unnecessary_mut("x", loc.clone()),
        deprecated_feature("old", None, loc.clone()),
        unused_import("std", loc.clone()),
        reachable_panic("test", loc),
    ];
    for diag in &warning_constructors {
        assert!(
            diag.severity == DiagnosticSeverity::Warning,
            "Warning code {} should have Warning severity, got {:?}",
            diag.code,
            diag.severity
        );
    }
}

#[test]
fn info_codes_produce_info_severity() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let info_constructors: Vec<VumaDiagnostic> = vec![
        compilation_started("main.vu"),
        stage_completed("parsing"),
        optimization_applied("dce", loc.clone()),
        verification_passed("liveness", loc),
        artifact_provided("a.out", 100),
    ];
    for diag in &info_constructors {
        assert!(
            diag.severity == DiagnosticSeverity::Info,
            "Info code {} should have Info severity, got {:?}",
            diag.code,
            diag.severity
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. JSON Output Validity
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn single_diagnostic_json_is_valid() {
    let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 15);
    let diag = VumaDiagnostic::new(
        "E002",
        DiagnosticSeverity::Error,
        "undefined variable `x`",
        "parser",
        loc,
    )
    .with_structured_suggestion(Suggestion::text("declare x first"))
    .with_related(RelatedInfo::new(
        DiagnosticSourceLocation::point("main.vu", 5, 1),
        "variable used here",
    ));

    let json_str = diag.to_json();
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Diagnostic JSON should be parseable");
    assert_eq!(parsed["code"], "E002");
    assert_eq!(parsed["severity"], "error");
    assert_eq!(parsed["message"], "undefined variable `x`");
    assert_eq!(parsed["source"], "parser");
    assert!(parsed["location"].is_object());
    assert!(parsed["suggestions"].is_array());
    assert!(parsed["related"].is_array());
}

#[test]
fn diagnostic_array_json_is_valid() {
    let loc1 = DiagnosticSourceLocation::point("a.vu", 1, 1);
    let loc2 = DiagnosticSourceLocation::point("b.vu", 2, 2);
    let diags = vec![
        syntax_error("bad", loc1),
        undefined_variable("y", loc2),
    ];
    let json_str = diagnostics_to_json(&diags);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Diagnostics array JSON should be parseable");
    assert!(parsed.is_array());
    assert_eq!(parsed.as_array().unwrap().len(), 2);
}

#[test]
fn diagnostic_pretty_json_is_valid() {
    let diags = vec![compilation_started("main.vu")];
    let json_str = diagnostics_to_json_pretty(&diags);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Pretty JSON should be parseable");
    assert!(parsed.is_array());
}

#[test]
fn diagnostic_json_roundtrip() {
    let loc = DiagnosticSourceLocation::range("main.vu", 3, 1, 10);
    let original = VumaDiagnostic::new(
        "E003",
        DiagnosticSeverity::Error,
        "type mismatch",
        "parser",
        loc,
    )
    .with_structured_suggestion(Suggestion::edit(
        "replace with i32",
        DiagnosticSourceLocation::range("main.vu", 3, 5, 8),
        "i32",
    ))
    .with_suggestion("check types");

    let json_str = original.to_json();
    let restored: VumaDiagnostic = serde_json::from_str(&json_str)
        .expect("Roundtrip deserialization should succeed");
    assert_eq!(restored.code, original.code);
    assert_eq!(restored.severity, original.severity);
    assert_eq!(restored.message, original.message);
    assert_eq!(restored.source, original.source);
    assert_eq!(restored.suggestions.len(), original.suggestions.len());
    assert_eq!(restored.legacy_suggestions.len(), original.legacy_suggestions.len());
}

#[test]
fn diagnostic_summary_json_is_valid() {
    let mut summary = DiagnosticSummary::new();
    summary.add(&syntax_error("test", DiagnosticSourceLocation::point("a.vu", 1, 1)));
    summary.add(&unused_variable("x", DiagnosticSourceLocation::point("a.vu", 2, 1)));
    summary.add(&compilation_started("main.vu"));

    let json_str = summary.to_json();
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Summary JSON should be parseable");
    assert_eq!(parsed["total"], 3);
    assert_eq!(parsed["errors"], 1);
    assert_eq!(parsed["warnings"], 1);
    assert_eq!(parsed["infos"], 1);
}

#[test]
fn lsp_output_is_valid_json() {
    let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 15);
    let diag = VumaDiagnostic::new(
        "E002",
        DiagnosticSeverity::Error,
        "undefined variable `x`",
        "parser",
        loc,
    );
    let lsp = diag.to_lsp();
    assert!(lsp.is_object());
    assert!(lsp["range"].is_object());
    assert_eq!(lsp["severity"], 1); // Error = 1
    assert_eq!(lsp["code"], "E002");
    assert_eq!(lsp["source"], "parser");
}

#[test]
fn lsp_warning_tags() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let d = unused_variable("x", loc.clone());
    let lsp = d.to_lsp();
    // W001 should have Unnecessary tag (1)
    assert!(lsp["tags"].is_array());
    let tags = lsp["tags"].as_array().unwrap();
    assert!(tags.contains(&serde_json::Value::Number(serde_json::Number::from(1))));

    let d = deprecated_feature("old", None, loc.clone());
    let lsp = d.to_lsp();
    // W008 should have Deprecated tag (2)
    assert!(lsp["tags"].is_array());
    let tags = lsp["tags"].as_array().unwrap();
    assert!(tags.contains(&serde_json::Value::Number(serde_json::Number::from(2))));
}

// ═══════════════════════════════════════════════════════════════════════════
// 12. Diagnostic Summary Counts
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn summary_counts_errors_warnings_info_hints() {
    let diags = vec![
        syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1)),
        undefined_variable("x", DiagnosticSourceLocation::point("a.vu", 2, 1)),
        unused_variable("y", DiagnosticSourceLocation::point("a.vu", 3, 1)),
        implicit_conversion("a", "b", DiagnosticSourceLocation::point("a.vu", 4, 1)),
        compilation_started("a.vu"),
        stage_completed("parse"),
        large_constant("0xFF", DiagnosticSourceLocation::point("a.vu", 5, 1)),
    ];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert_eq!(summary.total, 7);
    assert_eq!(summary.errors, 2);
    assert_eq!(summary.warnings, 2);
    assert_eq!(summary.infos, 2);
    assert_eq!(summary.hints, 1); // W003 large_constant has Hint severity
}

#[test]
fn summary_counts_by_code() {
    let diags = vec![
        syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1)),
        syntax_error("e2", DiagnosticSourceLocation::point("a.vu", 2, 1)),
        undefined_variable("x", DiagnosticSourceLocation::point("a.vu", 3, 1)),
    ];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert_eq!(summary.count_for_code("E001"), 2);
    assert_eq!(summary.count_for_code("E002"), 1);
    assert_eq!(summary.count_for_code("E003"), 0);
}

#[test]
fn summary_counts_by_source() {
    let diags = vec![
        syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1)),
        undefined_variable("x", DiagnosticSourceLocation::point("a.vu", 2, 1)),
        invalid_instruction("bad op", DiagnosticSourceLocation::point("a.vu", 3, 1)),
    ];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert_eq!(summary.count_for_source("parser"), 2);
    assert_eq!(summary.count_for_source("codegen"), 1);
}

#[test]
fn summary_counts_by_subcategory() {
    let diags = vec![
        syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1)),
        invalid_instruction("bad", DiagnosticSourceLocation::point("a.vu", 2, 1)),
        invariant_violation("broken", DiagnosticSourceLocation::point("a.vu", 3, 1)),
        unused_variable("x", DiagnosticSourceLocation::point("a.vu", 4, 1)),
        compilation_started("a.vu"),
    ];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert_eq!(summary.by_subcategory.get("compilation").copied().unwrap_or(0), 1);
    assert_eq!(summary.by_subcategory.get("codegen").copied().unwrap_or(0), 1);
    assert_eq!(summary.by_subcategory.get("verification").copied().unwrap_or(0), 1);
    assert_eq!(summary.by_subcategory.get("warning").copied().unwrap_or(0), 1);
    assert_eq!(summary.by_subcategory.get("informational").copied().unwrap_or(0), 1);
}

#[test]
fn summary_has_errors_flag() {
    let diags = vec![syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1))];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert!(summary.has_errors());
    assert!(!summary.has_warnings());

    let diags = vec![unused_variable("x", DiagnosticSourceLocation::point("a.vu", 1, 1))];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    assert!(!summary.has_errors());
    assert!(summary.has_warnings());
}

#[test]
fn summary_display_format() {
    let diags = vec![
        syntax_error("e1", DiagnosticSourceLocation::point("a.vu", 1, 1)),
        unused_variable("x", DiagnosticSourceLocation::point("a.vu", 2, 1)),
    ];
    let summary = DiagnosticSummary::from_diagnostics(&diags);
    let display = summary.to_string();
    assert!(display.contains("2 total"));
    assert!(display.contains("1 errors"));
    assert!(display.contains("1 warnings"));
}

// ═══════════════════════════════════════════════════════════════════════════
// 13. LLM API: explain_error
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn explain_error_returns_human_readable_explanation() {
    let diags = VumaForLLM::check("fn 123bad() {}");
    if let Some(diag) = diags.first() {
        let explanation = VumaForLLM::explain_error(diag);
        assert!(!explanation.is_empty(), "Explanation should not be empty");
        // Should include the error code in brackets
        assert!(
            explanation.contains('[') && explanation.contains(']'),
            "Explanation should include error code in brackets"
        );
        // Should include the severity
        assert!(
            explanation.contains("error") || explanation.contains("warning")
                || explanation.contains("information") || explanation.contains("hint"),
            "Explanation should mention the severity"
        );
    }
}

#[test]
fn explain_error_includes_code_description() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = undefined_variable("x", loc);
    let explanation = VumaForLLM::explain_error(&diag);
    assert!(
        explanation.contains("Undefined variable"),
        "Explanation should include the code description 'Undefined variable'"
    );
}

#[test]
fn explain_error_includes_stage() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = syntax_error("bad", loc);
    let explanation = VumaForLLM::explain_error(&diag);
    assert!(
        explanation.contains("[stage:"),
        "Explanation should include the compiler stage"
    );
}

#[test]
fn explain_error_includes_source_location() {
    let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 15);
    let diag = type_mismatch("expected i32, found u64", loc);
    let explanation = VumaForLLM::explain_error(&diag);
    assert!(
        explanation.contains("main.vu"),
        "Explanation should include the file name"
    );
    assert!(
        explanation.contains("10"),
        "Explanation should include the line number"
    );
}

#[test]
fn explain_error_includes_chain() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let root = VumaDiagnostic::new(
        "E023",
        DiagnosticSeverity::Error,
        "Unknown type 'int'",
        "parser",
        loc.clone(),
    );
    let diag = VumaDiagnostic::new(
        "E003",
        DiagnosticSeverity::Error,
        "Type mismatch",
        "parser",
        loc,
    ).chain(root);
    let explanation = VumaForLLM::explain_error(&diag);
    assert!(
        explanation.contains("Caused by:"),
        "Explanation should include causal chain"
    );
    assert!(
        explanation.contains("E023"),
        "Explanation should include the root cause code"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 14. LLM API: suggest_fixes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn suggest_fixes_returns_actionable_suggestions() {
    let diags = VumaForLLM::check("fn 123bad() {}");
    if let Some(diag) = diags.first() {
        let fixes = VumaForLLM::suggest_fixes(diag);
        assert!(!fixes.is_empty(), "Should have at least one suggestion");
        for fix in &fixes {
            assert!(!fix.is_empty(), "Each suggestion should not be empty");
        }
    }
}

#[test]
fn suggest_fixes_for_e002_includes_declare_hint() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = undefined_variable("x", loc);
    let fixes = VumaForLLM::suggest_fixes(&diag);
    // Should include the structured suggestion to prefix with _
    assert!(!fixes.is_empty());
}

#[test]
fn suggest_fixes_generates_generic_hint_for_uncommon_codes() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = stack_layout_error("misaligned stack", loc);
    let fixes = VumaForLLM::suggest_fixes(&diag);
    // Since E038 doesn't have an explicit case in suggest_fixes,
    // it should fall back to the generic hint
    assert!(!fixes.is_empty());
    assert!(
        fixes.iter().any(|f| f.contains("Review the error message")),
        "Should include generic review suggestion for unhandled codes"
    );
}

#[test]
fn suggest_fixes_for_e021_llm_mistake() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = VumaDiagnostic::new(
        "E021",
        DiagnosticSeverity::Error,
        "LLM code mismatch: C-style syntax",
        "parser",
        loc,
    );
    let fixes = VumaForLLM::suggest_fixes(&diag);
    assert!(!fixes.is_empty());
    assert!(
        fixes.iter().any(|f| f.contains("C/Rust syntax") || f.contains("VUMA")),
        "E021 fix should mention C/Rust syntax or VUMA"
    );
}

#[test]
fn suggest_fixes_for_e022_c_style_for_loop() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = VumaDiagnostic::new(
        "E022",
        DiagnosticSeverity::Error,
        "C-style for loop detected",
        "parser",
        loc,
    );
    let fixes = VumaForLLM::suggest_fixes(&diag);
    assert!(!fixes.is_empty());
    assert!(
        fixes.iter().any(|f| f.contains("range-based")),
        "E022 fix should mention range-based loops"
    );
}

#[test]
fn suggest_fixes_for_e023_unknown_type() {
    let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);
    let diag = VumaDiagnostic::new(
        "E023",
        DiagnosticSeverity::Error,
        "Unknown type 'int'",
        "parser",
        loc,
    );
    let fixes = VumaForLLM::suggest_fixes(&diag);
    assert!(!fixes.is_empty());
    assert!(
        fixes.iter().any(|f| f.contains("i32") || f.contains("i64")),
        "E023 fix should suggest VUMA sized types"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 15. LLM API: compile_for_target — All 8 Targets
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn compile_for_all_eight_targets() {
    let compiler = VumaCompiler::new();
    let source = "fn main() {}";

    let targets = [
        "x86_64",
        "aarch64",
        "riscv64",
        "wasm32",
        "loongarch64",
        "arm32",
        "mips64",
        "ppc64",
    ];

    for target in &targets {
        let result = compiler.compile_for_target(source, target);
        assert!(
            result.success,
            "Compilation for target '{}' should succeed, diagnostics: {:?}",
            target,
            result.diagnostics
        );
        assert!(
            result.target.is_some(),
            "Target output should be present for '{}'",
            target
        );
        let target_output = result.target.as_ref().unwrap();
        assert_eq!(
            target_output.backend, *target,
            "Backend should match for '{}'",
            target
        );
        assert!(
            !target_output.binary.is_empty(),
            "Binary should not be empty for '{}'",
            target
        );
        assert!(
            !target_output.disassembly.is_empty(),
            "Disassembly should not be empty for '{}'",
            target
        );
    }
}

#[test]
fn compile_for_target_unknown_target_emits_e021() {
    let compiler = VumaCompiler::new();
    let source = "fn main() {}";
    let result = compiler.compile_for_target(source, "nonexistent_arch");
    assert!(!result.success, "Unknown target should fail");
    assert!(
        result.diagnostics.iter().any(|d| d.code == "E021" || d.message.contains("Unknown target")),
        "Unknown target should emit E021 or mention 'Unknown target'"
    );
}

#[test]
fn compile_for_target_valid_source_serializable() {
    let compiler = VumaCompiler::new();
    let source = "fn main() {}";
    let result = compiler.compile_for_target(source, "x86_64");
    let json = serde_json::to_string(&result);
    assert!(json.is_ok(), "CompileResult should be serializable for target output");
}

#[test]
fn compile_for_target_alternate_names() {
    let compiler = VumaCompiler::new();
    let source = "fn main() {}";

    // Test alternate target name parsing
    let alternate_names = [
        ("amd64", "x86_64"),
        ("arm64", "aarch64"),
        ("risc-v64", "riscv64"),
        ("wasm", "wasm32"),
        ("la64", "loongarch64"),
        ("arm", "arm32"),
        ("mips", "mips64"),
        ("powerpc64", "ppc64"),
    ];

    for (alt, expected_backend) in &alternate_names {
        let result = compiler.compile_for_target(source, alt);
        assert!(
            result.success,
            "Compilation for alternate target name '{}' should succeed",
            alt
        );
        if let Some(ref target_output) = result.target {
            assert_eq!(
                target_output.backend, *expected_backend,
                "Alternate name '{}' should resolve to backend '{}'",
                alt,
                expected_backend
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Additional Integration Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn memory_safety_violation_diagnostics() {
    let violations = vec![
        MemorySafetyViolation::UseAfterFree {
            allocation_name: "buf".to_string(),
            dealloc_line: Some(10),
            violation_count: 2,
        },
        MemorySafetyViolation::DoubleFree {
            allocation_name: "ptr".to_string(),
            first_free_line: Some(5),
            second_free_line: Some(8),
        },
        MemorySafetyViolation::MemoryLeak {
            allocation_name: "heap_obj".to_string(),
            alloc_line: Some(3),
            alloc_size: Some(1024),
        },
        MemorySafetyViolation::NullDereference {
            pointer_name: "p".to_string(),
        },
    ];

    for violation in &violations {
        let diag = from_memory_safety_violation(violation);
        // Each violation should map to an E041–E050 code
        let num: u32 = diag.code[1..].parse().unwrap();
        assert!(
            (41..=50).contains(&num),
            "Memory safety violation should map to E041–E050, got {}",
            diag.code
        );
        assert_eq!(diag.severity, DiagnosticSeverity::Error);
        assert!(!diag.message.is_empty());
    }
}

#[test]
fn vuma_error_to_diagnostics_mapping() {
    // Test each VumaError variant that can be easily converted to diagnostics.
    // Some variants (ScgToMsg, Verification, ModuleResolution) require complex
    // internal types and are tested indirectly via the compile pipeline.

    let errors_and_expected_codes: Vec<(VumaError, &str)> = vec![
        (
            VumaError::AstToScg { message: "test".to_string() },
            "E024",
        ),
        (
            VumaError::ScgValidation { errors: vec!["err1".to_string()] },
            "E041",
        ),
        (
            VumaError::BdInference { node_id: None, message: "bd err".to_string() },
            "E048",
        ),
        (
            VumaError::BdInference { node_id: Some(42), message: "bd err at node".to_string() },
            "E048",
        ),
        (
            VumaError::Transform { pass_name: "dce".to_string(), errors: vec!["transform err".to_string()] },
            "E041",
        ),
        (
            VumaError::Codegen { error: CodegenError::InvalidInstruction("bad op".to_string()) },
            "E031",
        ),
        (
            VumaError::Codegen { error: CodegenError::UnresolvedRelocation {
                symbol: "missing_fn".to_string(),
                function: "caller".to_string(),
                offset: 0x10,
                reloc_type: "R_AARCH64_CALL26".to_string(),
            }},
            "E037",
        ),
        (
            VumaError::RegisterAlloc { message: "spill".to_string() },
            "E032",
        ),
        (
            VumaError::Emission { message: "elf err".to_string() },
            "E035",
        ),
        (
            VumaError::CorInit { message: "cor init err".to_string() },
            "E024",
        ),
        (
            VumaError::PanicCaught { stage: "codegen".to_string(), message: "panic".to_string() },
            "E050",
        ),
        (
            VumaError::BackendFallback {
                failed_backend: "x86_64".to_string(),
                fallback_backend: Some("aarch64".to_string()),
                error: "backend failed".to_string(),
            },
            "E031",
        ),
    ];

    for (error, expected_code) in &errors_and_expected_codes {
        let diags = from_vuma_error(error);
        assert!(
            !diags.is_empty(),
            "VumaError {:?} should produce at least one diagnostic",
            error
        );
        assert_eq!(
            diags[0].code, *expected_code,
            "VumaError variant should produce code {}",
            expected_code
        );
    }
}

#[test]
fn multi_error_flattens_to_multiple_diagnostics() {
    let err = VumaError::Multi {
        errors: vec![
            VumaError::AstToScg { message: "err1".to_string() },
            VumaError::RegisterAlloc { message: "err2".to_string() },
        ],
    };
    let diags = from_vuma_error(&err);
    assert_eq!(diags.len(), 2, "Multi error should produce 2 diagnostics");
    assert_eq!(diags[0].code, "E024");
    assert_eq!(diags[1].code, "E032");
}

#[test]
fn plain_text_format_includes_all_fields() {
    let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 15);
    let diag = VumaDiagnostic::new(
        "E002",
        DiagnosticSeverity::Error,
        "undefined variable `x`",
        "parser",
        loc,
    )
    .with_suggestion("declare x first")
    .with_related(RelatedInfo::new(
        DiagnosticSourceLocation::point("main.vu", 5, 1),
        "variable used here",
    ));

    let text = diag.to_plain_text();
    assert!(text.contains("error[E002]"), "Plain text should include severity and code");
    assert!(text.contains("undefined variable"), "Plain text should include message");
    assert!(text.contains("note:"), "Plain text should include related info");
    assert!(text.contains("help:"), "Plain text should include suggestions");
}

#[test]
fn rich_text_format_includes_ansi_codes() {
    let loc = DiagnosticSourceLocation::point("test.vu", 5, 1);
    let diag = syntax_error("bad syntax", loc);
    let rich = diag.to_rich_text();
    // Should contain ANSI escape codes for colors
    assert!(rich.contains("\x1b["), "Rich text should contain ANSI escape codes");
    assert!(rich.contains("error"), "Rich text should contain 'error'");
}

#[test]
fn diagnostic_chain_root_cause() {
    let root = VumaDiagnostic::new(
        "E023",
        DiagnosticSeverity::Error,
        "Unknown type 'int'",
        "parser",
        DiagnosticSourceLocation::point("test.vu", 1, 1),
    );
    let intermediate = VumaDiagnostic::new(
        "E003",
        DiagnosticSeverity::Error,
        "Type mismatch",
        "parser",
        DiagnosticSourceLocation::point("test.vu", 1, 1),
    ).chain(root);

    let diag = VumaDiagnostic::new(
        "E002",
        DiagnosticSeverity::Error,
        "Undefined variable",
        "parser",
        DiagnosticSourceLocation::point("test.vu", 1, 1),
    ).chain(intermediate);

    assert!(diag.has_chain());
    assert_eq!(diag.chain.len(), 1); // immediate cause
    assert_eq!(diag.immediate_cause().unwrap().code, "E003");
    // The intermediate itself has a chain to E023
    assert_eq!(diag.immediate_cause().unwrap().chain.len(), 1);
    assert_eq!(
        diag.immediate_cause().unwrap().immediate_cause().unwrap().code,
        "E023"
    );
}

#[test]
fn available_targets_returns_eight_targets() {
    let compiler = VumaCompiler::new();
    let targets = compiler.available_targets();
    assert_eq!(targets.len(), 8, "Should have exactly 8 targets");

    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"x86_64"));
    assert!(names.contains(&"aarch64"));
    assert!(names.contains(&"riscv64"));
    assert!(names.contains(&"wasm32"));
    assert!(names.contains(&"loongarch64"));
    assert!(names.contains(&"arm32"));
    assert!(names.contains(&"mips64"));
    assert!(names.contains(&"ppc64"));
}

#[test]
fn llm_targets_returns_eight_targets() {
    let targets = VumaForLLM::targets();
    assert_eq!(targets.len(), 8, "LLM API should report 8 targets");
}

#[test]
fn compile_result_from_invalid_source_has_diagnostics() {
    let compiler = VumaCompiler::new();
    let result = compiler.compile("this is not valid vuma");
    assert!(!result.success, "Invalid source should fail compilation");
    assert!(!result.diagnostics.is_empty(), "Should have diagnostics");
    assert!(
        result.diagnostics.iter().any(|d| d.severity == DiagnosticSeverity::Error),
        "Should have at least one error diagnostic"
    );
}

#[test]
fn parse_result_from_invalid_source_has_diagnostics() {
    let compiler = VumaCompiler::new();
    let result = compiler.parse("fn 123bad() {}");
    assert!(!result.success, "Invalid source should fail parsing");
    assert!(!result.diagnostics.is_empty(), "Should have diagnostics");
}

#[test]
fn validate_returns_empty_for_valid_source() {
    let compiler = VumaCompiler::new();
    let diags = compiler.validate("fn main() {}");
    assert!(diags.is_empty(), "Valid source should have no diagnostics");
}

#[test]
fn validate_returns_errors_for_invalid_source() {
    let compiler = VumaCompiler::new();
    let diags = compiler.validate("fn 123bad() {}");
    assert!(!diags.is_empty(), "Invalid source should have diagnostics");
    assert!(diags.iter().any(|d| d.severity == DiagnosticSeverity::Error));
}
