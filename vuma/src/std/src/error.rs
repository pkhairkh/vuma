//! # Unified Error Chain
//!
//! This module provides a VUMA-verified unified error type with Behavioral
//! Description (BD) annotations, error chaining, and a result type alias.
//!
//! ## Types
//!
//! - **VumaError**: The unified error trait with description, cause, source, and kind.
//! - **VumaErrorKind**: Categorises errors by domain (Io, Net, Parse, etc.).
//! - **VumaErrorChain**: A concrete error type supporting chained causes and
//!   context annotations, with `root_cause()` traversal.
//! - **VumaResult\<T\>**: Convenience `Result<T, VumaErrorChain>` alias.
//!
//! ## BD Annotations
//!
//! - VumaErrorChain: CapD { Read, Compare, Serialize }
//! - SyncEdge: none (passive value type)

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// VumaErrorKind
// ---------------------------------------------------------------------------

/// Categorises VUMA errors by domain.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VumaErrorKind {
    /// I/O error (file, stream, UART, MMIO).
    Io,
    /// Network error (TCP, UDP, DNS).
    Net,
    /// Parse error (lexer, parser, deserialization).
    Parse,
    /// Runtime error (evaluator, VM).
    Runtime,
    /// Verification error (BD, IVE, proof).
    Verification,
    /// Code generation error (IR, assembly, ELF).
    Codegen,
    /// Resource not found.
    NotFound,
    /// Permission denied.
    PermissionDenied,
    /// Invalid argument or configuration.
    InvalidArgument,
    /// Timeout exceeded.
    Timeout,
    /// Operation cancelled.
    Cancelled,
    /// Out-of-memory or allocation failure.
    OutOfMemory,
    /// Concurrency error (data race, deadlock, poisoned lock).
    Concurrency,
    /// A catch-all / unknown error kind.
    Other,
}

impl fmt::Display for VumaErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VumaErrorKind::Io => write!(f, "I/O error"),
            VumaErrorKind::Net => write!(f, "network error"),
            VumaErrorKind::Parse => write!(f, "parse error"),
            VumaErrorKind::Runtime => write!(f, "runtime error"),
            VumaErrorKind::Verification => write!(f, "verification error"),
            VumaErrorKind::Codegen => write!(f, "codegen error"),
            VumaErrorKind::NotFound => write!(f, "not found"),
            VumaErrorKind::PermissionDenied => write!(f, "permission denied"),
            VumaErrorKind::InvalidArgument => write!(f, "invalid argument"),
            VumaErrorKind::Timeout => write!(f, "timeout"),
            VumaErrorKind::Cancelled => write!(f, "cancelled"),
            VumaErrorKind::OutOfMemory => write!(f, "out of memory"),
            VumaErrorKind::Concurrency => write!(f, "concurrency error"),
            VumaErrorKind::Other => write!(f, "unknown error"),
        }
    }
}

// ---------------------------------------------------------------------------
// VumaError trait
// ---------------------------------------------------------------------------

/// The unified VUMA error trait.
///
/// Every VUMA error type must implement this trait, providing a description,
/// an optional cause (for error chaining), an optional source, and the
/// error kind.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
pub trait VumaError: fmt::Debug + fmt::Display + Send + Sync + 'static {
    /// Returns a human-readable description of the error.
    // VUMA-VERIFIED: pure accessor
    fn description(&self) -> &str;

    /// Returns the immediate cause of this error, if any.
    // VUMA-VERIFIED: pure accessor
    fn cause(&self) -> Option<&dyn VumaError>;

    /// Returns the underlying source of this error, if any.
    // VUMA-VERIFIED: pure accessor
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)>;

    /// Returns the error kind.
    // VUMA-VERIFIED: pure accessor
    fn kind(&self) -> VumaErrorKind;

    /// Returns the CapD for this error.
    // VUMA-VERIFIED: capability descriptor for error types
    fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }
}

// ---------------------------------------------------------------------------
// VumaErrorChain
// ---------------------------------------------------------------------------

/// A concrete, chainable VUMA error type with BD annotations.
///
/// `VumaErrorChain` supports:
/// - An error kind and message.
/// - An optional source error (chaining).
/// - Optional context strings attached at each chain level.
/// - `root_cause()` traversal to find the original error.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
/// - SyncEdge: none (passive value type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaErrorChain {
    /// The error kind.
    pub kind: VumaErrorKind,
    /// Human-readable error message.
    pub message: String,
    /// Optional context annotation (e.g., "while opening config file").
    pub context: Option<String>,
    /// Optional source error (chaining).
    pub source: Option<Box<VumaErrorChain>>,
}

impl VumaErrorChain {
    /// Create a new error chain with a kind and message.
    // VUMA-VERIFIED: error construction is pure
    pub fn new(kind: VumaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            context: None,
            source: None,
        }
    }

    /// Create a new error chain with a source error.
    // VUMA-VERIFIED: chaining preserves all error information
    pub fn with_source(
        kind: VumaErrorKind,
        message: impl Into<String>,
        source: VumaErrorChain,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            context: None,
            source: Some(Box::new(source)),
        }
    }

    /// Create a new error chain with a context annotation.
    // VUMA-VERIFIED: context addition is pure
    pub fn with_context(
        kind: VumaErrorKind,
        message: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            context: Some(context.into()),
            source: None,
        }
    }

    /// Attach a context annotation to this error (builder pattern).
    // VUMA-VERIFIED: context addition is pure
    pub fn context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }

    /// Attach a source error (builder pattern).
    // VUMA-VERIFIED: source attachment preserves all information
    pub fn source(mut self, src: VumaErrorChain) -> Self {
        self.source = Some(Box::new(src));
        self
    }

    /// Returns the chain of errors from outermost to innermost.
    // VUMA-VERIFIED: pure traversal
    pub fn chain(&self) -> Vec<&VumaErrorChain> {
        let mut result = vec![self];
        let mut current = self.source.as_deref();
        while let Some(src) = current {
            result.push(src);
            current = src.source.as_deref();
        }
        result
    }

    /// Returns the root cause (deepest error in the chain).
    // VUMA-VERIFIED: pure traversal
    pub fn root_cause(&self) -> &VumaErrorChain {
        let mut current = self;
        while let Some(ref src) = current.source {
            current = src;
        }
        current
    }

    /// Returns the error kind.
    // VUMA-VERIFIED: pure accessor
    pub fn kind(&self) -> VumaErrorKind {
        self.kind
    }

    /// Returns the error message.
    // VUMA-VERIFIED: pure accessor
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the context annotation, if any.
    // VUMA-VERIFIED: pure accessor
    pub fn get_context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// Returns the CapD for this error chain.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Returns the RepD for this error chain.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("VumaErrorChain", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this error chain.
    // VUMA-VERIFIED: error chains are passive value types
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl fmt::Display for VumaErrorChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VumaError({}): {}", self.kind, self.message)?;
        if let Some(ref ctx) = self.context {
            write!(f, " [context: {}]", ctx)?;
        }
        if let Some(ref src) = self.source {
            write!(f, "\n  caused by: {}", src)?;
        }
        Ok(())
    }
}

impl std::error::Error for VumaErrorChain {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|b| b as &(dyn std::error::Error + 'static))
    }
}

impl VumaError for VumaErrorChain {
    fn description(&self) -> &str {
        &self.message
    }

    fn cause(&self) -> Option<&dyn VumaError> {
        self.source.as_ref().map(|b| b.as_ref() as &dyn VumaError)
    }

    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|b| b as &(dyn std::error::Error + 'static))
    }

    fn kind(&self) -> VumaErrorKind {
        self.kind
    }
}

// ---------------------------------------------------------------------------
// VumaResult<T>
// ---------------------------------------------------------------------------

/// Convenience result type alias for VUMA operations using `VumaErrorChain`.
pub type VumaResult<T> = Result<T, VumaErrorChain>;

// ---------------------------------------------------------------------------
// From impls
// ---------------------------------------------------------------------------

impl From<std::io::Error> for VumaErrorChain {
    fn from(err: std::io::Error) -> Self {
        let kind = match err.kind() {
            std::io::ErrorKind::NotFound => VumaErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => VumaErrorKind::PermissionDenied,
            std::io::ErrorKind::TimedOut => VumaErrorKind::Timeout,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                VumaErrorKind::InvalidArgument
            }
            std::io::ErrorKind::OutOfMemory => VumaErrorKind::OutOfMemory,
            _ => VumaErrorKind::Io,
        };
        VumaErrorChain::new(kind, err.to_string())
    }
}

impl From<String> for VumaErrorChain {
    fn from(s: String) -> Self {
        VumaErrorChain::new(VumaErrorKind::Other, s)
    }
}

impl From<&str> for VumaErrorChain {
    fn from(s: &str) -> Self {
        VumaErrorChain::new(VumaErrorKind::Other, s)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_kind_display() {
        assert_eq!(VumaErrorKind::Io.to_string(), "I/O error");
        assert_eq!(VumaErrorKind::Net.to_string(), "network error");
        assert_eq!(VumaErrorKind::Parse.to_string(), "parse error");
        assert_eq!(
            VumaErrorKind::Verification.to_string(),
            "verification error"
        );
        assert_eq!(VumaErrorKind::NotFound.to_string(), "not found");
    }

    #[test]
    fn test_error_chain_new() {
        let err = VumaErrorChain::new(VumaErrorKind::Io, "file not found");
        assert_eq!(err.kind(), VumaErrorKind::Io);
        assert_eq!(err.message(), "file not found");
        assert!(err.get_context().is_none());
        assert!(err.source.is_none());
    }

    #[test]
    fn test_error_chain_with_source() {
        let inner = VumaErrorChain::new(VumaErrorKind::Io, "disk read failed");
        let outer =
            VumaErrorChain::with_source(VumaErrorKind::Runtime, "could not load config", inner);
        assert_eq!(outer.kind(), VumaErrorKind::Runtime);
        assert!(outer.source.is_some());
        assert_eq!(outer.source.as_ref().unwrap().kind(), VumaErrorKind::Io);
    }

    #[test]
    fn test_error_chain_with_context() {
        let err = VumaErrorChain::with_context(
            VumaErrorKind::Parse,
            "invalid syntax",
            "while parsing config file",
        );
        assert_eq!(err.get_context(), Some("while parsing config file"));
    }

    #[test]
    fn test_error_chain_root_cause() {
        let inner = VumaErrorChain::new(VumaErrorKind::Io, "disk read failed");
        let mid = VumaErrorChain::with_source(VumaErrorKind::Runtime, "config load error", inner);
        let outer =
            VumaErrorChain::with_source(VumaErrorKind::Verification, "BD check failed", mid);
        let root = outer.root_cause();
        assert_eq!(root.kind(), VumaErrorKind::Io);
        assert_eq!(root.message(), "disk read failed");
    }

    #[test]
    fn test_error_chain_traversal() {
        let inner = VumaErrorChain::new(VumaErrorKind::Io, "disk error");
        let outer = VumaErrorChain::with_source(VumaErrorKind::Runtime, "config error", inner);
        let chain = outer.chain();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].kind(), VumaErrorKind::Runtime);
        assert_eq!(chain[1].kind(), VumaErrorKind::Io);
    }

    #[test]
    fn test_from_std_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let vuma_err: VumaErrorChain = io_err.into();
        assert_eq!(vuma_err.kind(), VumaErrorKind::NotFound);
    }

    #[test]
    fn test_from_string_and_str() {
        let from_str: VumaErrorChain = "something went wrong".into();
        assert_eq!(from_str.kind(), VumaErrorKind::Other);
        assert_eq!(from_str.message(), "something went wrong");

        let from_string: VumaErrorChain = String::from("also wrong").into();
        assert_eq!(from_string.kind(), VumaErrorKind::Other);
    }

    #[test]
    fn test_error_chain_display() {
        let inner = VumaErrorChain::new(VumaErrorKind::Io, "disk read failed");
        let outer = VumaErrorChain::with_source(VumaErrorKind::Runtime, "config error", inner)
            .context("loading app");
        let display = format!("{}", outer);
        assert!(display.contains("VumaError(runtime error)"));
        assert!(display.contains("config error"));
        assert!(display.contains("context: loading app"));
        assert!(display.contains("caused by: VumaError(I/O error)"));
    }

    #[test]
    fn test_error_trait_impl() {
        let err = VumaErrorChain::new(VumaErrorKind::Parse, "bad input");
        assert_eq!(err.description(), "bad input");
        assert_eq!(err.kind(), VumaErrorKind::Parse);
        assert!(err.cause().is_none());
        assert!(err.source.is_none());
    }

    #[test]
    fn test_vuma_result_type() {
        fn succeed() -> VumaResult<i32> {
            Ok(42)
        }
        fn fail() -> VumaResult<i32> {
            Err(VumaErrorChain::new(VumaErrorKind::Io, "fail"))
        }
        assert_eq!(succeed().unwrap(), 42);
        assert!(fail().is_err());
    }
}
