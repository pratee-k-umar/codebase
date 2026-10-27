//! Tests for file discovery and the ignore report.
//!
//! These exercise the rules that decide what gets analyzed, and — just as
//! importantly — that every skipped path is reported with the reason it was
//! skipped.

use std::fs;
use std::path::Path;

use codebase_intel_core::discovery::{discover, DiscoverySummary, IgnoreReason};
use codebase_intel_core::{analyze, Config};
use tempfile::TempDir;

const TS: &str = "export function hello(): void {}\n";
const PY: &str = "def hello():\n    pass\n";
const GO: &str = "package main\n\nfunc hello() {}\n";
const RS: &str = "pub fn hello() {}\n";

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn config() -> Config {
    let mut config = Config::default();
    // Discovery must not depend on (or pollute) a shared cache directory.
    config.cache.enabled = false;
    config
}

fn reasons(summary: &DiscoverySummary) -> Vec<(IgnoreReason, usize)> {
    summary
        .ignored_by_reason
        .iter()
        .map(|(reason, count)| (*reason, *count))
        .collect()
}

fn skipped_paths(summary: &DiscoverySummary) -> Vec<String> {
    summary
        .ignored
        .iter()
        .map(|entry| {
            entry
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn gitignore_controls_what_is_analyzed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "generated.ts\n");
    write(root, "src/keep.ts", TS);
    write(root, "generated.ts", TS);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 1, "only the non-ignored file should be analyzed");
    assert_eq!(report.files[0].path.file_name().unwrap(), "keep.ts");
    assert_eq!(report.summary.count(IgnoreReason::GitIgnore), 1);
    assert!(skipped_paths(&report.summary).contains(&"generated.ts".to_string()));
}

#[test]
fn gitignore_directory_pattern_skips_the_whole_tree() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // "generated" is deliberately not one of the default config excludes, so
    // this can only be skipped by the .gitignore rule.
    write(root, ".gitignore", "generated/\n");
    write(root, "src/main.ts", TS);
    write_generated_tree(root);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(
        report.summary.count(IgnoreReason::GitIgnore),
        2,
        "both files under generated/ are ignored, got {:?}",
        reasons(&report.summary)
    );
}

fn write_generated_tree(root: &Path) {
    write(root, "generated/bundle.ts", TS);
    write(root, "generated/vendor.ts", TS);
}

#[test]
fn gitignore_negation_whitelists_a_file() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "*.ts\n!keep.ts\n");
    write(root, "keep.ts", TS);
    write(root, "drop.ts", TS);

    let report = discover(root, &config()).unwrap();

    let names: Vec<String> = report
        .files
        .iter()
        .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(names.contains(&"keep.ts".to_string()), "negated file should survive: {names:?}");
    assert!(!names.contains(&"drop.ts".to_string()), "ignored file leaked in: {names:?}");
}

#[test]
fn nested_gitignore_overrides_parent() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "*.ts\n");
    write(root, "src/.gitignore", "!special.ts\n");
    write(root, "src/special.ts", TS);
    write(root, "src/ordinary.ts", TS);

    let report = discover(root, &config()).unwrap();

    let names: Vec<String> = report
        .files
        .iter()
        .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(names.contains(&"special.ts".to_string()), "nested negation failed: {names:?}");
    assert!(!names.contains(&"ordinary.ts".to_string()), "parent ignore failed: {names:?}");
}

#[test]
fn ignore_file_is_honored() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".ignore", "secret.ts\n");
    write(root, "secret.ts", TS);
    write(root, "public.ts", TS);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.summary.count(IgnoreReason::IgnoreFile), 1);
}

/// Regression: a nested `.gitignore` containing a bare `*` must only ignore
/// its own directory. Merging all `.gitignore` files into one repo-rooted
/// matcher made it ignore the entire repository (strapi's `.husky/_` does this).
#[test]
fn nested_ignore_everything_does_not_leak_outward() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".husky/_/.gitignore", "*\n");
    write(root, ".husky/install.sh", "#!/bin/sh\n");
    write(root, ".husky/pre-commit.ts", TS);
    write(root, "src/app.ts", TS);
    write(root, "README.md", "# hi\n");

    let report = discover(root, &config()).unwrap();

    let names: Vec<String> = report
        .files
        .iter()
        .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(
        names.contains(&"app.ts".to_string()),
        "files outside the nested ignore scope must survive: {names:?}"
    );
    assert!(
        names.contains(&"pre-commit.ts".to_string()),
        "sibling of the ignored dir must survive: {names:?}"
    );
    // Only `.husky/_/.gitignore` itself falls under the `*` pattern.
    assert_eq!(report.summary.count(IgnoreReason::GitIgnore), 1);
}

/// A nested `*` must still ignore everything inside its own directory.
#[test]
fn nested_ignore_everything_still_applies_inside() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "vendor/.gitignore", "*\n");
    write(root, "vendor/lib.ts", TS);
    write(root, "vendor/nested/deep.ts", TS);

    let report = discover(root, &config()).unwrap();

    assert!(report.files.is_empty(), "vendor/ should be fully ignored");
    // `vendor/.gitignore`, `vendor/lib.ts` and `vendor/nested/deep.ts`.
    assert_eq!(report.summary.count(IgnoreReason::GitIgnore), 3);
}

/// Directory patterns in a nested file apply to that subtree only.
#[test]
fn nested_directory_pattern_is_scoped() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "packages/a/.gitignore", "generated/\n");
    write(root, "packages/a/generated/x.ts", TS);
    write(root, "packages/a/src/y.ts", TS);
    // A sibling package has its own "generated" dir which is NOT ignored.
    write(root, "packages/b/generated/z.ts", TS);

    let report = discover(root, &config()).unwrap();

    let names: Vec<String> = report
        .files
        .iter()
        .map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    assert!(names.contains(&"y.ts".to_string()), "{names:?}");
    assert!(names.contains(&"z.ts".to_string()), "sibling scope leaked: {names:?}");
    assert!(!names.contains(&"x.ts".to_string()), "scoped ignore failed: {names:?}");
    assert_eq!(report.summary.count(IgnoreReason::GitIgnore), 1);
}

#[test]
fn config_excludes_are_reported_with_their_reason() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "app.py", PY);
    write(root, "app.ts", TS);

    let mut config = config();
    config.exclude.push("**/*.py".to_string());

    let report = discover(root, &config).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.summary.count(IgnoreReason::ConfigExclude), 1);
    assert!(skipped_paths(&report.summary).contains(&"app.py".to_string()));
}

#[test]
fn git_directory_is_pruned_and_reported() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".git/config", "[core]\n");
    write(root, ".git/objects/ab/cdef", "junk");
    write(root, "src/main.ts", TS);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 1, "nothing inside .git may be analyzed");
    assert_eq!(report.summary.dirs_pruned, 1);
    assert_eq!(report.summary.count(IgnoreReason::GitMetadata), 1);
    assert!(
        report.summary.files_scanned < 3,
        "the walk must not descend into .git, scanned {}",
        report.summary.files_scanned
    );
}

#[test]
fn unsupported_files_are_counted_not_listed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "README.md", "# hi\n");
    write(root, "data.json", "{}\n");
    write(root, "main.ts", TS);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.summary.files_unsupported, 2);
    assert!(
        report.summary.ignored.is_empty(),
        "unsupported files are not ignore-rule violations"
    );
}

#[test]
fn counts_are_internally_consistent() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "ignored.ts\n");
    write(root, "ignored.ts", TS);
    write(root, "kept.ts", TS);
    write(root, "notes.md", "# notes\n");

    let report = discover(root, &config()).unwrap();
    let summary = &report.summary;

    assert_eq!(
        summary.files_scanned,
        summary.files_included + summary.files_ignored + summary.files_unsupported,
        "every scanned file must be accounted for exactly once"
    );
    assert_eq!(summary.files_included, report.files.len());
    assert_eq!(
        summary.total_skipped(),
        summary.files_ignored + summary.dirs_pruned
    );
    let by_reason: usize = summary.ignored_by_reason.values().sum();
    assert_eq!(
        by_reason,
        summary.total_skipped(),
        "per-reason counts must add up to the total"
    );
    assert_eq!(summary.ignored.len(), summary.total_skipped());
}

#[test]
fn disabled_language_is_unsupported_not_ignored() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "app.py", PY);
    write(root, "main.rs", RS);

    let mut config = config();
    config.languages.clear();
    config.languages.insert(codebase_intel_core::Language::Rust);

    let report = discover(root, &config).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].language, codebase_intel_core::Language::Rust);
    assert_eq!(report.summary.files_unsupported, 1);
}

#[test]
fn oversized_files_are_reported_as_too_large() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // 2 MB against a 1 MB limit. The body is padded with a trailing comment so
    // the file stays syntactically valid.
    let padding = "x".repeat(2 * 1024 * 1024);
    write(root, "huge.rs", &format!("pub fn hello() {{}}\n// {padding}\n"));
    write(root, "small.rs", RS);

    let mut config = config();
    config.parsers.rust.max_file_size_mb = 1;

    let report = discover(root, &config).unwrap();

    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].path.file_name().unwrap(), "small.rs");
    assert_eq!(report.summary.count(IgnoreReason::TooLarge), 1);
}

#[test]
fn zero_size_limit_means_unlimited() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "small.rs", RS);

    let mut config = config();
    config.parsers.rust.max_file_size_mb = 0;

    let report = discover(root, &config).unwrap();
    assert_eq!(report.files.len(), 1, "0 should disable the limit, not skip everything");
}

#[test]
fn analysis_result_carries_the_report() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "ignored.ts\n");
    write(root, "ignored.ts", TS);
    write(root, "kept.ts", TS);

    let result = analyze(root, config()).unwrap();

    assert_eq!(result.discovery.files_included, 1);
    assert_eq!(result.discovery.count(IgnoreReason::GitIgnore), 1);
    assert!(result
        .discovery
        .ignored
        .iter()
        .any(|entry| entry.reason == IgnoreReason::GitIgnore));
}

#[test]
fn report_is_exported_in_json_and_summary() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "ignored.ts\n");
    write(root, "ignored.ts", TS);
    write(root, "kept.ts", TS);

    let result = analyze(root, config()).unwrap();

    let mut json = Vec::new();
    codebase_intel_core::export::export(&result, codebase_intel_core::OutputFormat::Json, &mut json)
        .unwrap();
    let json = String::from_utf8(json).unwrap();
    assert!(json.contains("\"discovery\""), "json export missing discovery block");
    assert!(json.contains("git_ignore"), "json export missing ignore reason");

    let mut summary = Vec::new();
    codebase_intel_core::export::export(
        &result,
        codebase_intel_core::OutputFormat::Summary,
        &mut summary,
    )
    .unwrap();
    let summary = String::from_utf8(summary).unwrap();
    assert!(summary.contains("=== Discovery ==="));
    assert!(summary.contains("gitignore"));
}

#[test]
fn discovery_does_not_depend_on_git_repository() {
    // The tool applies .gitignore even outside a git checkout, which is what
    // makes it usable on exported tarballs and vendored snapshots.
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, ".gitignore", "build.ts\n");
    write(root, "build.ts", TS);
    write(root, "app.ts", TS);

    assert!(!root.join(".git").exists());

    let report = discover(root, &config()).unwrap();
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.summary.count(IgnoreReason::GitIgnore), 1);
}

#[test]
fn missing_gitignore_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "app.ts", TS);
    write(root, "app.py", PY);
    write(root, "app.go", GO);

    let report = discover(root, &config()).unwrap();

    assert_eq!(report.files.len(), 3);
    assert_eq!(report.summary.files_ignored, 0);
    assert_eq!(report.summary.dirs_pruned, 0);
    assert!(reasons(&report.summary).is_empty());
}
