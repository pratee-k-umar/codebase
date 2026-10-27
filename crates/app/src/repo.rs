//! Repository acquisition: cloning and validating local folders.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Derive a directory name from a repository URL.
///
/// Handles `https://host/org/name.git`, `git@host:org/name.git`, and plain
/// local paths. Returns `None` when nothing usable can be extracted.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }

    let without_scheme = match trimmed.split_once("://") {
        Some((_, rest)) => rest,
        None => trimmed,
    };

    // Find the path part: after the first `/`, or after the scp-style
    // `host:org/name` colon. A bare host such as `github.com` has neither and
    // does not name a repository.
    let path = if let Some((_, rest)) = without_scheme.split_once('/') {
        rest
    } else if let Some((_host, rest)) = without_scheme.split_once(':') {
        rest
    } else {
        return None;
    };

    let last = path
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())?;

    let name = last.strip_suffix(".git").unwrap_or(last).trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }

    // Keep only characters that are safe in a directory name on Windows.
    let sanitised: String = name
        .chars()
        .map(|ch| match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            other => other,
        })
        .collect();

    (!sanitised.is_empty()).then_some(sanitised)
}

/// Validate that a folder looks like something worth analyzing.
pub fn validate_repository(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("{} is a file, not a folder", path.display()));
    }

    let mut entries = std::fs::read_dir(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    if entries.next().is_none() {
        return Err(format!("{} is empty", path.display()));
    }

    Ok(())
}

/// Clone `url` into `parent/<repo-name>` and return the created directory.
///
/// Uses a shallow clone: analysis never looks at history, and this keeps the
/// wait short on large repositories.
pub fn clone_repository(url: &str, parent: &Path, name: &str) -> Result<PathBuf, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Enter a repository URL first.".to_string());
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("Enter a destination folder name.".to_string());
    }

    std::fs::create_dir_all(parent)
        .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;

    let destination = parent.join(name);
    if destination.exists() {
        return Err(format!("{} already exists", destination.display()));
    }

    let output = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--")
        .arg(url)
        .arg(&destination)
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "Git was not found on PATH. Install Git or open an existing folder instead.".to_string()
            } else {
                format!("could not run git: {err}")
            }
        })?;

    if !output.status.success() {
        // `git` writes progress to stderr; keep the last lines as the message.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail: Vec<&str> = stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .rev()
            .take(4)
            .collect();
        let detail: Vec<&str> = detail.into_iter().rev().collect();

        // A failed clone must not leave a half-populated directory behind.
        let _ = std::fs::remove_dir_all(&destination);

        return Err(if detail.is_empty() {
            format!("git clone failed ({})", output.status)
        } else {
            detail.join("\n")
        });
    }

    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_names_from_common_url_forms() {
        assert_eq!(
            repo_name_from_url("https://github.com/strapi/strapi.git").as_deref(),
            Some("strapi")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/rust-lang/rust").as_deref(),
            Some("rust")
        );
        assert_eq!(
            repo_name_from_url("git@github.com:org/project.git").as_deref(),
            Some("project")
        );
        assert_eq!(
            repo_name_from_url("https://github.com/org/repo.git/").as_deref(),
            Some("repo")
        );
    }

    #[test]
    fn rejects_empty_urls() {
        assert!(repo_name_from_url("").is_none());
        assert!(repo_name_from_url("   ").is_none());
        assert!(repo_name_from_url("https://github.com/").is_none());
    }

    #[test]
    fn sanitises_unsafe_characters() {
        let name = repo_name_from_url("https://example.com/a/bad:name").unwrap();
        assert!(!name.contains(':'));
    }

    #[test]
    fn validates_folders() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(validate_repository(dir.path()).is_err(), "empty folder is rejected");

        std::fs::write(dir.path().join("file.ts"), "").unwrap();
        assert!(validate_repository(dir.path()).is_ok());

        let file = dir.path().join("file.ts");
        assert!(validate_repository(&file).is_err(), "files are rejected");
        assert!(validate_repository(&dir.path().join("missing")).is_err());
    }
}

