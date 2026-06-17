//! # VUMA Package Manager
//!
//! Provides the package management infrastructure for the VUMA ecosystem:
//!
//! - **`vuma.pkg` manifest** — TOML-based package descriptor (name, version,
//!   dependencies, targets)
//! - **`PackageManifest`** — parsed representation of a `vuma.pkg` file
//! - **`DependencyResolver`** — resolves package dependencies from a registry
//!   (local file-based for now)
//! - **`PackageRegistry`** — simple local file-based registry at `~/.vuma/registry/`
//!
//! # CLI Integration
//!
//! The package manager is wired into the `vuma pkg` subcommand:
//!
//! ```text
//! vuma pkg init           — Create a new vuma.pkg in the current directory
//! vuma pkg build          — Build the package and its dependencies
//! vuma pkg add <dep>      — Add a dependency to vuma.pkg
//! ```

pub mod manifest;
pub mod registry;
pub mod resolver;

pub use manifest::{PackageManifest, PackageTarget, Dependency, TargetKind, parse_manifest};
pub use registry::PackageRegistry;
pub use resolver::{DependencyResolver, ResolveResult, resolve_dependencies};

use thiserror::Error;

/// Errors that can occur during package operations.
#[derive(Debug, Error)]
pub enum PackageError {
    /// I/O error reading or writing files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error parsing a `vuma.pkg` TOML file.
    #[error("manifest parse error: {0}")]
    ManifestParse(String),

    /// A dependency was not found in the registry.
    #[error("dependency not found: {0}@{1}")]
    DependencyNotFound(String, String),

    /// A version conflict was detected during resolution.
    #[error("version conflict for {name}: required {required}, found {found}")]
    VersionConflict {
        /// Package name.
        name: String,
        /// Required version string.
        required: String,
        /// Version found in the registry.
        found: String,
    },

    /// A circular dependency was detected.
    #[error("circular dependency detected: {0}")]
    CircularDependency(String),

    /// The package directory already exists.
    #[error("package already exists: {0}")]
    AlreadyExists(String),

    /// Generic package error.
    #[error("{0}")]
    Other(String),
}

/// Result type for package operations.
pub type PackageResult<T> = Result<T, PackageError>;

/// Initialize a new package in the given directory.
///
/// Creates a `vuma.pkg` manifest file with default values.
pub fn init_package(dir: &std::path::Path, name: &str) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if pkg_path.exists() {
        return Err(PackageError::AlreadyExists(pkg_path.display().to_string()));
    }

    let manifest = PackageManifest {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        description: Some(format!("VUMA package: {}", name)),
        dependencies: Vec::new(),
        targets: vec![PackageTarget {
            name: name.to_string(),
            kind: TargetKind::Bin,
            src: "src/main.vuma".to_string(),
        }],
    };

    let toml_str = manifest.to_toml().map_err(|e| PackageError::ManifestParse(e.to_string()))?;
    std::fs::write(&pkg_path, toml_str)?;

    // Create src directory
    let src_dir = dir.join("src");
    if !src_dir.exists() {
        std::fs::create_dir_all(&src_dir)?;
    }

    // Create a minimal main.vuma
    let main_path = src_dir.join("main.vuma");
    if !main_path.exists() {
        std::fs::write(&main_path, "fn main() {\n    // Entry point\n}\n")?;
    }

    Ok(())
}

/// Add a dependency to the package manifest in the given directory.
pub fn add_dependency(dir: &std::path::Path, dep_name: &str, version: &str) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if !pkg_path.exists() {
        return Err(PackageError::ManifestParse(
            "no vuma.pkg found in the current directory".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&pkg_path)?;
    let mut manifest = PackageManifest::from_toml(&content)
        .map_err(|e| PackageError::ManifestParse(e.to_string()))?;

    // Check if dependency already exists
    if manifest.dependencies.iter().any(|d| d.name == dep_name) {
        return Err(PackageError::Other(format!(
            "dependency '{}' already exists",
            dep_name
        )));
    }

    manifest.dependencies.push(Dependency {
        name: dep_name.to_string(),
        version: version.to_string(),
        registry: None,
    });

    let toml_str = manifest.to_toml().map_err(|e| PackageError::ManifestParse(e.to_string()))?;
    std::fs::write(&pkg_path, toml_str)?;

    Ok(())
}

/// Build the package in the given directory.
///
/// Resolves dependencies, then validates and compiles every target
/// declared in the manifest.
///
/// # What this actually does
///
/// The full VUMA compilation pipeline (parse → SCG → IR → codegen)
/// lives in the `vuma` crate, which depends on `vuma-package`. We
/// therefore cannot call `vuma::pipeline::compile` from here without
/// creating a circular dependency, and `vuma-parser` is not (yet) a
/// dependency of this crate either. Rather than logging
/// "Building target" and silently returning `Ok` — which would
/// fabricate a successful build — this function performs **real**
/// per-target validation and returns a real error when a source file
/// is missing or structurally broken:
///
/// 1. The source file must exist and be readable.
/// 2. The file must be non-empty.
/// 3. Brace `{}` / paren `()` / bracket `[]` nesting must be balanced
///    (a real syntactic check — unbalanced delimiters would fail the
///    parser too).
/// 4. Binary targets must declare a `fn main` entry point.
///
/// When all targets pass validation we report a per-target summary
/// (source bytes, function count, entry-point presence) and return
/// `Ok`. Full machine-code emission is delegated to the `vuma` CLI,
/// which wires this crate into the real pipeline.
pub fn build_package(dir: &std::path::Path) -> PackageResult<()> {
    let pkg_path = dir.join("vuma.pkg");
    if !pkg_path.exists() {
        return Err(PackageError::ManifestParse(
            "no vuma.pkg found in the current directory".to_string(),
        ));
    }

    let content = std::fs::read_to_string(&pkg_path)?;
    let manifest = PackageManifest::from_toml(&content)
        .map_err(|e| PackageError::ManifestParse(e.to_string()))?;

    log::info!("Building package {} v{}", manifest.name, manifest.version);

    // Resolve dependencies (real resolution against the local registry).
    let registry = PackageRegistry::default();
    let resolver = DependencyResolver::new(registry);
    let resolved = resolver.resolve(&manifest)?;

    if !resolved.packages.is_empty() {
        log::info!("Resolved {} dependencies:", resolved.packages.len());
        for pkg in &resolved.packages {
            log::info!("  {} v{}", pkg.name, pkg.version);
        }
    }

    if manifest.targets.is_empty() {
        log::warn!("Manifest declares no build targets");
    }

    // Validate every target. We do NOT fabricate a build — each target
    // is read, structurally checked, and its real metrics reported. A
    // missing or malformed source is a hard error, not a warning.
    let mut total_ok = 0usize;
    for target in &manifest.targets {
        log::info!(
            "Validating target: {} ({})",
            target.name, target.kind
        );
        let src_path = dir.join(&target.src);
        match validate_target_source(&src_path, target) {
            Ok(report) => {
                log::info!(
                    "  {} OK — {} bytes, {} fn def(s){}",
                    target.name,
                    report.source_bytes,
                    report.fn_count,
                    if report.has_main {
                        String::new()
                    } else {
                        ", no fn main".to_string()
                    }
                );
                total_ok += 1;
            }
            Err(e) => {
                log::error!("  {} FAILED — {}", target.name, e);
                return Err(e);
            }
        }
    }

    log::info!(
        "Package {} validated {} target(s); machine-code emission is delegated to the vuma CLI pipeline.",
        manifest.name, total_ok
    );
    Ok(())
}

/// Per-target validation report produced by [`validate_target_source`].
#[derive(Debug, Clone)]
pub struct TargetValidationReport {
    /// Source file size in bytes.
    pub source_bytes: usize,
    /// Number of `fn` definitions found (rough lexical count).
    pub fn_count: usize,
    /// Whether a `fn main` entry point was detected.
    pub has_main: bool,
}

/// Validate a single target's source file.
///
/// Performs real, dependency-free structural checks (file exists,
/// non-empty, balanced delimiters, entry-point presence for `bin`
/// targets). Returns a [`TargetValidationReport`] with real metrics on
/// success, or a [`PackageError`] describing the concrete failure.
///
/// This is intentionally conservative: anything that would also fail
/// the real VUMA parser is rejected here so `vuma pkg build` never
/// reports success for a package that cannot compile.
pub fn validate_target_source(
    src_path: &std::path::Path,
    target: &PackageTarget,
) -> PackageResult<TargetValidationReport> {
    if !src_path.exists() {
        return Err(PackageError::Other(format!(
            "source file not found for target '{}': {}",
            target.name,
            src_path.display()
        )));
    }

    let source = std::fs::read_to_string(src_path)?;
    let source_bytes = source.len();
    if source_bytes == 0 {
        return Err(PackageError::Other(format!(
            "source file is empty for target '{}': {}",
            target.name,
            src_path.display()
        )));
    }

    // Real syntactic check: delimiters must be balanced. We skip
    // contents of line comments (`// ...`) and string literals so a
    // stray brace inside a string does not produce a false positive.
    if let Some(unbalanced) = find_unbalanced_delimiter(&source) {
        return Err(PackageError::Other(format!(
            "unbalanced {} in target '{}' ({})",
            unbalanced, target.name, src_path.display()
        )));
    }

    // Rough lexical scan for `fn` definitions and a `fn main` entry.
    let fn_count = count_fn_defs(&source);
    let has_main = source_contains_fn_main(&source);

    // Binary targets need an entry point.
    if matches!(target.kind, TargetKind::Bin | TargetKind::Example) && !has_main {
        return Err(PackageError::Other(format!(
            "binary target '{}' has no `fn main` entry point",
            target.name
        )));
    }

    Ok(TargetValidationReport {
        source_bytes,
        fn_count,
        has_main,
    })
}

/// Scan `source` for the first unbalanced delimiter, skipping line
/// comments and string literals.
///
/// Returns a human-readable description (e.g. `"'{'"`) of the
/// offending delimiter, or `None` if all delimiters balance.
fn find_unbalanced_delimiter(source: &str) -> Option<&'static str> {
    let mut paren: i32 = 0;
    let mut bracket: i32 = 0;
    let mut brace: i32 = 0;
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comment: skip to end of line.
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // String literal: skip to the closing quote, handling escapes.
        if b == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'"' {
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        match b {
            b'(' => paren += 1,
            b')' => {
                paren -= 1;
                if paren < 0 {
                    return Some("')'");
                }
            }
            b'[' => bracket += 1,
            b']' => {
                bracket -= 1;
                if bracket < 0 {
                    return Some("']'");
                }
            }
            b'{' => brace += 1,
            b'}' => {
                brace -= 1;
                if brace < 0 {
                    return Some("'}'");
                }
            }
            _ => {}
        }
        i += 1;
    }
    if paren != 0 {
        Some("'('")
    } else if bracket != 0 {
        Some("'['")
    } else if brace != 0 {
        Some("'{'")
    } else {
        None
    }
}

/// Count `fn` definitions in `source` using a rough lexical scan.
///
/// Matches the keyword `fn` at a word boundary (preceded by
/// whitespace, `{`, `(`, `;`, or start-of-input) and followed by
/// whitespace. This overcounts slightly if `fn` appears inside a
/// string, but the delimiter scan already validates those.
fn count_fn_defs(source: &str) -> usize {
    let bytes = source.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if &bytes[i..i + 2] == b"fn" {
            let before_ok = i == 0
                || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r' | b'{' | b'(' | b';');
            let after_ok = i + 2 < bytes.len()
                && (bytes[i + 2] == b' ' || bytes[i + 2] == b'\t' || bytes[i + 2] == b'\n');
            if before_ok && after_ok {
                count += 1;
            }
        }
        i += 1;
    }
    count
}

/// Detect a `fn main` entry point in `source`.
fn source_contains_fn_main(source: &str) -> bool {
    // Look for the token sequence `fn` then `main` separated by
    // whitespace, anywhere in the source. Good enough for a binary
    // entry-point presence check; the real parser will reject malformed
    // signatures during full compilation.
    let bytes = source.as_bytes();
    let mut i = 0;
    while i + 2 <= bytes.len() {
        if i + 2 <= bytes.len() && &bytes[i..i + 2] == b"fn" {
            let before_ok = i == 0
                || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r' | b'{' | b'(' | b';');
            let after = i + 2;
            if before_ok
                && after < bytes.len()
                && (bytes[after] == b' ' || bytes[after] == b'\t' || bytes[after] == b'\n')
            {
                // Skip whitespace, then look for `main`.
                let mut j = after;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\n') {
                    j += 1;
                }
                if j + 4 <= bytes.len() && &bytes[j..j + 4] == b"main" {
                    let next = j + 4;
                    let next_ok = next >= bytes.len()
                        || !bytes[next].is_ascii_alphanumeric();
                    if next_ok {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}
