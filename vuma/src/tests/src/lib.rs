//! VUMA Tests Module
//!
//! Integration and unit tests for the VUMA framework, covering:
//! - Trivial program memory safety tests
//! - Doubly-linked list structure tests
//! - Graph structure tests
//! - Concurrent access tests
//! - BD (Behavioral Descriptor) inference tests
//! - Integration test framework with pipeline helpers, test registry,
//!   helper macros, and SCG builders
//! - Benchmark suite producing [`BenchmarkResult`] { mean_ns, median_ns,
//!   iterations } across 8 categories:
//!   SCG construction, BD inference, MSG construction, IVE verification,
//!   ARM64 codegen, C-equivalent comparison, memory usage, and
//!   end-to-end pipeline
//!
//! # Test Categories
//!
//! | Category       | Module              | Scope                                          |
//! |----------------|---------------------|------------------------------------------------|
//! | Unit           | all                 | Individual crate functions, edge cases         |
//! | Integration    | `framework`         | Cross-crate pipelines (parse -> SCG -> verify) |
//! | Verification   | `trivial`, `dlist`  | IVE invariant checks, proofs                   |
//! | Codegen        | `codegen`           | ARM64 code emission, ELF generation            |
//! | Pi5            | `pi5_hardware`      | Raspberry Pi 5 target tests (GPIO/UART/timer)  |
//! | Pipeline       | `full_pipeline`     | Full compile() pipeline end-to-end             |
//! | Benchmark      | `benchmarks`        | Performance benchmarks (8 categories)           |
//!
//! # Benchmark Result Type
//!
//! All benchmarks in the [`benchmarks`] module produce
//! [`BenchmarkResult`] { mean_ns, median_ns, iterations }, a minimal
//! structured result suitable for CI comparison and reporting:
//!
//! ```rust,ignore
//! use vuma_tests::benchmarks::BenchmarkResult;
//!
//! let result = BenchmarkResult {
//!     name: "scg_construction/100_nodes".to_string(),
//!     mean_ns: 12_345,
//!     median_ns: 11_900,
//!     iterations: 100,
//! };
//! ```
//!
//! # Helper Macros
//!
//! The `framework` module provides declarative macros for annotating tests
//! with categories:
//!
//! - [`vuma_unit_test!`] — unit test category
//! - [`vuma_integration_test!`] — integration test category
//! - [`vuma_verification_test!`] — verification test category
//! - [`vuma_codegen_test!`] — codegen test category
//! - [`vuma_pi5_test!`] — Pi5 target test category

#[cfg(test)]
pub mod trivial;
#[cfg(test)]
pub mod dlist;
#[cfg(test)]
pub mod graph;
#[cfg(test)]
pub mod concurrent;
#[cfg(test)]
pub mod bd_inference;
pub mod framework;
pub mod benchmarks;
#[cfg(test)]
pub mod pi5_hardware;
#[cfg(test)]
pub mod codegen;
#[cfg(test)]
pub mod full_pipeline;
#[cfg(test)]
pub mod e2e_cor;
#[cfg(test)]
pub mod execution_validation;

// Re-export the helper macros from the framework module.
// Note: #[macro_export] macros are already at the crate root, so no
// pub use is needed. These re-exports are kept as documentation anchors.
