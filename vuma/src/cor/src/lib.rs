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
//!   (local, remote, Pi 5 cores). Migrates regions at runtime to rebalance
//!   load.
//! - **[`config`]** – Runtime configuration (optimization level, time
//!   budgets, target architecture, etc.).
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
//! // Run an optimization cycle.
//! // rt.optimize();
//! ```

pub mod bridge;
pub mod config;
pub mod deployment;
pub mod optimization;
pub mod profile;
pub mod runtime;
pub mod speculative;
pub mod types;

// Re-export the primary entry point for convenience.
pub use runtime::CORuntime;
pub use config::Config;
pub use optimization::{OptimizationEngine, OptimizationResult, apply_optimizations};
