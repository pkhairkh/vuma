//! VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language Framework
//!
//! This is the root crate that aggregates all workspace members and provides
//! the full compilation pipeline.
//!
//! # Architecture
//!
//! The VUMA framework is organised as a workspace of specialised crates:
//!
//! | Crate           | Purpose                                          |
//! |-----------------|--------------------------------------------------|
//! | `vuma-parser`   | Lexer, parser, AST, and AST → SCG bridge        |
//! | `vuma-scg`      | Semantic Computation Graph (SCG) core            |
//! | `vuma-ive`      | Inference & Verification Engine (5 invariants)   |
//! | `vuma-bd`       | Behavioral Descriptors (RepD, CapD, RelD)        |
//! | `vuma-core`     | Memory State Graph (MSG) and SCG → MSG           |
//! | `vuma-codegen`  | IR lowering, register allocation, ARM64 codegen   |
//! | `vuma-projection` | Textual/visual/conversational projections       |
//! | `vuma-pi5`      | Raspberry Pi 5 bare-metal runtime               |
//! | `vuma-proof`    | Proof generation and checking                    |
//! | `vuma-cor`      | Coordination runtime                             |
//! | `vuma-std`      | Standard library                                 |
//!
//! # Quick Start
//!
//! ```rust
//! use vuma::pipeline::{compile, CompileConfig};
//!
//! let source = "fn main() {}";
//! let config = CompileConfig::default();
//! match compile(source, &config) {
//!     Ok(output) => println!("Compiled {} bytes", output.binary.len()),
//!     Err(errors) => {
//!         for err in &errors {
//!             eprintln!("{}", err);
//!         }
//!     }
//! }
//! ```

#![warn(missing_docs)]

pub mod pipeline;

// Re-export the primary pipeline API at the crate root for convenience.
pub use pipeline::{
    compile, compile_incremental, CompilationOutput, CompileConfig, CompileTarget, DebugInfo,
    IncrementalCache, OptLevel, PipelineStage, SourceFingerprint, VerificationLevel, VumaError,
};
