//! Import resolution.
//!
//! The parsers record *what* a file imports, not *which file* it ends up in.
//! This module turns module specifiers into file references so the dependency
//! graph can carry real edges.
//!
//! Resolution is per language and intentionally conservative: if a specifier
//! cannot be mapped to a file that was actually scanned, it is left unresolved
//! rather than guessed. External packages (`node_modules`, crates.io, Go
//! modules outside the tree) are therefore skipped by design.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::{DiscoveredFile, FileId, Import};

/// Extensions tried for TypeScript/JavaScript specifiers, in priority order.
const JS_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];

/// Lookup table from file path to `FileId`, with per-language resolution rules.
pub struct FileIndex {
    by_path: HashMap<PathBuf, FileId>,
    files: Vec<PathBuf>,
    root: PathBuf,
}

impl FileIndex {
    /// Build an index over the files selected for analysis.
    ///
    /// `FileId`s follow the order of `files`, so the caller can build graph
    /// nodes from the same slice and get matching indices.
    pub fn new(files: &[DiscoveredFile], root: &Path) -> Self {
        let root = normalize(root);
        let mut by_path = HashMap::with_capacity(files.len());
        let mut paths = Vec::with_capacity(files.len());

        for (index, file) in files.iter().enumerate() {
            let path = normalize(&file.path);
            by_path.insert(path.clone(), FileId(index));
            paths.push(path);
        }

        Self {
            by_path,
            files: paths,
            root,
        }
    }

    pub fn id_of(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(&normalize(path)).copied()
    }

    pub fn path_of(&self, id: FileId) -> Option<&Path> {
        self.files.get(id.0).map(|path| path.as_path())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve an import to the file it refers to, if that file was scanned.
    pub fn resolve(&self, import: &Import) -> Option<FileId> {
        let importer = normalize(&import.file);
        let from_dir = importer.parent()?;

        let target = match importer.extension().and_then(|ext| ext.to_str()) {
            Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => {
                self.resolve_js(&import.module, from_dir)
            }
            Some("py") => self.resolve_python(&import.module, from_dir, &importer),
            Some("go") => self.resolve_go(&import.module),
            Some("rs") => self.resolve_rust(&import.module, from_dir),
            _ => None,
        }?;

        self.by_path.get(&target).copied()
    }

    fn resolve_js(&self, specifier: &str, from_dir: &Path) -> Option<PathBuf> {
        // Bare specifiers are package imports; map them only if a workspace
        // file happens to exist at that path, which is rare but harmless.
        if !specifier.starts_with('.') {
            let candidate = normalize(&from_dir.join(specifier));
            return self.probe(&candidate);
        }

        let base = normalize(&from_dir.join(specifier));
        self.probe(&base)
            .or_else(|| self.probe_index(&base))
            // `./x.js` is how TypeScript ESM code refers to `./x.ts`.
            .or_else(|| {
                let stem = base.file_stem()?;
                let mut stripped = base.clone();
                stripped.set_file_name(stem);
                self.probe(&stripped)
            })
    }

    fn resolve_python(&self, module: &str, from_dir: &Path, importer: &Path) -> Option<PathBuf> {
        let dots = module.chars().take_while(|c| *c == '.').count();
        let rest = &module[dots..];

        let base = if dots == 0 {
            self.root.clone()
        } else {
            // One dot means "this package", so start at the file's directory and
            // walk up once for each extra dot.
            let mut base = from_dir.to_path_buf();
            for _ in 1..dots {
                base = base.parent()?.to_path_buf();
            }
            base
        };

        if rest.is_empty() {
            // `from . import x` refers to the package's `__init__` only when the
            // importer is a package module; otherwise there is no file target.
            return self.probe_python(&base.join("__init__"));
        }

        let mut path = base;
        for part in rest.split('.') {
            path.push(part);
        }

        // A relative import may name a sibling module rather than a child.
        self.probe_python(&path).or_else(|| {
            let parent = importer.parent()?;
            let mut sibling = parent.to_path_buf();
            for part in rest.split('.') {
                sibling.push(part);
            }
            self.probe_python(&sibling)
        })
    }

    fn resolve_go(&self, specifier: &str) -> Option<PathBuf> {
        if specifier.starts_with('.') {
            return None;
        }

        // A Go import path carries the module prefix (`example.com/project`)
        // which does not exist on disk. Try the full path first, then drop
        // leading segments until the remaining path matches a directory.
        let normalised = specifier.replace('\\', "/");
        let mut candidate = normalised.as_str();

        loop {
            let needle = format!("/{candidate}");
            let mut best: Option<&PathBuf> = None;

            for path in &self.files {
                let Some(dir) = path.parent() else { continue };
                let dir = dir.to_string_lossy().replace('\\', "/");
                if dir.ends_with(&needle) {
                    if best.map(|current| path < current).unwrap_or(true) {
                        best = Some(path);
                    }
                }
            }

            if best.is_some() {
                return best.cloned();
            }

            match candidate.split_once('/') {
                Some((_, rest)) => candidate = rest,
                None => return None,
            }
        }
    }

    fn resolve_rust(&self, specifier: &str, from_dir: &Path) -> Option<PathBuf> {
        // `use a::{b, c}` is recorded with the brace list; only the module path
        // matters for a file-level edge.
        let specifier = specifier.split("::{").next().unwrap_or(specifier).trim();
        let specifier = specifier.trim_end_matches("::").trim();

        if specifier.is_empty() {
            return None;
        }

        let parts: Vec<&str> = specifier.split("::").filter(|p| !p.is_empty()).collect();
        let mut base = from_dir.to_path_buf();
        let mut rest: &[&str] = &parts;

        match parts.first().copied() {
            Some("crate") => {
                base = self.rust_crate_root(from_dir)?;
                rest = &parts[1..];
            }
            Some("self") => {
                rest = &parts[1..];
            }
            Some("super") => {
                while let Some(first) = rest.first() {
                    if *first != "super" {
                        break;
                    }
                    base = base.parent()?.to_path_buf();
                    rest = &rest[1..];
                }
            }
            // `std`, `core`, `alloc` and external crates are not in the tree.
            Some(_) => return None,
            None => return None,
        }

        let mut path = base;
        for part in rest {
            path.push(part);
        }

        self.probe_rust(&path)
    }

    /// Nearest ancestor containing `lib.rs` or `main.rs`.
    fn rust_crate_root(&self, from_dir: &Path) -> Option<PathBuf> {
        let mut current = Some(from_dir);
        while let Some(dir) = current {
            if self.by_path.contains_key(&normalize(&dir.join("lib.rs")))
                || self.by_path.contains_key(&normalize(&dir.join("main.rs")))
            {
                return Some(dir.to_path_buf());
            }
            current = dir.parent();
        }
        None
    }

    fn probe(&self, base: &Path) -> Option<PathBuf> {
        if self.by_path.contains_key(base) {
            return Some(base.to_path_buf());
        }

        for ext in JS_EXTENSIONS {
            let candidate = base.with_extension(ext);
            if self.by_path.contains_key(&candidate) {
                return Some(candidate);
            }
        }

        None
    }

    fn probe_index(&self, base: &Path) -> Option<PathBuf> {
        for ext in JS_EXTENSIONS {
            let candidate = normalize(&base.join(format!("index.{ext}")));
            if self.by_path.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    fn probe_python(&self, base: &Path) -> Option<PathBuf> {
        let module = base.with_extension("py");
        if self.by_path.contains_key(&module) {
            return Some(module);
        }

        let package = normalize(&base.join("__init__.py"));
        if self.by_path.contains_key(&package) {
            return Some(package);
        }

        None
    }

    fn probe_rust(&self, base: &Path) -> Option<PathBuf> {
        let direct = base.with_extension("rs");
        if self.by_path.contains_key(&direct) {
            return Some(direct);
        }

        let module = normalize(&base.join("mod.rs"));
        if self.by_path.contains_key(&module) {
            return Some(module);
        }

        None
    }
}

/// Lexically normalise a path: drop `.`, resolve `..`, and use a stable
/// separator. This avoids the `\\?\` prefix that `canonicalize` adds on
/// Windows, which would make index keys and lookups disagree.
pub fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Never pop past the root of an absolute path.
                if parts.len() > 1 {
                    parts.pop();
                }
            }
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }

    if parts.is_empty() {
        PathBuf::from(".")
    } else {
        parts.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImportKind, Language, Span};
    use std::path::PathBuf;

    fn span() -> Span {
        Span {
            start: crate::Position {
                line: 1,
                column: 1,
                offset: 0,
            },
            end: crate::Position {
                line: 1,
                column: 1,
                offset: 0,
            },
        }
    }

    fn discovered(root: &str, paths: &[&str]) -> Vec<DiscoveredFile> {
        paths
            .iter()
            .map(|relative| DiscoveredFile {
                path: PathBuf::from(root).join(relative),
                language: Language::from_path(Path::new(relative)).unwrap_or(Language::TypeScript),
                size: 0,
            })
            .collect()
    }

    fn import(file: &str, module: &str) -> Import {
        Import {
            file: PathBuf::from(file),
            span: span(),
            module: module.to_string(),
            kind: ImportKind::Static,
            items: Vec::new(),
            is_type_only: false,
        }
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        let normalized = normalize(Path::new("C:/repo/src/./services/../index.ts"));
        assert_eq!(normalized, PathBuf::from("C:/repo/src/index.ts"));
    }

    #[test]
    fn resolves_relative_typescript_imports() {
        let root = "C:/repo";
        let files = discovered(
            root,
            &["src/index.ts", "src/services/user.ts", "src/services/mod.ts"],
        );
        let index = FileIndex::new(&files, Path::new(root));

        let target = index
            .resolve(&import("C:/repo/src/index.ts", "./services/user"))
            .unwrap();
        assert_eq!(index.path_of(target).unwrap(), Path::new("C:/repo/src/services/user.ts"));

        // TypeScript ESM style: ./services/mod.js means ./services/mod.ts.
        let target = index
            .resolve(&import("C:/repo/src/index.ts", "./services/mod.js"))
            .unwrap();
        assert_eq!(index.path_of(target).unwrap(), Path::new("C:/repo/src/services/mod.ts"));
    }

    #[test]
    fn resolves_directory_index_files() {
        let root = "C:/repo";
        let files = discovered(root, &["src/index.ts", "src/services/index.ts"]);
        let index = FileIndex::new(&files, Path::new(root));

        let target = index
            .resolve(&import("C:/repo/src/index.ts", "./services"))
            .unwrap();
        assert_eq!(
            index.path_of(target).unwrap(),
            Path::new("C:/repo/src/services/index.ts")
        );
    }

    #[test]
    fn leaves_external_packages_unresolved() {
        let root = "C:/repo";
        let files = discovered(root, &["src/index.ts"]);
        let index = FileIndex::new(&files, Path::new(root));

        assert!(index.resolve(&import("C:/repo/src/index.ts", "react")).is_none());
    }

    #[test]
    fn resolves_python_relative_and_absolute_imports() {
        let root = "C:/repo";
        let files = discovered(
            root,
            &["app/main.py", "app/utils.py", "app/pkg/__init__.py", "lib/core.py"],
        );
        let index = FileIndex::new(&files, Path::new(root));

        let sibling = index
            .resolve(&import("C:/repo/app/main.py", ".utils"))
            .unwrap();
        assert_eq!(index.path_of(sibling).unwrap(), Path::new("C:/repo/app/utils.py"));

        let package = index
            .resolve(&import("C:/repo/app/main.py", ".pkg"))
            .unwrap();
        assert_eq!(
            index.path_of(package).unwrap(),
            Path::new("C:/repo/app/pkg/__init__.py")
        );

        let absolute = index
            .resolve(&import("C:/repo/app/main.py", "lib.core"))
            .unwrap();
        assert_eq!(index.path_of(absolute).unwrap(), Path::new("C:/repo/lib/core.py"));
    }

    #[test]
    fn resolves_rust_crate_and_super_paths() {
        let root = "C:/repo";
        let files = discovered(
            root,
            &["src/main.rs", "src/lib.rs", "src/graph.rs", "src/api/mod.rs"],
        );
        let index = FileIndex::new(&files, Path::new(root));

        let crate_path = index
            .resolve(&import("C:/repo/src/main.rs", "crate::graph"))
            .unwrap();
        assert_eq!(index.path_of(crate_path).unwrap(), Path::new("C:/repo/src/graph.rs"));

        let module = index
            .resolve(&import("C:/repo/src/main.rs", "crate::api"))
            .unwrap();
        assert_eq!(index.path_of(module).unwrap(), Path::new("C:/repo/src/api/mod.rs"));
    }

    #[test]
    fn resolves_go_package_paths_by_suffix() {
        let root = "C:/repo";
        let mut files = discovered(
            root,
            &["internal/store/store.go", "internal/api/api.go", "cmd/main.go"],
        );
        files[0].language = Language::Go;
        files[1].language = Language::Go;
        files[2].language = Language::Go;

        let index = FileIndex::new(&files, Path::new(root));

        let target = index
            .resolve(&import("C:/repo/cmd/main.go", "example.com/project/internal/store"))
            .unwrap();
        assert_eq!(
            index.path_of(target).unwrap(),
            Path::new("C:/repo/internal/store/store.go")
        );
    }
}



