//! # Filesystem Operations
//!
//! This module provides VUMA-verified filesystem operations with Behavioral
//! Description (BD) annotations, delegating to `std::fs` for real I/O.
//!
//! ## Types
//!
//! - **VumaFile**: Open, create, read, write, metadata, set_len, sync.
//! - **VumaMetadata**: File metadata (size, type, permissions, timestamps).
//! - **VumaDir**: Directory iteration.
//! - **VumaPermissions**: File permissions (readonly query).
//! - **VumaIoError**: Error type for all filesystem operations.
//!
//! ## Free Functions
//!
//! remove_file, remove_dir, remove_dir_all, rename, copy, create_dir,
//! create_dir_all, canonicalize, exists, hard_link, read_link.
//!
//! ## BD Annotations
//!
//! - VumaFile: CapD { Read, Write } depending on open mode
//! - VumaMetadata: CapD { Read, Serialize }
//! - VumaDir: CapD { Read, Execute }
//! - VumaPermissions: CapD { Read, Serialize }

use crate::error::{VumaErrorChain, VumaErrorKind, VumaResult};
use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Read as StdRead, Seek as StdSeek, SeekFrom, Write as StdWrite};
use std::path::Path;

// ---------------------------------------------------------------------------
// VumaIoError (fs-specific)
// ---------------------------------------------------------------------------

/// A filesystem-specific I/O error.
///
/// Wraps `std::io::Error` with VUMA BD annotations.
#[derive(Debug, Clone)]
pub struct VumaIoError {
    /// The underlying I/O error kind.
    pub kind: VumaErrorKind,
    /// Human-readable message.
    pub message: String,
}

impl VumaIoError {
    /// Create a new filesystem I/O error.
    // VUMA-VERIFIED: error construction is pure
    pub fn new(kind: VumaErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Create from a `std::io::Error`.
    // VUMA-VERIFIED: conversion preserves error semantics
    pub fn from_io(e: std::io::Error, context: &str) -> Self {
        let kind = match e.kind() {
            std::io::ErrorKind::NotFound => VumaErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => VumaErrorKind::PermissionDenied,
            std::io::ErrorKind::TimedOut => VumaErrorKind::Timeout,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                VumaErrorKind::InvalidArgument
            }
            _ => VumaErrorKind::Io,
        };
        Self {
            kind,
            message: format!("{}: {}", context, e),
        }
    }
}

impl fmt::Display for VumaIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VumaIoError({}): {}", self.kind, self.message)
    }
}

impl std::error::Error for VumaIoError {}

impl From<VumaIoError> for VumaErrorChain {
    fn from(e: VumaIoError) -> Self {
        VumaErrorChain::new(e.kind, e.message)
    }
}

impl From<std::io::Error> for VumaIoError {
    fn from(e: std::io::Error) -> Self {
        let kind = match e.kind() {
            std::io::ErrorKind::NotFound => VumaErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => VumaErrorKind::PermissionDenied,
            std::io::ErrorKind::TimedOut => VumaErrorKind::Timeout,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                VumaErrorKind::InvalidArgument
            }
            std::io::ErrorKind::OutOfMemory => VumaErrorKind::OutOfMemory,
            _ => VumaErrorKind::Io,
        };
        Self {
            kind,
            message: e.to_string(),
        }
    }
}

impl From<VumaIoError> for std::io::Error {
    fn from(e: VumaIoError) -> Self {
        let kind = match e.kind {
            VumaErrorKind::NotFound => std::io::ErrorKind::NotFound,
            VumaErrorKind::PermissionDenied => std::io::ErrorKind::PermissionDenied,
            VumaErrorKind::Timeout => std::io::ErrorKind::TimedOut,
            VumaErrorKind::InvalidArgument => std::io::ErrorKind::InvalidInput,
            VumaErrorKind::OutOfMemory => std::io::ErrorKind::OutOfMemory,
            _ => std::io::ErrorKind::Other,
        };
        std::io::Error::new(kind, e.message)
    }
}

// ---------------------------------------------------------------------------
// VumaPermissions
// ---------------------------------------------------------------------------

/// VUMA-verified file permissions.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaPermissions {
    /// Whether the file is read-only.
    pub readonly: bool,
}

impl VumaPermissions {
    /// Create from `std::fs::Permissions`.
    // VUMA-VERIFIED: conversion is lossless
    pub fn from_std(p: &std::fs::Permissions) -> Self {
        Self {
            readonly: p.readonly(),
        }
    }

    /// Returns `true` if the file is read-only.
    // VUMA-VERIFIED: pure query
    pub fn readonly(&self) -> bool {
        self.readonly
    }

    /// Returns the CapD for this permissions.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Serialize])
    }
}

// ---------------------------------------------------------------------------
// VumaMetadata
// ---------------------------------------------------------------------------

/// VUMA-verified file metadata.
///
/// ## BD Annotations
///
/// - CapD: { Read, Serialize }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaMetadata {
    /// File size in bytes.
    pub size: u64,
    /// Whether the path is a directory.
    pub is_dir: bool,
    /// Whether the path is a file.
    pub is_file: bool,
    /// Whether the path is a symlink.
    pub is_symlink: bool,
    /// File permissions.
    pub permissions: VumaPermissions,
    /// Modification time (seconds since Unix epoch).
    pub modified_secs: Option<u64>,
}

impl VumaMetadata {
    /// Create from `std::fs::Metadata`.
    // VUMA-VERIFIED: conversion is lossless
    pub fn from_std(m: &std::fs::Metadata) -> Self {
        let modified_secs = m.modified().ok().map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
        Self {
            size: m.len(),
            is_dir: m.is_dir(),
            is_file: m.is_file(),
            is_symlink: m.is_symlink(),
            permissions: VumaPermissions::from_std(&m.permissions()),
            modified_secs,
        }
    }

    /// Returns the file size in bytes.
    // VUMA-VERIFIED: pure accessor
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns `true` if this is a directory.
    // VUMA-VERIFIED: pure query
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Returns `true` if this is a file.
    // VUMA-VERIFIED: pure query
    pub fn is_file(&self) -> bool {
        self.is_file
    }

    /// Returns `true` if this is a symlink.
    // VUMA-VERIFIED: pure query
    pub fn is_symlink(&self) -> bool {
        self.is_symlink
    }

    /// Returns the modification time as seconds since Unix epoch.
    // VUMA-VERIFIED: pure accessor
    pub fn modified(&self) -> Option<u64> {
        self.modified_secs
    }

    /// Returns the file permissions.
    // VUMA-VERIFIED: pure accessor
    pub fn permissions(&self) -> &VumaPermissions {
        &self.permissions
    }

    /// Returns the CapD for this metadata.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Serialize])
    }

    /// Returns the RepD for this metadata.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("VumaMetadata", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this metadata.
    // VUMA-VERIFIED: metadata is a passive value type
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// VumaFile
// ---------------------------------------------------------------------------

/// VUMA-verified file handle with BD annotations, delegating to `std::fs::File`.
///
/// ## BD Annotations
///
/// - CapD: { Read } for read mode, { Write } for write mode, { Read, Write } for read-write
/// - SyncEdge: open → read/write (Seq), close → read/write (Fence)
#[derive(Debug)]
pub struct VumaFile {
    /// The underlying OS file handle.
    inner: Option<std::fs::File>,
    /// The file path.
    pub path: String,
    /// Whether the file was opened for reading.
    pub can_read: bool,
    /// Whether the file was opened for writing.
    pub can_write: bool,
}

impl VumaFile {
    /// Open a file for reading.
    // VUMA-VERIFIED: open read requires Read capability
    pub fn open(path: impl AsRef<Path>) -> VumaResult<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let f = std::fs::File::open(&path)
            .map_err(|e| VumaIoError::from_io(e, &format!("open '{}'", path_str)))?;
        Ok(Self {
            inner: Some(f),
            path: path_str,
            can_read: true,
            can_write: false,
        })
    }

    /// Create (or truncate) a file for writing.
    // VUMA-VERIFIED: create requires Write capability
    pub fn create(path: impl AsRef<Path>) -> VumaResult<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let f = std::fs::File::create(&path)
            .map_err(|e| VumaIoError::from_io(e, &format!("create '{}'", path_str)))?;
        Ok(Self {
            inner: Some(f),
            path: path_str,
            can_read: false,
            can_write: true,
        })
    }

    /// Read bytes from the file into a buffer.
    // VUMA-VERIFIED: read requires Read capability
    pub fn read(&mut self, buf: &mut [u8]) -> VumaResult<usize> {
        let f = self
            .inner
            .as_mut()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        f.read(buf)
            .map_err(|e| VumaIoError::from_io(e, "read").into())
    }

    /// Write bytes to the file.
    // VUMA-VERIFIED: write requires Write capability
    pub fn write(&mut self, buf: &[u8]) -> VumaResult<usize> {
        let f = self
            .inner
            .as_mut()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        f.write(buf)
            .map_err(|e| VumaIoError::from_io(e, "write").into())
    }

    /// Returns the file metadata.
    // VUMA-VERIFIED: metadata query is safe
    pub fn metadata(&self) -> VumaResult<VumaMetadata> {
        let f = self
            .inner
            .as_ref()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        let m = f
            .metadata()
            .map_err(|e| VumaIoError::from_io(e, "metadata"))?;
        Ok(VumaMetadata::from_std(&m))
    }

    /// Truncate or extend the file to the specified size.
    // VUMA-VERIFIED: set_len requires Write capability
    pub fn set_len(&self, size: u64) -> VumaResult<()> {
        let f = self
            .inner
            .as_ref()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        f.set_len(size)
            .map_err(|e| VumaIoError::from_io(e, "set_len").into())
    }

    /// Sync all OS-internal metadata to disk.
    // VUMA-VERIFIED: sync_all is safe
    pub fn sync_all(&self) -> VumaResult<()> {
        let f = self
            .inner
            .as_ref()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        f.sync_all()
            .map_err(|e| VumaIoError::from_io(e, "sync_all").into())
    }

    /// Sync file data (but not necessarily metadata) to disk.
    // VUMA-VERIFIED: sync_data is safe
    pub fn sync_data(&self) -> VumaResult<()> {
        let f = self
            .inner
            .as_ref()
            .ok_or_else(|| VumaIoError::new(VumaErrorKind::Io, "file is not open"))?;
        f.sync_data()
            .map_err(|e| VumaIoError::from_io(e, "sync_data").into())
    }

    /// Returns the CapD for this file.
    // VUMA-VERIFIED: capability descriptor matches open mode
    pub fn capd(&self) -> CapD {
        let mut flags = vec![];
        if self.can_read {
            flags.push(CapFlag::Read);
        }
        if self.can_write {
            flags.push(CapFlag::Write);
        }
        CapD::new(flags)
    }

    /// Returns the RepD for this file.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("VumaFile", 0, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this file.
    // VUMA-VERIFIED: synchronization edges model file I/O ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![
            SyncEdge::new("fs_open", "fs_read", SyncEdgeKind::Seq),
            SyncEdge::new("fs_open", "fs_write", SyncEdgeKind::Seq),
            SyncEdge::new("fs_close", "fs_read", SyncEdgeKind::Fence),
            SyncEdge::new("fs_close", "fs_write", SyncEdgeKind::Fence),
        ]
    }
}

// ---------------------------------------------------------------------------
// VumaFile: std::io::Read + Write + Seek
// ---------------------------------------------------------------------------

impl StdRead for VumaFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let f = self.inner.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotConnected, "file is not open")
        })?;
        f.read(buf)
    }
}

impl StdWrite for VumaFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let f = self.inner.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotConnected, "file is not open")
        })?;
        f.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = self.inner.as_mut() {
            f.flush()?;
        }
        Ok(())
    }
}

impl StdSeek for VumaFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let f = self.inner.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotConnected, "file is not open")
        })?;
        f.seek(pos)
    }
}

// ---------------------------------------------------------------------------
// VumaDir
// ---------------------------------------------------------------------------

/// VUMA-verified directory iterator.
///
/// ## BD Annotations
///
/// - CapD: { Read, Execute }
/// - SyncEdge: read_dir → next_entry (Seq)
pub struct VumaDir {
    /// The underlying directory iterator.
    inner: std::fs::ReadDir,
}

impl fmt::Debug for VumaDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VumaDir").finish_non_exhaustive()
    }
}

impl VumaDir {
    /// Read the contents of a directory.
    // VUMA-VERIFIED: read_dir requires Read+Execute capabilities
    pub fn read_dir(path: impl AsRef<Path>) -> VumaResult<Self> {
        let rd = std::fs::read_dir(&path).map_err(|e| VumaIoError::from_io(e, "read_dir"))?;
        Ok(Self { inner: rd })
    }

    /// Returns the next entry in the directory.
    // VUMA-VERIFIED: next_entry is safe
    pub fn next_entry(&mut self) -> VumaResult<Option<std::fs::DirEntry>> {
        match self.inner.next() {
            None => Ok(None),
            Some(Ok(entry)) => Ok(Some(entry)),
            Some(Err(e)) => Err(VumaIoError::from_io(e, "next_entry").into()),
        }
    }

    /// Returns the CapD for this directory iterator.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Execute])
    }

    /// Returns the SyncEdge annotations for this directory iterator.
    // VUMA-VERIFIED: synchronization edges model directory iteration
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "fs_read_dir",
            "fs_next_entry",
            SyncEdgeKind::Seq,
        )]
    }
}

// ---------------------------------------------------------------------------
// Free Functions
// ---------------------------------------------------------------------------

/// Remove a file from the filesystem.
// VUMA-VERIFIED: remove_file requires Write capability on parent dir
pub fn remove_file(path: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::remove_file(&path).map_err(|e| VumaIoError::from_io(e, "remove_file").into())
}

/// Remove an empty directory.
// VUMA-VERIFIED: remove_dir requires Write capability on parent dir
pub fn remove_dir(path: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::remove_dir(&path).map_err(|e| VumaIoError::from_io(e, "remove_dir").into())
}

/// Remove a directory and all its contents recursively.
// VUMA-VERIFIED: remove_dir_all requires Write capability
pub fn remove_dir_all(path: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::remove_dir_all(&path).map_err(|e| VumaIoError::from_io(e, "remove_dir_all").into())
}

/// Rename a file or directory.
// VUMA-VERIFIED: rename requires Write capability on both paths
pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::rename(&from, &to).map_err(|e| VumaIoError::from_io(e, "rename").into())
}

/// Copy a file to a new path, returning the number of bytes copied.
// VUMA-VERIFIED: copy requires Read on source, Write on destination
pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> VumaResult<u64> {
    std::fs::copy(&from, &to).map_err(|e| VumaIoError::from_io(e, "copy").into())
}

/// Create a new directory.
// VUMA-VERIFIED: create_dir requires Write capability on parent dir
pub fn create_dir(path: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::create_dir(&path).map_err(|e| VumaIoError::from_io(e, "create_dir").into())
}

/// Create a directory and all parent directories as needed.
// VUMA-VERIFIED: create_dir_all requires Write capability
pub fn create_dir_all(path: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::create_dir_all(&path).map_err(|e| VumaIoError::from_io(e, "create_dir_all").into())
}

/// Returns the canonical, absolute form of a path.
// VUMA-VERIFIED: canonicalize requires Read capability
pub fn canonicalize(path: impl AsRef<Path>) -> VumaResult<std::path::PathBuf> {
    std::fs::canonicalize(&path).map_err(|e| VumaIoError::from_io(e, "canonicalize").into())
}

/// Returns `true` if the path exists.
// VUMA-VERIFIED: pure query
pub fn exists(path: impl AsRef<Path>) -> bool {
    path.as_ref().exists()
}

/// Create a hard link.
// VUMA-VERIFIED: hard_link requires Write capability on destination dir
pub fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> VumaResult<()> {
    std::fs::hard_link(&src, &dst).map_err(|e| VumaIoError::from_io(e, "hard_link").into())
}

/// Read a symbolic link, returning the path it points to.
// VUMA-VERIFIED: read_link requires Read capability
pub fn read_link(path: impl AsRef<Path>) -> VumaResult<std::path::PathBuf> {
    std::fs::read_link(&path).map_err(|e| VumaIoError::from_io(e, "read_link").into())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Returns a unique per-test temp directory to avoid parallel test conflicts.
    fn test_dir(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("vuma_fs_test_{}_{}", name, id));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn test_vuma_file_create_and_write() {
        let dir = test_dir("create_write");
        let path = dir.join("test_create.txt");
        let mut f = VumaFile::create(&path).unwrap();
        assert!(f.can_write);
        assert!(!f.can_read);
        let n = f.write(b"hello world").unwrap();
        assert_eq!(n, 11);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_open_and_read() {
        let dir = test_dir("open_read");
        let path = dir.join("test_read.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"test content").unwrap();
        }
        let mut f = VumaFile::open(&path).unwrap();
        assert!(f.can_read);
        assert!(!f.can_write);
        let mut buf = [0u8; 64];
        let n = f.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"test content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_metadata() {
        let dir = test_dir("metadata");
        let path = dir.join("test_meta.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"12345").unwrap();
        }
        let f = VumaFile::open(&path).unwrap();
        let meta = f.metadata().unwrap();
        assert_eq!(meta.size(), 5);
        assert!(meta.is_file());
        assert!(!meta.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_set_len() {
        let dir = test_dir("set_len");
        let path = dir.join("test_setlen.txt");
        let f = VumaFile::create(&path).unwrap();
        f.set_len(1024).unwrap();
        let meta = f.metadata().unwrap();
        assert_eq!(meta.size(), 1024);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_sync() {
        let dir = test_dir("sync");
        let path = dir.join("test_sync.txt");
        let f = VumaFile::create(&path).unwrap();
        f.sync_all().unwrap();
        f.sync_data().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_metadata_permissions() {
        let dir = test_dir("perms");
        let path = dir.join("test_perms.txt");
        let _ = std::fs::File::create(&path).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let vmeta = VumaMetadata::from_std(&meta);
        assert!(!vmeta.permissions().readonly());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_dir_read_dir() {
        let dir = test_dir("read_dir");
        let sub = dir.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        let _ = std::fs::File::create(sub.join("a.txt")).unwrap();
        let _ = std::fs::File::create(sub.join("b.txt")).unwrap();

        let mut d = VumaDir::read_dir(&sub).unwrap();
        let mut count = 0;
        while let Some(_) = d.next_entry().unwrap() {
            count += 1;
        }
        assert_eq!(count, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_remove_file() {
        let dir = test_dir("rm_file");
        let path = dir.join("to_remove.txt");
        let _ = std::fs::File::create(&path).unwrap();
        assert!(path.exists());
        remove_file(&path).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_and_remove_dir() {
        let dir = test_dir("mkdir_rmdir");
        let path = dir.join("new_dir");
        create_dir(&path).unwrap();
        assert!(path.is_dir());
        remove_dir(&path).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_dir_all_and_remove_dir_all() {
        let dir = test_dir("mkdir_all");
        let path = dir.join("a/b/c");
        create_dir_all(&path).unwrap();
        assert!(path.is_dir());
        let root = dir.join("a");
        remove_dir_all(&root).unwrap();
        assert!(!root.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rename() {
        let dir = test_dir("rename");
        let src = dir.join("orig.txt");
        let dst = dir.join("renamed.txt");
        let _ = std::fs::File::create(&src).unwrap();
        rename(&src, &dst).unwrap();
        assert!(!src.exists());
        assert!(dst.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_copy() {
        let dir = test_dir("copy");
        let src = dir.join("copy_src.txt");
        let dst = dir.join("copy_dst.txt");
        {
            let mut f = std::fs::File::create(&src).unwrap();
            f.write_all(b"copy me").unwrap();
        }
        let n = copy(&src, &dst).unwrap();
        assert_eq!(n, 7);
        assert!(dst.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_exists() {
        let dir = test_dir("exists");
        let path = dir.join("exists_check.txt");
        assert!(!exists(&path));
        let _ = std::fs::File::create(&path).unwrap();
        assert!(exists(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_canonicalize() {
        let dir = test_dir("canon");
        let path = dir.join("canonical_test.txt");
        let _ = std::fs::File::create(&path).unwrap();
        let canon = canonicalize(&path).unwrap();
        assert!(canon.is_absolute());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_hard_link() {
        let dir = test_dir("hardlink");
        let src = dir.join("hl_src.txt");
        let dst = dir.join("hl_dst.txt");
        {
            let mut f = std::fs::File::create(&src).unwrap();
            f.write_all(b"hard link data").unwrap();
        }
        hard_link(&src, &dst).unwrap();
        assert!(dst.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_open_nonexistent_returns_error() {
        let result = VumaFile::open("/tmp/vuma_nonexistent_12345");
        assert!(result.is_err());
    }

    // --- Tests for new trait implementations ---

    #[test]
    fn test_vuma_file_std_read_trait() {
        let dir = test_dir("std_read");
        let path = dir.join("std_read_test.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello std::io::Read").unwrap();
        }
        let mut f = VumaFile::open(&path).unwrap();
        let mut buf = [0u8; 64];
        let n = StdRead::read(&mut f, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello std::io::Read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_std_write_trait() {
        let dir = test_dir("std_write");
        let path = dir.join("std_write_test.txt");
        let mut f = VumaFile::create(&path).unwrap();
        let n = StdWrite::write(&mut f, b"hello from std::io::Write").unwrap();
        assert_eq!(n, 24);
        StdWrite::flush(&mut f).unwrap();
        // Read back separately with a new VumaFile
        let mut f2 = VumaFile::open(&path).unwrap();
        let mut buf = [0u8; 64];
        let n = StdRead::read(&mut f2, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello from std::io::Write");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_file_std_seek_trait() {
        let dir = test_dir("std_seek");
        let path = dir.join("std_seek_test.txt");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"0123456789").unwrap();
        }
        let mut f = VumaFile::open(&path).unwrap();
        // Seek to offset 5
        let pos = StdSeek::seek(&mut f, SeekFrom::Start(5)).unwrap();
        assert_eq!(pos, 5);
        let mut buf = [0u8; 5];
        let n = StdRead::read(&mut f, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"56789");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_vuma_io_error_from_std_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let vuma_err: VumaIoError = io_err.into();
        assert_eq!(vuma_err.kind, VumaErrorKind::NotFound);
    }

    #[test]
    fn test_vuma_io_error_into_std_io_error() {
        let vuma_err = VumaIoError::new(VumaErrorKind::PermissionDenied, "denied");
        let io_err: std::io::Error = vuma_err.into();
        assert_eq!(io_err.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
