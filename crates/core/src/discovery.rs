//! File discovery.
//!
//! Decides which files get analyzed and records why every other file was
//! skipped. Discovery walks the tree once, prunes `.git` and directories
//! matched by the configured `exclude` globs, then classifies each remaining
//! file in this order:
//!
//! 1. the analysis cache file
//! 2. configured `exclude` globs
//! 3. `.git/info/exclude`
//! 4. `.ignore`
//! 5. `.gitignore` (including nested files, with Git's precedence rules)
//! 6. language support and the per-language enable list
//! 7. the per-language file size limit
//!
//! Anything that fails one of those rules is reported with a reason, so a scan
//! can explain exactly why a file is missing from the analysis.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::config::Config;
use crate::{DiscoveredFile, Language, Result};

/// Maximum number of skipped entries kept in the report. Counters always cover
/// every skipped file; only the sample list is capped.
const MAX_REPORTED_ENTRIES: usize = 500;

/// Why a path was not analyzed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IgnoreReason {
    /// Matched a pattern in a `.gitignore` file.
    GitIgnore,
    /// Matched `.git/info/exclude`.
    GitExclude,
    /// Matched a `.ignore` file.
    IgnoreFile,
    /// Matched one of the configured `exclude` globs.
    ConfigExclude,
    /// Lives inside the repository's `.git` directory.
    GitMetadata,
    /// Larger than the per-language size limit.
    TooLarge,
    /// The analysis cache file itself.
    CacheFile,
}

impl IgnoreReason {
    /// Stable, human-readable label used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            IgnoreReason::GitIgnore => "gitignore",
            IgnoreReason::GitExclude => "git_exclude",
            IgnoreReason::IgnoreFile => "ignore_file",
            IgnoreReason::ConfigExclude => "config_exclude",
            IgnoreReason::GitMetadata => "git_metadata",
            IgnoreReason::TooLarge => "too_large",
            IgnoreReason::CacheFile => "cache_file",
        }
    }
}

/// A single path that discovery skipped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredEntry {
    pub path: PathBuf,
    pub reason: IgnoreReason,
    /// True when a whole directory was pruned rather than one file.
    #[serde(default)]
    pub is_dir: bool,
}

/// Serializable summary of a discovery run.
///
/// This is what lands in exported reports; the full file list stays in
/// [`DiscoveryReport::files`] because it is only useful to the analyzer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoverySummary {
    /// Every regular file seen while walking, before any filtering.
    pub files_scanned: usize,
    /// Files selected for analysis.
    pub files_included: usize,
    /// Files skipped by an ignore rule.
    pub files_ignored: usize,
    /// Files whose extension or language is not part of this run.
    pub files_unsupported: usize,
    /// Directories pruned without descending into them.
    pub dirs_pruned: usize,
    /// Skip counts grouped by reason.
    pub ignored_by_reason: BTreeMap<IgnoreReason, usize>,
    /// Sample of skipped entries, capped at [`MAX_REPORTED_ENTRIES`].
    pub ignored: Vec<IgnoredEntry>,
    /// True when `ignored` holds less than `files_ignored + dirs_pruned`.
    pub ignored_truncated: bool,
}

impl DiscoverySummary {
    /// Number of skipped entries of a given reason.
    pub fn count(&self, reason: IgnoreReason) -> usize {
        self.ignored_by_reason.get(&reason).copied().unwrap_or(0)
    }

    /// Total number of skipped entries, files and pruned directories alike.
    pub fn total_skipped(&self) -> usize {
        self.files_ignored + self.dirs_pruned
    }
}

/// Result of a discovery run.
#[derive(Debug)]
pub struct DiscoveryReport {
    /// Files selected for analysis, in walk order.
    pub files: Vec<DiscoveredFile>,
    /// Counters and a sample of skipped paths.
    pub summary: DiscoverySummary,
}

/// Walk `root` and decide which files to analyze.
pub fn discover(root: &Path, config: &Config) -> Result<DiscoveryReport> {
    let mut gitignore_files: Vec<PathBuf> = Vec::new();
    let mut ignore_files: Vec<PathBuf> = Vec::new();
    let mut candidates: Vec<(PathBuf, u64)> = Vec::new();
    let mut scanned = 0usize;
    let mut skips = SkipLog::default();

    let mut walker = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        // Unreadable entries are skipped rather than aborting the scan.
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        let path = entry.path();
        let file_type = entry.file_type();

        if file_type.is_dir() {
            if is_git_metadata(path) {
                skips.record(path, IgnoreReason::GitMetadata, true);
                walker.skip_current_dir();
                continue;
            }
            if dir_is_excluded(config, path) {
                skips.record(path, IgnoreReason::ConfigExclude, true);
                walker.skip_current_dir();
                continue;
            }
            continue;
        }

        // A `.git` file replaces the directory in worktrees and submodules.
        if is_git_metadata(path) {
            scanned += 1;
            skips.record(path, IgnoreReason::GitMetadata, false);
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        scanned += 1;

        match entry.file_name().to_str() {
            Some(".gitignore") => gitignore_files.push(path.to_path_buf()),
            Some(".ignore") => ignore_files.push(path.to_path_buf()),
            _ => {}
        }

        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        candidates.push((path.to_path_buf(), size));
    }

    let matchers = IgnoreMatchers::build(&gitignore_files, &ignore_files);
    let git_exclude_matcher = git_exclude_matcher(root);

    let cache_file = config.cache_file_path();
    let mut files = Vec::new();
    let mut unsupported = 0usize;

    for (path, size) in candidates {
        if config.cache.enabled && path == cache_file {
            skips.record(&path, IgnoreReason::CacheFile, false);
            continue;
        }

        if config.is_excluded(&path) {
            skips.record(&path, IgnoreReason::ConfigExclude, false);
            continue;
        }

        if let Some(reason) = git_exclude_matcher
            .as_ref()
            .filter(|matcher| matcher.matched_path_or_any_parents(&path, false).is_ignore())
            .map(|_| IgnoreReason::GitExclude)
        {
            skips.record(&path, reason, false);
            continue;
        }

        if let Some(reason) = matchers.reason_for(&path) {
            skips.record(&path, reason, false);
            continue;
        }

        let Some(language) = Language::from_path(&path) else {
            unsupported += 1;
            continue;
        };

        if !config.should_parse(language) {
            unsupported += 1;
            continue;
        }

        if size > config.max_file_size_bytes(language) as u64 {
            skips.record(&path, IgnoreReason::TooLarge, false);
            continue;
        }

        files.push(DiscoveredFile {
            path,
            language,
            size,
        });
    }

    let summary = DiscoverySummary {
        files_scanned: scanned,
        files_included: files.len(),
        files_ignored: skips.files,
        files_unsupported: unsupported,
        dirs_pruned: skips.dirs,
        ignored_by_reason: skips.by_reason,
        ignored: skips.entries,
        ignored_truncated: skips.truncated,
    };

    Ok(DiscoveryReport { files, summary })
}

/// Collects skip entries, counting all of them but only keeping a sample.
#[derive(Debug, Default)]
struct SkipLog {
    files: usize,
    dirs: usize,
    by_reason: BTreeMap<IgnoreReason, usize>,
    entries: Vec<IgnoredEntry>,
    truncated: bool,
}

impl SkipLog {
    fn record(&mut self, path: &Path, reason: IgnoreReason, is_dir: bool) {
        if is_dir {
            self.dirs += 1;
        } else {
            self.files += 1;
        }
        *self.by_reason.entry(reason).or_insert(0) += 1;

        if self.entries.len() < MAX_REPORTED_ENTRIES {
            self.entries.push(IgnoredEntry {
                path: path.to_path_buf(),
                reason,
                is_dir,
            });
        } else {
            self.truncated = true;
        }
    }
}

fn git_exclude_matcher(root: &Path) -> Option<Gitignore> {
    let exclude_file = root.join(".git").join("info").join("exclude");
    if !exclude_file.is_file() {
        return None;
    }

    // Patterns in `.git/info/exclude` are relative to the worktree root, not to
    // the `.git` directory, so they are added as lines against a root builder.
    let mut builder = GitignoreBuilder::new(root);
    let contents = std::fs::read_to_string(&exclude_file).ok()?;
    for line in contents.lines() {
        let _ = builder.add_line(None, line);
    }

    builder.build().ok()
}

/// A single directory's ignore rules.
///
/// Each `.gitignore` (and `.ignore`) file is scoped to the directory that
/// contains it. Merging files from different directories into one matcher is
/// wrong: a nested file containing `*` would then ignore the entire repository.
#[derive(Debug)]
struct MatcherEntry {
    dir: PathBuf,
    matcher: Gitignore,
    reason: IgnoreReason,
}

/// All ignore rules for a tree, ordered by precedence.
#[derive(Debug, Default)]
struct IgnoreMatchers {
    /// Deepest directories first; within one directory, `.ignore` before
    /// `.gitignore`.
    entries: Vec<MatcherEntry>,
}

impl IgnoreMatchers {
    fn build(gitignore_files: &[PathBuf], ignore_files: &[PathBuf]) -> Self {
        let mut entries = Vec::new();

        for (files, reason) in [
            (gitignore_files, IgnoreReason::GitIgnore),
            (ignore_files, IgnoreReason::IgnoreFile),
        ] {
            let mut by_dir: BTreeMap<PathBuf, Vec<&PathBuf>> = BTreeMap::new();
            for file in files {
                if let Some(dir) = file.parent() {
                    by_dir.entry(dir.to_path_buf()).or_default().push(file);
                }
            }

            for (dir, files) in by_dir {
                let mut builder = GitignoreBuilder::new(&dir);
                for file in files {
                    // A malformed pattern in one file must not discard the rest.
                    let _ = builder.add(file);
                }
                let Ok(matcher) = builder.build() else {
                    continue;
                };
                if matcher.is_empty() {
                    continue;
                }
                entries.push(MatcherEntry {
                    dir,
                    matcher,
                    reason,
                });
            }
        }

        // Deeper directories take precedence, so check them first. `.ignore`
        // wins over `.gitignore` in the same directory.
        entries.sort_by(|a, b| {
            let depth = b.dir.components().count().cmp(&a.dir.components().count());
            depth.then_with(|| reason_rank(a.reason).cmp(&reason_rank(b.reason)))
        });

        Self { entries }
    }

    /// Reason the path is ignored, or `None` if it is not.
    ///
    /// The first decisive match wins: an `Ignore` skips the file, a `Whitelist`
    /// (from a `!pattern`) stops the search and keeps it.
    fn reason_for(&self, path: &Path) -> Option<IgnoreReason> {
        for entry in &self.entries {
            // `matched_path_or_any_parents` panics when the path is outside the
            // matcher's root, so only consult matchers that contain this path.
            if !path.starts_with(&entry.dir) {
                continue;
            }

            let matched = entry.matcher.matched_path_or_any_parents(path, false);
            if matched.is_ignore() {
                return Some(entry.reason);
            }
            if matched.is_whitelist() {
                return None;
            }
        }

        None
    }
}

fn reason_rank(reason: IgnoreReason) -> u8 {
    match reason {
        IgnoreReason::IgnoreFile => 0,
        IgnoreReason::GitIgnore => 1,
        _ => 2,
    }
}

/// True for the `.git` directory (or the `.git` pointer file used by worktrees).
fn is_git_metadata(path: &Path) -> bool {
    path.file_name()
        .map(|name| name == ".git")
        .unwrap_or(false)
}

/// True when a directory matches the configured excludes.
///
/// Patterns that end in `/**` (such as the default `**/node_modules/**`) only
/// match paths *inside* the directory, so the directory itself is probed with a
/// synthetic child to allow pruning before descending.
fn dir_is_excluded(config: &Config, path: &Path) -> bool {
    config.is_excluded(path) || config.is_excluded(&path.join("__probe__"))
}


