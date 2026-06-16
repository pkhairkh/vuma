//! # vuma-bd — Behavioral Descriptors
//!
//! This crate implements the **BD** (Behavioral Descriptor) layer of the VUMA
//! framework.  A BD fully characterises a value along three orthogonal axes:
//!
//! 1. **Representation** ([`repd`]) — memory shape, size, alignment.
//! 2. **Capability** ([`capd`]) — permitted operations (read, write, …).
//! 3. **Relational** ([`reld`]) — temporal, dependency, and security relations.
//!
//! The top-level [`BD`] struct composes all three layers and provides
//! compatibility, refinement, and composition queries.
//!
//! # Quick start
//!
//! ```
//! use vuma_bd::{
//!     repd::{RepD, ByteRep, POINTER_SIZE},
//!     capd::{CapD, Capability},
//!     reld::{RelD, Relation},
//!     descriptor::{BD, BDId},
//!     context::Context,
//! };
//!
//! let repd = RepD::Byte(ByteRep { size: 8, align: 8 });
//! let capd = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
//! let reld = RelD::empty();
//! let bd = BD::new(repd, capd, reld);
//! println!("{bd}");
//! ```

pub mod capd;
pub mod capd_lattice;
pub mod context;
pub mod context_solver;
pub mod descriptor;
pub mod error_reporting;
pub mod inference;
pub mod reld;
pub mod reld_refine;
pub mod repd;
pub mod repd_compat;
pub mod unify;

// Convenient re-exports for the most commonly used types.
pub use descriptor::{BDId, BD};
