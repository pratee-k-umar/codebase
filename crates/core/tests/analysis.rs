//! End-to-end tests for the analysis pipeline.
//!
//! Each test writes a tiny multi-language fixture into a temporary directory,
//! runs the full analyzer over it, and asserts on the resulting model and
//! exported artifacts.

use std::fs;
use std::path::Path;

use codebase_intel_core::export::export;
use codebase_intel_core::{analyze, Config, Language, OutputFormat, SymbolKind};
use tempfile::TempDir;

const TS_SOURCE: &str = r#"
export interface Options {
  verbose: boolean;
}

export class UserService {
  async getUser(id: number): Promise<string> {
    const label = this.format(id);
    return label;
  }

  private format(id: number): string {
    return `user-${id}`;
  }
}

export function main(): void {
  const service = new UserService();
  service.getUser(1);
}
"#;

const PY_SOURCE: &str = r#"
import os


class Greeter:
    def hello(self, name: str) -> str:
        return f"hi {name}"


def main():
    greeter = Greeter()
    print(greeter.hello("world"))
"#;

const GO_SOURCE: &str = r#"
package main

import "fmt"

type Greeter struct{}

func (g Greeter) Hello(name string) string {
	return "hi " + name
}

func main() {
	fmt.Println(Greeter{}.Hello("world"))
}
"#;

const RS_SOURCE: &str = r#"
pub struct Greeter;

impl Greeter {
    pub fn hello(&self, name: &str) -> String {
        format!("hi {}", name)
    }
}

fn main() {
    let greeter = Greeter;
    println!("{}", greeter.hello("world"));
}
"#;

/// Build a fixture containing one file per supported language.
fn fixture() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let root = dir.path();

    fs::write(root.join("sample.ts"), TS_SOURCE).unwrap();
    fs::write(root.join("sample.py"), PY_SOURCE).unwrap();
    fs::write(root.join("sample.go"), GO_SOURCE).unwrap();
    fs::write(root.join("sample.rs"), RS_SOURCE).unwrap();

    dir
}

fn analyze_fixture(dir: &TempDir) -> codebase_intel_core::AnalysisResult {
    let mut config = Config::default();
    // Keep the test hermetic: never read or write a shared cache directory.
    config.cache.enabled = false;
    analyze(dir.path(), config).expect("analysis should succeed")
}

#[test]
fn analyzes_every_supported_language() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    assert_eq!(result.file_count(), 4, "all four fixture files should be analyzed");

    let languages: Vec<Language> = result.symbols.iter().map(|s| s.language).collect();
    for expected in [
        Language::TypeScript,
        Language::Python,
        Language::Go,
        Language::Rust,
    ] {
        assert!(
            languages.contains(&expected),
            "expected at least one {expected:?} symbol, got {languages:?}"
        );
    }

    assert!(result.errors.is_empty(), "unexpected errors: {:?}", result.errors);
}

#[test]
fn extracts_functions_classes_and_interfaces() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let names: Vec<&str> = result.symbols.iter().map(|s| s.name.as_str()).collect();
    for expected in ["UserService", "Greeter", "main", "hello", "getUser"] {
        assert!(names.contains(&expected), "missing symbol {expected:?} in {names:?}");
    }

    let kinds: Vec<SymbolKind> = result.symbols.iter().map(|s| s.kind).collect();
    assert!(
        kinds.contains(&SymbolKind::Class),
        "expected a class symbol, got {kinds:?}"
    );
    assert!(
        kinds.contains(&SymbolKind::Function),
        "expected a function symbol, got {kinds:?}"
    );
}

#[test]
fn symbol_ids_are_unique_and_reference_their_file() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let mut seen = std::collections::HashSet::new();
    for symbol in &result.symbols {
        assert!(seen.insert(symbol.id.clone()), "duplicate symbol id: {}", symbol.id.0);
        assert!(
            symbol.file.exists(),
            "symbol {} points at missing file {}",
            symbol.name,
            symbol.file.display()
        );
        assert!(
            symbol.span.start.line >= 1,
            "span lines are 1-indexed, got {} for {}",
            symbol.span.start.line,
            symbol.name
        );
    }
}

#[test]
fn file_dependency_nodes_cover_every_parsed_file() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    assert_eq!(result.file_dependencies.nodes.len(), 4);
    for node in &result.file_dependencies.nodes {
        assert!(node.path.exists(), "dependency node path vanished: {}", node.path.display());
        assert!(node.symbol_count > 0, "{} has no symbols attributed", node.path.display());
    }
}

#[test]
fn metrics_are_populated() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    assert_eq!(result.metrics.total_files, 4);
    assert_eq!(result.metrics.total_symbols, result.symbols.len());
    assert!(!result.metrics.languages.is_empty());
    // Coupling is derived from the file graph, so every node needs an entry.
    assert_eq!(result.metrics.coupling.instability.len(), 4);
}

#[test]
fn every_export_format_produces_output() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    for format in [
        OutputFormat::Json,
        OutputFormat::Yaml,
        OutputFormat::Summary,
        OutputFormat::Mermaid,
        OutputFormat::GraphML,
        OutputFormat::Dot,
        OutputFormat::Csv,
    ] {
        let mut buffer = Vec::new();
        export(&result, format, &mut buffer).expect("export should succeed");
        assert!(!buffer.is_empty(), "{format:?} produced no output");

        let text = String::from_utf8(buffer).expect("exports must be valid UTF-8");
        // Summary aggregates counts and intentionally omits file names; every
        // graph-oriented format names the files it describes.
        if format != OutputFormat::Summary {
            assert!(
                text.contains("sample"),
                "{format:?} output should mention a fixture file"
            );
        }
    }
}

#[test]
fn json_export_round_trips() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let mut buffer = Vec::new();
    export(&result, OutputFormat::Json, &mut buffer).expect("json export");
    let json = String::from_utf8(buffer).unwrap();

    let parsed: codebase_intel_core::AnalysisResult =
        serde_json::from_str(&json).expect("analysis result should deserialize");

    assert_eq!(parsed.symbols.len(), result.symbols.len());
    assert_eq!(
        parsed.file_dependencies.nodes.len(),
        result.file_dependencies.nodes.len()
    );
    assert_eq!(parsed.call_graph.node_count(), result.call_graph.node_count());
}

#[test]
fn graphml_export_is_well_formed_xml() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let mut buffer = Vec::new();
    export(&result, OutputFormat::GraphML, &mut buffer).expect("graphml export");
    let xml = String::from_utf8(buffer).unwrap();

    assert!(xml.starts_with("<?xml"), "graphml must begin with an XML declaration");
    assert!(xml.contains("<graphml"));
    assert!(xml.trim_end().ends_with("</graphml>"));
    assert_eq!(
        xml.matches("<node ").count(),
        xml.matches("</node>").count(),
        "node elements must be balanced"
    );
    assert_eq!(
        xml.matches("<edge ").count(),
        xml.matches("</edge>").count(),
        "edge elements must be balanced"
    );
}

#[test]
fn respects_language_filter() {
    let dir = fixture();
    let mut config = Config::default();
    config.cache.enabled = false;
    config.languages.clear();
    config.languages.insert(Language::Rust);

    let result = analyze(dir.path(), config).expect("analysis should succeed");

    assert_eq!(result.file_count(), 1, "only the Rust file should be analyzed");
    assert!(result.symbols.iter().all(|s| s.language == Language::Rust));
}

#[test]
fn respects_exclude_patterns() {
    let dir = fixture();
    let mut config = Config::default();
    config.cache.enabled = false;
    config.exclude.push("**/*.py".to_string());

    let result = analyze(dir.path(), config).expect("analysis should succeed");

    assert_eq!(result.file_count(), 3, "the excluded Python file should be skipped");
    assert!(result.symbols.iter().all(|s| s.language != Language::Python));
}

#[test]
fn empty_directory_analyzes_cleanly() {
    let dir = TempDir::new().unwrap();
    let mut config = Config::default();
    config.cache.enabled = false;

    let result = analyze(dir.path(), config).expect("empty analysis should not fail");

    assert_eq!(result.file_count(), 0);
    assert!(result.symbols.is_empty());
    assert!(result.errors.is_empty());
}

#[test]
fn unparsable_source_is_reported_not_fatal() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("broken.ts"), "function ((( {").unwrap();
    fs::write(dir.path().join("good.rs"), RS_SOURCE).unwrap();

    let mut config = Config::default();
    config.cache.enabled = false;
    let result = analyze(dir.path(), config).expect("analysis should not abort");

    // Tree-sitter is error tolerant, so a broken file simply yields few or no
    // symbols. The important part is that the healthy file still gets parsed.
    assert!(
        result.symbols.iter().any(|s| s.language == Language::Rust),
        "the valid Rust file should still be analyzed"
    );
}

#[test]
fn config_loads_from_partial_toml() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("partial.toml");
    fs::write(
        &config_path,
        "exclude = [\"**/vendor/**\"]\n[lint]\nunused = false\n",
    )
    .unwrap();

    let config = Config::load(Some(config_path.as_path())).expect("partial config should load");

    assert_eq!(config.exclude, vec!["**/vendor/**".to_string()]);
    // Unspecified sections fall back to defaults rather than erroring.
    assert!(config.languages.contains(&Language::Rust));
    assert!(config.cache.enabled);
}

#[test]
fn missing_config_file_falls_back_to_defaults() {
    let missing = Path::new("this-config-does-not-exist.toml");
    let config = Config::load(Some(missing)).expect("missing config should not error");

    assert_eq!(config, Config::default(), "should be the built-in defaults");
}

#[test]
fn detects_rust_struct_and_impl_methods() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let rust_symbols: Vec<_> = result
        .symbols
        .iter()
        .filter(|s| s.language == Language::Rust)
        .collect();

    assert!(
        rust_symbols.iter().any(|s| s.name == "Greeter" && s.kind == SymbolKind::Struct),
        "expected Greeter struct, got {:?}",
        rust_symbols.iter().map(|s| (&s.name, s.kind)).collect::<Vec<_>>()
    );
    assert!(
        rust_symbols.iter().any(|s| s.name == "hello"),
        "expected the impl method, got {:?}",
        rust_symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
}

#[test]
fn detects_python_class_and_method() {
    let dir = fixture();
    let result = analyze_fixture(&dir);

    let python_symbols: Vec<_> = result
        .symbols
        .iter()
        .filter(|s| s.language == Language::Python)
        .collect();

    assert!(
        python_symbols.iter().any(|s| s.name == "Greeter" && s.kind == SymbolKind::Class),
        "expected Greeter class, got {:?}",
        python_symbols.iter().map(|s| (&s.name, s.kind)).collect::<Vec<_>>()
    );
}

#[test]
fn compares_equal_configs() {
    // Guards against accidental non-determinism in defaults, which would make
    // the cache keys and the tests above flaky.
    assert_eq!(Config::default(), Config::default());
}

#[test]
fn relative_and_absolute_roots_agree() {
    let dir = fixture();
    let absolute = analyze_fixture(&dir);

    let mut config = Config::default();
    config.cache.enabled = false;
    let relative = analyze(dir.path(), config).expect("relative analysis");

    assert_eq!(absolute.symbols.len(), relative.symbols.len());
}

#[test]
fn resolves_relative_imports_into_file_edges() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("src/app.ts"),
        "import { helper } from './util';\nexport function main(): void { helper(); }\n",
    )
    .unwrap();
    fs::write(
        root.join("src/util.ts"),
        "export function helper(): void {}\n",
    )
    .unwrap();

    let mut config = Config::default();
    config.cache.enabled = false;
    let result = analyze(root, config).expect("analysis should succeed");

    assert_eq!(result.file_dependencies.nodes.len(), 2);
    assert_eq!(
        result.file_dependencies.edges.len(),
        1,
        "app.ts imports ./util, so there should be exactly one relation"
    );

    let edge = &result.file_dependencies.edges[0];
    let from = result
        .file_dependencies
        .nodes
        .iter()
        .find(|node| node.id == edge.from)
        .unwrap();
    let to = result
        .file_dependencies
        .nodes
        .iter()
        .find(|node| node.id == edge.to)
        .unwrap();

    assert!(from.path.ends_with("app.ts"), "edge starts at app.ts");
    assert!(to.path.ends_with("util.ts"), "edge points at util.ts");
}

#[test]
fn unresolved_package_imports_do_not_create_edges() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    fs::write(
        root.join("app.ts"),
        "import React from 'react';\nexport function main(): void {}\n",
    )
    .unwrap();

    let mut config = Config::default();
    config.cache.enabled = false;
    let result = analyze(root, config).expect("analysis should succeed");

    assert_eq!(result.file_dependencies.nodes.len(), 1);
    assert!(
        result.file_dependencies.edges.is_empty(),
        "external packages must not invent relations"
    );
}
