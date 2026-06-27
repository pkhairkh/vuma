//! Module resolver for the VUMA language frontend.
//!
//! Handles import resolution, circular-import detection, and namespace merging.
//!
//! # Overview
//!
//! When the compiler encounters an `import "path"` statement, the resolver:
//! 1. Resolves the file path relative to the importing file's directory.
//! 2. Reads and parses the imported file.
//! 3. Recursively resolves any imports in the imported file.
//! 4. Merges the imported functions/structs/enums into the importing module's
//!    namespace.
//! 5. Detects and reports circular imports.
//!
//! # Import syntax
//!
//! ```text
//! import "crypto.vuma"              // Import all items from file
//! import "crypto.vuma"::{sha256}    // Import specific items only
//! import "crypto.vuma" {sha256}     // Legacy form (also accepted)
//! ```
//!
//! # Name conflicts
//!
//! If two imported modules export a function with the same name, an error is
//! reported.  The user must use selective imports (`::{name}`) to disambiguate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{Import, Item, Program};
use crate::error::{ParseError, ParseErrorKind, Span};
use crate::parser::Parser;

// ---------------------------------------------------------------------------
// ResolveError
// ---------------------------------------------------------------------------

/// Errors that can occur during module resolution.
#[derive(Debug, Clone)]
pub enum ResolveError {
    /// The imported file was not found on disk.
    FileNotFound {
        /// Import path as written in the source.
        path: String,
        /// Resolved absolute path that was searched.
        resolved: PathBuf,
    },
    /// A circular import was detected.
    CircularImport {
        /// Import path as written in the source.
        path: String,
        /// Chain of files that form the cycle.
        cycle: Vec<String>,
    },
    /// A name conflict: the same symbol is available from multiple imports.
    NameConflict {
        /// The conflicting name.
        name: String,
        /// The files that export this name.
        sources: Vec<String>,
    },
    /// An import specified a symbol name that does not exist in the imported
    /// file.
    SymbolNotFound {
        /// The symbol that was requested.
        symbol: String,
        /// The file that was imported.
        path: String,
        /// Symbols that are available in the file.
        available: Vec<String>,
    },
    /// A parse error occurred in an imported file.
    Parse {
        /// The file that failed to parse.
        path: String,
        /// The parse errors.
        errors: Vec<ParseError>,
    },
    /// An I/O error occurred while reading a file.
    Io {
        /// The file that could not be read.
        path: String,
        /// The I/O error message.
        message: String,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::FileNotFound { path, resolved } => {
                write!(
                    f,
                    "import file '{}' not found (resolved to '{}')",
                    path,
                    resolved.display()
                )
            }
            ResolveError::CircularImport { path, cycle } => {
                write!(
                    f,
                    "circular import detected for '{}' (cycle: {})",
                    path,
                    cycle.join(" -> ")
                )
            }
            ResolveError::NameConflict { name, sources } => {
                write!(
                    f,
                    "name conflict: '{}' is exported by multiple files: {}",
                    name,
                    sources.join(", ")
                )
            }
            ResolveError::SymbolNotFound {
                symbol,
                path,
                available,
            } => {
                write!(
                    f,
                    "symbol '{}' not found in '{}' (available: {})",
                    symbol,
                    path,
                    available.join(", ")
                )
            }
            ResolveError::Parse { path, errors } => {
                write!(f, "parse error in imported file '{}': ", path)?;
                for (i, err) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{}", err.message)?;
                }
                Ok(())
            }
            ResolveError::Io { path, message } => {
                write!(f, "cannot read imported file '{}': {}", path, message)
            }
        }
    }
}

impl std::error::Error for ResolveError {}

// ---------------------------------------------------------------------------
// ModuleResolver
// ---------------------------------------------------------------------------

/// Resolves imports and merges modules into a single program.
///
/// Usage:
/// ```ignore
/// use vuma_parser::resolver::ModuleResolver;
///
/// let mut resolver = ModuleResolver::new();
/// let resolved = resolver.resolve_file("src/main.vuma")?;
/// ```
pub struct ModuleResolver {
    /// Cache of already-parsed programs keyed by canonical file path.
    /// Prevents re-parsing the same file and enables circular-import
    /// detection via the `in_progress` set.
    cache: HashMap<PathBuf, Program>,

    /// Set of file paths currently being resolved (on the call stack).
    /// If we encounter an import whose canonical path is already in this
    /// set, we have a circular import.
    in_progress: HashSet<PathBuf>,
}

impl ModuleResolver {
    /// Create a new module resolver with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    /// Resolve a single source file and all its imports.
    ///
    /// Returns a merged [`Program`] containing:
    /// - All items from the root file.
    /// - All items from transitively imported files (excluding `import`
    ///   statements themselves, which are consumed during resolution).
    /// - Duplicate `import` and `export` items are removed from the final
    ///   program.
    ///
    /// The `file_path` should be the path to the root `.vuma` source file.
    pub fn resolve_file(&mut self, file_path: &Path) -> Result<Program, Vec<ResolveError>> {
        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.to_path_buf());

        let mut errors = Vec::new();
        let program = self.resolve_recursive(&canonical, &mut errors);

        match program {
            Some(p) if errors.is_empty() => Ok(p),
            Some(_) => Err(errors),
            None => Err(errors),
        }
    }

    /// Resolve a source string given an optional base directory for imports.
    ///
    /// This is the main entry point for the compilation pipeline, which
    /// typically has a source string and an optional file path.
    ///
    /// If `base_dir` is `None`, imports will be resolved relative to the
    /// current working directory.
    pub fn resolve_source(
        &mut self,
        source: &str,
        base_path: Option<&Path>,
    ) -> Result<Program, Vec<ResolveError>> {
        let mut errors = Vec::new();

        // Parse the root source.
        let program = match Self::parse_source(source) {
            Ok(p) => p,
            Err(e) => {
                errors.push(ResolveError::Parse {
                    path: base_path
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<source>".to_string()),
                    errors: vec![e],
                });
                return Err(errors);
            }
        };

        // If there are no imports, just return the program as-is.
        let has_imports = program.items.iter().any(|i| matches!(i, Item::Import(_)));
        if !has_imports {
            return Ok(program);
        }

        // Determine the base directory for resolving import paths.
        let base_dir = base_path
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        // Use the base_path as a pseudo-canonical path for cycle detection.
        let canonical = base_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("<source>"));

        self.in_progress.insert(canonical.clone());

        // Resolve imports and merge.
        let merged = self.merge_imports(program, &base_dir, &mut errors);

        self.in_progress.remove(&canonical);

        if errors.is_empty() {
            Ok(merged)
        } else {
            Err(errors)
        }
    }

    // ── Internal helpers ────────────────────────────────────────────────

    /// Recursively resolve a file: parse it, resolve its imports, and cache
    /// the result.
    fn resolve_recursive(
        &mut self,
        file_path: &Path,
        errors: &mut Vec<ResolveError>,
    ) -> Option<Program> {
        // Check cache first.
        if let Some(cached) = self.cache.get(file_path).cloned() {
            return Some(cached);
        }

        // Circular import detection.
        if self.in_progress.contains(file_path) {
            errors.push(ResolveError::CircularImport {
                path: file_path.display().to_string(),
                cycle: self
                    .in_progress
                    .iter()
                    .map(|p| p.display().to_string())
                    .chain(std::iter::once(file_path.display().to_string()))
                    .collect(),
            });
            return None;
        }

        self.in_progress.insert(file_path.to_path_buf());

        // Read the file.
        let source = match std::fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    errors.push(ResolveError::FileNotFound {
                        path: file_path.display().to_string(),
                        resolved: file_path.to_path_buf(),
                    });
                } else {
                    errors.push(ResolveError::Io {
                        path: file_path.display().to_string(),
                        message: e.to_string(),
                    });
                }
                self.in_progress.remove(file_path);
                return None;
            }
        };

        // Parse the file.
        let program = match Self::parse_source(&source) {
            Ok(p) => p,
            Err(e) => {
                errors.push(ResolveError::Parse {
                    path: file_path.display().to_string(),
                    errors: vec![e],
                });
                self.in_progress.remove(file_path);
                return None;
            }
        };

        // Resolve imports in this file.
        let base_dir = file_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let merged = self.merge_imports(program, &base_dir, errors);

        self.in_progress.remove(file_path);

        // Cache the result.
        self.cache.insert(file_path.to_path_buf(), merged.clone());

        Some(merged)
    }

    /// Merge imported items into a program.
    ///
    /// This processes all `import` statements in the program, resolves them,
    /// and adds the imported items. The `import` statements themselves are
    /// removed from the resulting program.
    fn merge_imports(
        &mut self,
        program: Program,
        base_dir: &Path,
        errors: &mut Vec<ResolveError>,
    ) -> Program {
        // Collect imports from the program.
        let imports: Vec<&Import> = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Import(i) => Some(i),
                _ => None,
            })
            .collect();

        // If no imports, return as-is.
        if imports.is_empty() {
            return program;
        }

        // Build a map of imported items: name -> (source_file, Item).
        // We track conflicts.
        let mut imported_items: HashMap<String, (String, Item)> = HashMap::new();
        let mut seen_names: HashMap<String, Vec<String>> = HashMap::new();

        for import in &imports {
            // Resolve the import path relative to the base directory.
            let resolved_path = self.resolve_import_path(import.path.as_str(), base_dir);

            // Try to parse and resolve the imported file.
            let imported_program = match self.resolve_recursive(&resolved_path, errors) {
                Some(p) => p,
                None => continue,
            };

            // Collect items from the imported program (excluding imports and
            // exports — only real definitions).
            let available_names = self.collect_exportable_names(&imported_program);

            // If specific symbols were requested, validate they exist.
            if !import.symbols.is_empty() {
                for sym in &import.symbols {
                    if !available_names.contains(sym) {
                        errors.push(ResolveError::SymbolNotFound {
                            symbol: sym.clone(),
                            path: import.path.clone(),
                            available: available_names.clone(),
                        });
                    }
                }
            }

            // Add items from the imported program.
            for item in imported_program.items {
                // Skip import/export items — they are namespace directives,
                // not definitions.
                if matches!(item, Item::Import(_) | Item::Export(_)) {
                    continue;
                }

                let name = match item_name(&item) {
                    Some(n) => n,
                    None => continue, // Skip items without names (top-level stmts).
                };

                // If the import specifies symbols, only include those.
                if !import.symbols.is_empty() && !import.symbols.contains(&name) {
                    continue;
                }

                // Track the source for conflict detection.
                seen_names
                    .entry(name.clone())
                    .or_default()
                    .push(import.path.clone());

                match imported_items.entry(name.clone()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert((import.path.clone(), item));
                    }
                    std::collections::hash_map::Entry::Occupied(e) => {
                        // Conflict: same name from different sources.
                        // Only report if the sources differ.
                        let (existing_source, _) = e.get();
                        if *existing_source != import.path {
                            // Keep the first one but report the conflict.
                            errors.push(ResolveError::NameConflict {
                                name: name.clone(),
                                sources: seen_names[&name].clone(),
                            });
                        }
                    }
                }
            }
        }

        // Build the merged program: start with non-import items from the
        // original program, then add imported items.
        let mut merged_items: Vec<Item> = program
            .items
            .into_iter()
            .filter(|item| !matches!(item, Item::Import(_)))
            .collect();

        for (_name, (_source, item)) in imported_items {
            // Skip items that are already defined in the main file.
            // The main file's definitions take precedence.
            let i_name = item_name(&item);
            if let Some(name) = &i_name {
                if merged_items.iter().any(|existing| {
                    item_name(existing).as_ref() == Some(name)
                }) {
                    continue;
                }
            }
            merged_items.push(item);
        }

        Program {
            items: merged_items,
            span: program.span,
        }
    }

    /// Resolve an import path relative to a base directory.
    fn resolve_import_path(&self, import_path: &str, base_dir: &Path) -> PathBuf {
        let path = Path::new(import_path);

        // If the path is already absolute, use it as-is.
        if path.is_absolute() {
            return path.to_path_buf();
        }

        // Otherwise, resolve relative to the base directory.
        base_dir.join(path)
    }

    /// Collect the names of all exportable items in a program.
    fn collect_exportable_names(&self, program: &Program) -> Vec<String> {
        program
            .items
            .iter()
            .filter_map(|item| {
                if matches!(item, Item::Import(_) | Item::Export(_)) {
                    return None;
                }
                item_name(item)
            })
            .collect()
    }

    /// Parse a source string into a Program.
    fn parse_source(source: &str) -> Result<Program, ParseError> {
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        if result.is_err() {
            // Return the first fatal error.
            return Err(result.errors.into_iter().next().unwrap_or_else(|| {
                ParseError::new(
                    "parse failed",
                    Span::new(0, 0),
                    ParseErrorKind::UnexpectedToken,
                )
            }));
        }
        if result.has_errors() {
            // Non-fatal errors — still return the program but log warnings.
            // For the resolver, we'll proceed with the partial program.
        }
        Ok(result.unwrap())
    }
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the name of a named item (function, struct, enum, etc.).
fn item_name(item: &Item) -> Option<String> {
    match item {
        Item::FnDef(f) => Some(f.name.clone()),
        Item::StructDef(s) => Some(s.name.clone()),
        Item::EnumDef(e) => Some(e.name.clone()),
        Item::RegionDef(r) => Some(r.name.clone()),
        Item::Const(c) => Some(c.name.clone()),
        Item::Static(s) => Some(s.name.clone()),
        Item::ModuleDef(m) => Some(m.name.clone()),
        Item::TraitDef(t) => Some(t.name.clone()),
        Item::ImplBlock(_) => None, // impl blocks are not named items
        Item::Import(_) => None,     // imports are not definitions
        Item::Export(_) => None,     // exports are not definitions
        Item::Stmt(_) => None,       // top-level statements are not named
        Item::ExternBlock(_) => None, // extern blocks are not named
        Item::ConceptDecl(c) => Some(c.name.clone()),
        Item::GestaltDecl(g) => Some(g.name.clone()),
        Item::ManifoldDecl(m) => Some(m.name.clone()),
        Item::AuraDecl(a) => Some(a.name.clone()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_import_path_relative() {
        let resolver = ModuleResolver::new();
        let base_dir = Path::new("/home/user/project/src");
        let resolved = resolver.resolve_import_path("crypto.vuma", base_dir);
        assert_eq!(
            resolved,
            PathBuf::from("/home/user/project/src/crypto.vuma")
        );
    }

    #[test]
    fn test_resolve_import_path_absolute() {
        let resolver = ModuleResolver::new();
        let base_dir = Path::new("/home/user/project/src");
        let resolved = resolver.resolve_import_path("/abs/crypto.vuma", base_dir);
        assert_eq!(resolved, PathBuf::from("/abs/crypto.vuma"));
    }

    #[test]
    fn test_item_name() {
        let source = "fn hello() {}";
        let mut parser = Parser::new(source);
        let program = parser.parse_program().unwrap();
        let name = item_name(&program.items[0]);
        assert_eq!(name, Some("hello".to_string()));
    }

    #[test]
    fn test_no_imports_returns_unchanged() {
        let source = "fn main() {}";
        let mut resolver = ModuleResolver::new();
        let result = resolver.resolve_source(source, None);
        assert!(result.is_ok());
        let program = result.unwrap();
        assert_eq!(program.items.len(), 1);
    }

    #[test]
    fn test_import_with_double_colon_syntax() {
        let source = r#"import "crypto.vuma"::{sha256, sha256d};"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().unwrap();
        match &program.items[0] {
            Item::Import(i) => {
                assert_eq!(i.path, "crypto.vuma");
                assert_eq!(i.symbols, vec!["sha256", "sha256d"]);
            }
            other => panic!("expected Import, got {:?}", other),
        }
    }

    #[test]
    fn test_import_without_symbols() {
        let source = r#"import "crypto.vuma";"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().unwrap();
        match &program.items[0] {
            Item::Import(i) => {
                assert_eq!(i.path, "crypto.vuma");
                assert!(i.symbols.is_empty());
            }
            other => panic!("expected Import, got {:?}", other),
        }
    }

    #[test]
    fn test_import_legacy_brace_syntax() {
        let source = r#"import "crypto.vuma" {sha256};"#;
        let mut parser = Parser::new(source);
        let program = parser.parse_program().unwrap();
        match &program.items[0] {
            Item::Import(i) => {
                assert_eq!(i.path, "crypto.vuma");
                assert_eq!(i.symbols, vec!["sha256"]);
            }
            other => panic!("expected Import, got {:?}", other),
        }
    }

    #[test]
    fn test_import_file_not_found() {
        let source = r#"import "nonexistent.vuma";"#;
        let mut resolver = ModuleResolver::new();
        let result = resolver.resolve_source(source, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| matches!(e, ResolveError::FileNotFound { .. })));
    }
}
