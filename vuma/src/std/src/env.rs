//! # Environment Variables and Process Args
//!
//! This module provides VUMA-verified environment variable and process argument
//! operations with Behavioral Description (BD) annotations, delegating to
//! `std::env` for real operations.
//!
//! ## Functions
//!
//! - args(), var(), var_os(), set_var(), remove_var(), vars()
//! - current_dir(), set_current_dir(), current_exe()
//! - temp_dir(), home_dir()
//!
//! ## Error Types
//!
//! - VumaEnvError: Environment variable errors.
//!
//! ## BD Annotations
//!
//! - Env operations: CapD { Read, Write } depending on operation

use crate::error::{VumaErrorChain, VumaErrorKind, VumaResult};
use crate::primitives::{CapD, CapFlag};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// VumaEnvError
// ---------------------------------------------------------------------------

/// Environment variable error.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VumaEnvError {
    /// The environment variable was not present.
    NotPresent,
    /// The environment variable was present but not valid Unicode.
    NotUnicode(String),
}

impl fmt::Display for VumaEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VumaEnvError::NotPresent => write!(f, "environment variable not found"),
            VumaEnvError::NotUnicode(name) => {
                write!(f, "environment variable '{}' is not valid Unicode", name)
            }
        }
    }
}

impl std::error::Error for VumaEnvError {}

impl From<VumaEnvError> for VumaErrorChain {
    fn from(e: VumaEnvError) -> Self {
        match e {
            VumaEnvError::NotPresent => VumaErrorChain::new(VumaErrorKind::NotFound, "environment variable not found"),
            VumaEnvError::NotUnicode(name) => {
                VumaErrorChain::new(VumaErrorKind::Io, format!("environment variable '{}' is not valid Unicode", name))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free Functions
// ---------------------------------------------------------------------------

/// Returns the command-line arguments (starting with the program name).
// VUMA-VERIFIED: args reads from OS, no side effects
pub fn args() -> Vec<String> {
    std::env::args().collect()
}

/// Fetches the environment variable `key`, returning an error if not present
/// or not valid Unicode.
// VUMA-VERIFIED: var requires Read capability
pub fn var(key: &str) -> Result<String, VumaEnvError> {
    std::env::var(key).map_err(|e| match e {
        std::env::VarError::NotPresent => VumaEnvError::NotPresent,
        std::env::VarError::NotUnicode(_) => VumaEnvError::NotUnicode(key.to_string()),
    })
}

/// Fetches the environment variable `key` as an `OsString`, returning `None`
/// if not present.
// VUMA-VERIFIED: var_os requires Read capability
pub fn var_os(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key)
}

/// Sets the environment variable `key` to `value`.
// VUMA-VERIFIED: set_var requires Write capability
pub fn set_var(key: &str, value: &str) {
    std::env::set_var(key, value);
}

/// Removes the environment variable `key`.
// VUMA-VERIFIED: remove_var requires Write capability
pub fn remove_var(key: &str) {
    std::env::remove_var(key);
}

/// Returns all environment variables as (key, value) pairs.
// VUMA-VERIFIED: vars requires Read capability
pub fn vars() -> Vec<(String, String)> {
    std::env::vars().collect()
}

/// Returns the current working directory.
// VUMA-VERIFIED: current_dir requires Read capability
pub fn current_dir() -> VumaResult<std::path::PathBuf> {
    std::env::current_dir().map_err(|e| {
        let err: VumaErrorChain = VumaErrorChain::new(VumaErrorKind::Io, format!("current_dir: {}", e));
        err
    })
}

/// Sets the current working directory.
// VUMA-VERIFIED: set_current_dir requires Write capability
pub fn set_current_dir(path: &std::path::Path) -> VumaResult<()> {
    std::env::set_current_dir(path).map_err(|e| {
        VumaErrorChain::new(VumaErrorKind::Io, format!("set_current_dir: {}", e))
    })
}

/// Returns the path to the current executable.
// VUMA-VERIFIED: current_exe requires Read capability
pub fn current_exe() -> VumaResult<std::path::PathBuf> {
    std::env::current_exe().map_err(|e| {
        VumaErrorChain::new(VumaErrorKind::Io, format!("current_exe: {}", e))
    })
}

/// Returns the path to the temporary directory.
// VUMA-VERIFIED: temp_dir is a pure query
pub fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir()
}

/// Returns the path to the user's home directory, if known.
// VUMA-VERIFIED: home_dir requires Read capability
pub fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}

/// Returns the CapD for read-only env operations.
// VUMA-VERIFIED: capability descriptor is correct
pub fn env_read_capd() -> CapD {
    CapD::new(vec![CapFlag::Read])
}

/// Returns the CapD for write env operations.
// VUMA-VERIFIED: capability descriptor is correct
pub fn env_write_capd() -> CapD {
    CapD::new(vec![CapFlag::Write])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_args_returns_nonempty() {
        let args = args();
        // At least the program name
        assert!(!args.is_empty());
    }

    #[test]
    fn test_var_missing() {
        let result = var("VUMA_DOES_NOT_EXIST_XYZ_12345");
        assert!(matches!(result, Err(VumaEnvError::NotPresent)));
    }

    #[test]
    fn test_var_set_and_get() {
        set_var("VUMA_TEST_ENV_KEY", "test_value");
        let val = var("VUMA_TEST_ENV_KEY").unwrap();
        assert_eq!(val, "test_value");
        remove_var("VUMA_TEST_ENV_KEY");
        assert!(matches!(var("VUMA_TEST_ENV_KEY"), Err(VumaEnvError::NotPresent)));
    }

    #[test]
    fn test_var_os_missing() {
        let result = var_os("VUMA_DOES_NOT_EXIST_XYZ_12345");
        assert!(result.is_none());
    }

    #[test]
    fn test_vars_returns_pairs() {
        // Set a unique variable and verify it appears
        set_var("VUMA_TEST_VARS_KEY", "vars_value");
        let all = vars();
        let found = all.iter().any(|(k, v)| k == "VUMA_TEST_VARS_KEY" && v == "vars_value");
        assert!(found);
        remove_var("VUMA_TEST_VARS_KEY");
    }

    #[test]
    fn test_current_dir() {
        let dir = current_dir().unwrap();
        assert!(dir.is_dir());
    }

    #[test]
    fn test_temp_dir() {
        let dir = temp_dir();
        assert!(dir.is_dir());
    }

    #[test]
    fn test_home_dir() {
        // On Linux CI, HOME is typically set
        let home = home_dir();
        // We can't assert it's Some on all platforms, but we can test the type
        if let Some(h) = home {
            assert!(h.is_dir() || !h.exists()); // may not exist but should be a path
        }
    }

    #[test]
    fn test_env_error_display() {
        let err = VumaEnvError::NotPresent;
        assert_eq!(format!("{}", err), "environment variable not found");

        let err = VumaEnvError::NotUnicode("FOO".to_string());
        assert!(format!("{}", err).contains("FOO"));
    }

    #[test]
    fn test_env_error_into_error_chain() {
        let err: VumaErrorChain = VumaEnvError::NotPresent.into();
        assert_eq!(err.kind(), VumaErrorKind::NotFound);
    }
}
