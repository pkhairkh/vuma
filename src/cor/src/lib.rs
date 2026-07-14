//! # vuma-cor — Continuous Optimization Runtime
//!
//! The COR (Continuous Optimization Runtime) is the always-on execution
//! engine of the VUMA language framework. Unlike traditional interpreters or
//! JIT compilers that switch between interpreted and compiled modes, COR
//! maintains an **always-compiled invariant**: every reachable region of the
//! Semantic Computation Graph (SCG) is kept in a compiled state at all
//! times.
//!
//! ## Architecture
//!
//! The COR is composed of several cooperating subsystems:
//!
//! - **[`runtime`]** – The central [`CORuntime`] orchestrator that
//!   coordinates compilation, execution, and optimization cycles.
//! - **[`profile`]** – Profile-guided data collection and analysis.
//!   Continuously records branch directions, call frequencies, and
//!   allocation statistics to drive optimization decisions.
//! - **[`speculative`]** – Speculative optimization framework. Allows the
//!   runtime to compile specialized code paths based on assumptions about
//!   runtime behaviour, with automatic deoptimization when assumptions are
//!   invalidated.
//! - **[`deployment`]** – Adaptive deployment across heterogeneous targets
//!   (local, remote). Migrates regions at runtime to rebalance
//!   load.
//! - **[`config`]** – Runtime configuration (optimization level, time
//!   budgets, target architecture, etc.).
//!
//! ## Wave 38 decision: CoR is profiling-only (option b)
//!
//! **Wave 38 task `4-c`** asked the implementer to choose between:
//!
//! - **(a)** call `CORuntime::optimize()` from the pipeline and have
//!   CoR-compiled regions replace the user binary, or
//! - **(b)** document CoR as profiling-only and stop claiming it
//!   optimizes user code.
//!
//! **Decision: (b).**
//!
//! Evidence (see `src/pipeline.rs` Stage 11: COR Initialization,
//! ~line 5261):
//!
//! 1. `pipeline.rs` constructs `CORuntime` at stage 11 *after*
//!    `emit_binary` has already produced the user binary. The user
//!    binary is therefore fixed before CoR exists.
//! 2. Splicing CoR-compiled regions back into the user binary would
//!    require (i) moving CoR construction *before* `emit_binary`, (ii)
//!    rewiring `emit_binary` to consume CoR-compiled regions, and (iii)
//!    teaching the binary emitter about CoR's region model. That is a
//!    large pipeline refactor that the orchestrator will perform in a
//!    final pass — out of scope for Wave 38's file ownership.
//! 3. The pre-Wave-38 `compile_region` (runtime.rs:580-660) compiled
//!    *synthetic* stub functions from SCG metadata (e.g. a `Compute`
//!    node became `arg0 + arg1`). These stubs did not represent real
//!    user code; emitting them as the "optimised binary" would have
//!    been dishonest.
//!
//! **What Wave 38 does instead:**
//!
//! - Makes `CORuntime::optimize()` *honest* via the new
//!   [`CORuntime::optimize_module`] entry point. It runs the 4 real W37
//!   optimisation passes (HotPathInlining, ColdPathOutline,
//!   LoopOptimization, MemoryOptimization) on the runtime's internal
//!   `Arc<SCG>` copy (mutated in place via `Arc::make_mut`) and returns
//!   a structured [`OptimizationSummary`] reporting what changed. It
//!   does **not** claim to replace the user binary.
//! - Removes the synthetic-stub compilation path in `compile_region`
//!   (replaced by a return-zero stub) so the runtime no longer
//!   pretends to compile user code it does not have access to.
//! - Adds the new speculative-optimization entry points
//!   ([`SpeculativeOptimizer::validate_all_speculations`] and
//!   [`SpeculativeOptimizer::apply_speculation`]) that the orchestrator
//!   can wire into the pipeline's verification stage.
//!
//! **Deferred to a final orchestrator pass:**
//!
//! - Move CoR construction before `emit_binary`.
//! - Have `emit_binary` consume CoR-compiled regions (option a).
//!
//! ## Quick start
//!
//! ```no_run
//! use vuma_cor::runtime::CORuntime;
//! use vuma_cor::config::Config;
//! use vuma_cor::types::SCG;
//! use std::sync::Arc;
//!
//! let scg = Arc::new(SCG::default());
//! let config = Config::default();
//! let mut rt = CORuntime::new(scg, config);
//!
//! // After the SCG is updated, compile the delta incrementally.
//! // let delta = vuma_cor::types::Delta::empty();
//! // rt.compile_incremental(&delta);
//!
//! // Execute a compiled region.
//! // rt.execute(1).unwrap();
//!
//! // Run an optimization cycle (Wave 38 entry point).
//! // let summary = rt.optimize_module().unwrap();
//! // assert!(summary.scg_changed());
//! ```

pub mod bridge;
pub mod config;
pub mod deployment;
pub mod optimization;
pub mod ownership;
pub mod profile;
pub mod runtime;
pub mod speculative;
pub mod types;

// Re-export the primary entry point for convenience.
pub use config::Config;
pub use optimization::{apply_optimizations, OptimizationEngine, OptimizationResult};
pub use runtime::{CORuntime, OptimizationSummary, OptError};
pub use speculative::{SpecCode, SpecError, SpecSite};

