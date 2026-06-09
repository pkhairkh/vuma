//! # vuma-parser — VUMA language frontend
//!
//! This crate implements the parsing pipeline for the VUMA language:
//!
//! 1. **Lexer** ([`lexer`]) — tokenises source text into a flat token stream.
//! 2. **Parser** ([`parser`]) — builds an abstract syntax tree via recursive
//!    descent with precedence climbing.
//! 3. **AST** ([`ast`]) — typed tree representing the full program structure.
//! 4. **Error reporting** ([`error`]) — structured errors with source spans.
//! 5. **SCG bridge** ([`to_scg`]) — converts the AST into a Structured
//!    Computation Graph for downstream analysis.
//!
//! ## Quick start
//!
//! ```rust
//! use vuma_parser::parser::Parser;
//! use vuma_parser::to_scg::AstToScg;
//!
//! let source = r#"
//!     region memory_pool = allocate(1024);
//!     fn main() {
//!         node_ptr = memory_pool + 64;
//!         header = node_ptr as *NodeHeader;
//!     }
//! "#;
//!
//! let mut parser = Parser::new(source);
//! let program = parser.parse_program().expect("parse");
//! let mut converter = AstToScg::new();
//! let scg = converter.convert(&program).expect("convert");
//! ```
//!
//! ## VUMA language design goals
//!
//! - **Minimal & machine-friendly**: the primary reader is an AI agent, so
//!   the syntax avoids syntactic sugar that obscures semantics.
//! - **Clean textual projection**: a human can still read and write VUMA
//!   code for debugging and review.
//! - **Memory-first**: `allocate`, `free`, `region`, pointer casts, and
//!   dereference are first-class constructs, not library calls.

// Public modules.
pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod to_scg;

// Convenience re-exports for the most commonly used types.
pub use ast::{Block, Expr, Item, Lit, Program, Stmt, Type};
pub use error::{
    Diagnostic, ErrorCollector, ErrorRecovery, ParseError, ParseErrorKind, ParseResult, Severity,
    SourceLocation, Span, format_suggestion, levenshtein, offset_to_location, suggest,
    suggest_keyword, VUMA_KEYWORDS,
};
pub use lexer::{Lexer, Position, Token, TokenKind};
pub use parser::Parser;
pub use to_scg::AstToScg;

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Full pipeline test: source → tokens → AST → SCG.
    #[test]
    fn full_pipeline_example() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn process() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
            free(memory_pool);
        "#;

        // Parse.
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse should succeed");

        // Verify AST structure.
        assert!(program.items.len() >= 2, "should have region + fn + free");

        // Convert to SCG.
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).expect("convert should succeed");

        // SCG should have nodes.
        assert!(scg.node_count() > 0, "SCG should not be empty");
    }

    #[test]
    fn parse_round_trip_keywords() {
        let source = "import \"std\"; export main;";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().expect("parse");
        assert_eq!(program.items.len(), 2);
    }
}
