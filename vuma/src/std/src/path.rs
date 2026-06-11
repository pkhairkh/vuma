//! # Path Manipulation
//!
//! This module provides VUMA-verified path types with Behavioral Description
//! (BD) annotations, delegating to `std::path` for real path operations.
//!
//! ## Types
//!
//! - **VumaPath**: A borrowed path slice (analogous to `std::path::Path`).
//! - **VumaPathBuf**: An owned, mutable path (analogous to `std::path::PathBuf`).
//! - **PathComponent**: Enum for the components of a path.
//!
//! ## BD Annotations
//!
//! - VumaPath: CapD { Read, Compare, Serialize }
//! - VumaPathBuf: CapD { Read, Write, Compare, Serialize }

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// PathComponent
// ---------------------------------------------------------------------------

/// A component of a path, mirroring `std::path::Component`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PathComponent {
    /// A platform-specific path prefix (e.g., `C:` on Windows).
    Prefix(String),
    /// The root directory component (`/` on Unix).
    RootDir,
    /// The current directory component (`.`).
    CurDir,
    /// The parent directory component (`..`).
    ParentDir,
    /// A normal (named) component.
    Normal(String),
}

impl PathComponent {
    /// Returns the CapD for this component.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Convert from a `std::path::Component`.
    // VUMA-VERIFIED: conversion preserves semantics
    fn from_std(comp: std::path::Component<'_>) -> Self {
        match comp {
            std::path::Component::Prefix(p) => {
                PathComponent::Prefix(p.as_os_str().to_string_lossy().to_string())
            }
            std::path::Component::RootDir => PathComponent::RootDir,
            std::path::Component::CurDir => PathComponent::CurDir,
            std::path::Component::ParentDir => PathComponent::ParentDir,
            std::path::Component::Normal(s) => {
                PathComponent::Normal(s.to_string_lossy().to_string())
            }
        }
    }
}

impl fmt::Display for PathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathComponent::Prefix(s) => write!(f, "{}", s),
            PathComponent::RootDir => write!(f, "/"),
            PathComponent::CurDir => write!(f, "."),
            PathComponent::ParentDir => write!(f, ".."),
            PathComponent::Normal(s) => write!(f, "{}", s),
        }
    }
}

// ---------------------------------------------------------------------------
// VumaPath
// ---------------------------------------------------------------------------

/// A VUMA-verified borrowed path slice, delegating to `std::path::Path`.
///
/// `VumaPath` is the VUMA-verified equivalent of `&std::path::Path`. It
/// carries BD annotations and provides path query operations.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
/// - SyncEdge: none (passive value type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaPath {
    /// The internal path string.
    inner: String,
}

impl VumaPath {
    /// Create a new `VumaPath` from a string.
    // VUMA-VERIFIED: construction is pure
    pub fn new(s: &str) -> Self {
        Self {
            inner: s.to_string(),
        }
    }

    /// Returns the path as a string slice.
    // VUMA-VERIFIED: pure accessor
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Returns the path as a `std::path::Path`.
    // VUMA-VERIFIED: conversion is lossless
    pub fn as_std_path(&self) -> &std::path::Path {
        std::path::Path::new(&self.inner)
    }

    /// Returns the parent directory of this path, if any.
    // VUMA-VERIFIED: pure query
    pub fn parent(&self) -> Option<VumaPath> {
        self.as_std_path().parent().map(|p| VumaPath {
            inner: p.to_string_lossy().to_string(),
        })
    }

    /// Returns the final component of the path, if any.
    // VUMA-VERIFIED: pure query
    pub fn file_name(&self) -> Option<&str> {
        self.as_std_path().file_name().and_then(|s| s.to_str())
    }

    /// Returns the extension of the path, if any.
    // VUMA-VERIFIED: pure query
    pub fn extension(&self) -> Option<&str> {
        self.as_std_path().extension().and_then(|s| s.to_str())
    }

    /// Returns the file stem (name without extension), if any.
    // VUMA-VERIFIED: pure query
    pub fn file_stem(&self) -> Option<&str> {
        self.as_std_path().file_stem().and_then(|s| s.to_str())
    }

    /// Returns a new path with the given extension replaced.
    // VUMA-VERIFIED: produces a valid path
    pub fn with_extension(&self, ext: &str) -> VumaPathBuf {
        let p = self.as_std_path().with_extension(ext);
        VumaPathBuf {
            inner: p.to_string_lossy().to_string(),
        }
    }

    /// Returns a new path with the given file name replaced.
    // VUMA-VERIFIED: produces a valid path
    pub fn with_file_name(&self, name: &str) -> VumaPathBuf {
        let p = self.as_std_path().with_file_name(name);
        VumaPathBuf {
            inner: p.to_string_lossy().to_string(),
        }
    }

    /// Returns `true` if the path is absolute.
    // VUMA-VERIFIED: pure query
    pub fn is_absolute(&self) -> bool {
        self.as_std_path().is_absolute()
    }

    /// Returns `true` if the path is relative.
    // VUMA-VERIFIED: pure query
    pub fn is_relative(&self) -> bool {
        self.as_std_path().is_relative()
    }

    /// Returns `true` if the path has a root component.
    // VUMA-VERIFIED: pure query
    pub fn has_root(&self) -> bool {
        self.as_std_path().has_root()
    }

    /// Returns `true` if this path starts with the given path.
    // VUMA-VERIFIED: pure query
    pub fn starts_with(&self, other: &VumaPath) -> bool {
        self.as_std_path().starts_with(other.as_std_path())
    }

    /// Returns `true` if this path ends with the given path.
    // VUMA-VERIFIED: pure query
    pub fn ends_with(&self, other: &VumaPath) -> bool {
        self.as_std_path().ends_with(other.as_std_path())
    }

    /// Returns the path components.
    // VUMA-VERIFIED: pure traversal
    pub fn components(&self) -> Vec<PathComponent> {
        self.as_std_path()
            .components()
            .map(PathComponent::from_std)
            .collect()
    }

    /// Joins this path with another path.
    // VUMA-VERIFIED: produces a valid path
    pub fn join(&self, other: &VumaPath) -> VumaPathBuf {
        let p = self.as_std_path().join(other.as_std_path());
        VumaPathBuf {
            inner: p.to_string_lossy().to_string(),
        }
    }

    /// Returns the CapD for this path.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Returns the RepD for this path.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("VumaPath", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this path.
    // VUMA-VERIFIED: path is a passive value type
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl fmt::Display for VumaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl AsRef<std::path::Path> for VumaPath {
    fn as_ref(&self) -> &std::path::Path {
        self.as_std_path()
    }
}

// ---------------------------------------------------------------------------
// VumaPathBuf
// ---------------------------------------------------------------------------

/// A VUMA-verified owned, mutable path buffer, delegating to `std::path::PathBuf`.
///
/// `VumaPathBuf` is the VUMA-verified equivalent of `std::path::PathBuf`. It
/// carries BD annotations and supports building paths incrementally.
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Compare, Serialize }
/// - SyncEdge: none (passive value type)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaPathBuf {
    /// The internal path string.
    inner: String,
}

impl VumaPathBuf {
    /// Create a new, empty `VumaPathBuf`.
    // VUMA-VERIFIED: construction is pure
    pub fn new() -> Self {
        Self {
            inner: String::new(),
        }
    }

    /// Create a `VumaPathBuf` from a string.
    // VUMA-VERIFIED: construction is pure
    pub fn from(s: impl Into<String>) -> Self {
        Self { inner: s.into() }
    }

    /// Push a path component onto this path buffer.
    // VUMA-VERIFIED: push appends correctly
    pub fn push(&mut self, component: &str) {
        let mut buf = std::path::PathBuf::from(&self.inner);
        buf.push(component);
        self.inner = buf.to_string_lossy().to_string();
    }

    /// Pop the last component from this path buffer.
    ///
    /// Returns `true` if a component was popped, `false` if the path was
    /// already empty or at the root.
    // VUMA-VERIFIED: pop removes the last component correctly
    pub fn pop(&mut self) -> bool {
        let mut buf = std::path::PathBuf::from(&self.inner);
        let result = buf.pop();
        self.inner = buf.to_string_lossy().to_string();
        result
    }

    /// Returns a `VumaPath` view of this path buffer.
    // VUMA-VERIFIED: conversion is lossless
    pub fn as_path(&self) -> VumaPath {
        VumaPath {
            inner: self.inner.clone(),
        }
    }

    /// Convert this path buffer into a `String`.
    // VUMA-VERIFIED: conversion is lossless
    pub fn into_string(self) -> String {
        self.inner
    }

    /// Returns the CapD for this path buffer.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
            CapFlag::Compare,
            CapFlag::Serialize,
        ])
    }

    /// Returns the RepD for this path buffer.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("VumaPathBuf", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this path buffer.
    // VUMA-VERIFIED: path buffer is a passive value type
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl Default for VumaPathBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for VumaPathBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<VumaPath> for VumaPathBuf {
    fn from(p: VumaPath) -> Self {
        VumaPathBuf { inner: p.inner }
    }
}

impl AsRef<std::path::Path> for VumaPathBuf {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(&self.inner)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vuma_path_new_and_as_str() {
        let p = VumaPath::new("/usr/local/bin");
        assert_eq!(p.as_str(), "/usr/local/bin");
    }

    #[test]
    fn test_vuma_path_parent() {
        let p = VumaPath::new("/usr/local/bin");
        let parent = p.parent().unwrap();
        assert_eq!(parent.as_str(), "/usr/local");

        let root = VumaPath::new("/");
        assert!(root.parent().is_none());
    }

    #[test]
    fn test_vuma_path_file_name_and_extension() {
        let p = VumaPath::new("/home/user/file.tar.gz");
        assert_eq!(p.file_name(), Some("file.tar.gz"));
        assert_eq!(p.extension(), Some("gz"));
        assert_eq!(p.file_stem(), Some("file.tar"));
    }

    #[test]
    fn test_vuma_path_join() {
        let p = VumaPath::new("/usr/local");
        let other = VumaPath::new("bin/app");
        let joined = p.join(&other);
        assert_eq!(joined.as_path().as_str(), "/usr/local/bin/app");
    }

    #[test]
    fn test_vuma_path_with_extension() {
        let p = VumaPath::new("data.csv");
        let new_p = p.with_extension("json");
        assert_eq!(new_p.as_path().as_str(), "data.json");
    }

    #[test]
    fn test_vuma_path_with_file_name() {
        let p = VumaPath::new("/home/user/old.txt");
        let new_p = p.with_file_name("new.txt");
        assert_eq!(new_p.as_path().as_str(), "/home/user/new.txt");
    }

    #[test]
    fn test_vuma_path_is_absolute_relative() {
        let abs = VumaPath::new("/usr/bin");
        let rel = VumaPath::new("src/main.rs");
        assert!(abs.is_absolute());
        assert!(!abs.is_relative());
        assert!(!rel.is_absolute());
        assert!(rel.is_relative());
    }

    #[test]
    fn test_vuma_path_has_root() {
        assert!(VumaPath::new("/usr").has_root());
        assert!(!VumaPath::new("usr").has_root());
    }

    #[test]
    fn test_vuma_path_starts_with_ends_with() {
        let p = VumaPath::new("/usr/local/bin");
        let prefix = VumaPath::new("/usr/local");
        let suffix = VumaPath::new("bin");
        assert!(p.starts_with(&prefix));
        assert!(p.ends_with(&suffix));
        assert!(!p.starts_with(&VumaPath::new("/opt")));
    }

    #[test]
    fn test_vuma_path_components() {
        let p = VumaPath::new("/usr/local/bin");
        let comps = p.components();
        assert!(matches!(comps[0], PathComponent::RootDir));
        assert!(matches!(&comps[1], PathComponent::Normal(s) if s == "usr"));
        assert!(matches!(&comps[2], PathComponent::Normal(s) if s == "local"));
        assert!(matches!(&comps[3], PathComponent::Normal(s) if s == "bin"));
    }

    #[test]
    fn test_vuma_path_buf_push_pop() {
        let mut pb = VumaPathBuf::new();
        pb.push("/usr");
        pb.push("local");
        pb.push("bin");
        assert_eq!(pb.as_path().as_str(), "/usr/local/bin");

        assert!(pb.pop());
        assert_eq!(pb.as_path().as_str(), "/usr/local");
    }

    #[test]
    fn test_vuma_path_buf_from_and_into() {
        let pb = VumaPathBuf::from("/tmp/test");
        assert_eq!(pb.as_path().as_str(), "/tmp/test");
        let s: String = pb.into_string();
        assert_eq!(s, "/tmp/test");
    }

    #[test]
    fn test_path_component_display() {
        assert_eq!(PathComponent::RootDir.to_string(), "/");
        assert_eq!(PathComponent::CurDir.to_string(), ".");
        assert_eq!(PathComponent::ParentDir.to_string(), "..");
        assert_eq!(PathComponent::Normal("foo".to_string()).to_string(), "foo");
    }
}
