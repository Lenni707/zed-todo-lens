use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::gitignore::Gitignore;
use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkState};

use crate::config::Config;
use crate::scanner::{is_prose_path, Scanner, Tag};

/// All tags in the workspace, keyed by absolute file path.
#[derive(Default)]
pub struct Index {
    files: HashMap<PathBuf, Vec<Tag>>,
}

/// A tag together with the file it was found in.
pub struct Hit<'a> {
    pub path: &'a Path,
    pub tag: &'a Tag,
}

impl Index {
    pub fn set(&mut self, path: PathBuf, tags: Vec<Tag>) {
        if tags.is_empty() {
            self.files.remove(&path);
        } else {
            self.files.insert(path, tags);
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    pub fn get(&self, path: &Path) -> &[Tag] {
        self.files.get(path).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn files(&self) -> impl Iterator<Item = (&PathBuf, &Vec<Tag>)> {
        self.files.iter()
    }

    /// Tags whose `"TAG: text  relative/path"` matches `query` as a case-insensitive
    /// subsequence (so `tdo pars` finds `TODO: fix parser`). Sorted by path, then line.
    pub fn search<'a>(&'a self, query: &str, tags: &[String], roots: &[PathBuf], limit: usize) -> Vec<Hit<'a>> {
        let query: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        let mut hits: Vec<Hit> = self
            .files
            .iter()
            .flat_map(|(path, file_tags)| file_tags.iter().map(move |tag| Hit { path, tag }))
            .filter(|hit| {
                if query.is_empty() {
                    return true;
                }
                let haystack = format!(
                    "{}: {} {}",
                    tags.get(hit.tag.kind).map(String::as_str).unwrap_or(""),
                    hit.tag.text,
                    relative_path(hit.path, roots).display()
                );
                is_subsequence(&query, &haystack.to_lowercase())
            })
            .collect();
        hits.sort_by(|a, b| a.path.cmp(b.path).then(a.tag.line.cmp(&b.tag.line)));
        hits.truncate(limit);
        hits
    }
}

fn is_subsequence(needle: &[char], haystack: &str) -> bool {
    let mut needle = needle.iter().peekable();
    for c in haystack.chars() {
        if needle.peek() == Some(&&c) {
            needle.next();
        }
    }
    needle.peek().is_none()
}

pub fn relative_path<'a>(path: &'a Path, roots: &[PathBuf]) -> &'a Path {
    roots
        .iter()
        .find_map(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
}

/// Reads and scans one file from disk. Returns `None` for unreadable, binary or oversized files.
pub fn scan_file(path: &Path, scanner: &Scanner, config: &Config) -> Option<Vec<Tag>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > config.max_file_size_kb * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes[..bytes.len().min(8192)].contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(scan_text(path, &text, scanner, config))
}

pub fn scan_text(path: &Path, text: &str, scanner: &Scanner, config: &Config) -> Vec<Tag> {
    let comment_only = config.comment_only && !is_prose_path(&path.to_string_lossy());
    scanner.scan(text, comment_only)
}

/// Builds a file walker that honours `.gitignore`, skips hidden files and applies `exclude`.
fn walker(roots: &[PathBuf], config: &Config) -> Option<WalkBuilder> {
    let (first, rest) = roots.split_first()?;
    let mut builder = WalkBuilder::new(first);
    for root in rest {
        builder.add(root);
    }
    if !config.exclude.is_empty() {
        let mut overrides = OverrideBuilder::new(first);
        for pattern in &config.exclude {
            // In override syntax a leading `!` means "ignore".
            let _ = overrides.add(&format!("!{pattern}"));
        }
        if let Ok(overrides) = overrides.build() {
            builder.overrides(overrides);
        }
    }
    builder.hidden(true).git_ignore(true).require_git(false);
    Some(builder)
}

/// Scans every file under `roots` in parallel.
pub fn scan_workspace(roots: &[PathBuf], scanner: &Scanner, config: &Config) -> Vec<(PathBuf, Vec<Tag>)> {
    let Some(builder) = walker(roots, config) else {
        return Vec::new();
    };
    let results = Mutex::new(Vec::new());
    builder.build_parallel().run(|| {
        Box::new(|entry| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if entry.file_type().is_some_and(|t| t.is_file()) {
                if let Some(tags) = scan_file(entry.path(), scanner, config) {
                    if !tags.is_empty() {
                        results.lock().unwrap().push((entry.into_path(), tags));
                    }
                }
            }
            WalkState::Continue
        })
    });
    results.into_inner().unwrap()
}

/// Decides whether a file reported by the file watcher should be (re)scanned: it has to be
/// inside a root, not hidden, not excluded, and not ignored by the root's `.gitignore`.
pub struct PathFilter {
    roots: Vec<(PathBuf, Gitignore, Override)>,
}

impl PathFilter {
    pub fn new(roots: &[PathBuf], config: &Config) -> Self {
        let roots = roots
            .iter()
            .map(|root| {
                let (gitignore, _) = Gitignore::new(root.join(".gitignore"));
                let mut overrides = OverrideBuilder::new(root);
                for pattern in &config.exclude {
                    let _ = overrides.add(&format!("!{pattern}"));
                }
                let overrides = overrides.build().unwrap_or_else(|_| Override::empty());
                (root.clone(), gitignore, overrides)
            })
            .collect();
        Self { roots }
    }

    pub fn is_tracked(&self, path: &Path) -> bool {
        let Some((root, gitignore, overrides)) = self.roots.iter().find(|(r, ..)| path.starts_with(r)) else {
            return false;
        };
        let rel = path.strip_prefix(root).unwrap_or(path);
        if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
            return false;
        }
        if overrides.matched(path, false).is_ignore() {
            return false;
        }
        !gitignore.matched_path_or_any_parents(path, false).is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_and_respects_gitignore() {
        let dir = std::env::temp_dir().join(format!("todo-lens-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("ignored")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::write(dir.join("src/a.rs"), "// TODO: a\nfn x() {}\n// BUG: b\n").unwrap();
        std::fs::write(dir.join("notes.md"), "NOTE: prose\n").unwrap();
        std::fs::write(dir.join("ignored/c.rs"), "// TODO: hidden\n").unwrap();

        let config = Config::default();
        let scanner = Scanner::new(&config.tags);
        let roots = vec![dir.clone()];
        let mut index = Index::default();
        for (path, tags) in scan_workspace(&roots, &scanner, &config) {
            index.set(path, tags);
        }

        let all = index.search("", &config.tags, &roots, 100);
        let found: Vec<_> = all.iter().map(|h| h.tag.text.as_str()).collect();
        assert_eq!(found, vec!["prose", "a", "b"]);

        let hits = index.search("bug", &config.tags, &roots, 100);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].tag.line, 2);

        // Fuzzy subsequence on path too.
        assert_eq!(index.search("todo src/a", &config.tags, &roots, 100).len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tracked_paths() {
        let dir = std::env::temp_dir().join(format!("todo-lens-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n*.log\n").unwrap();
        let config = Config {
            exclude: vec!["vendor/**".into()],
            ..Config::default()
        };
        let filter = PathFilter::new(&[dir.clone()], &config);
        assert!(filter.is_tracked(&dir.join("src/main.rs")));
        assert!(!filter.is_tracked(&dir.join(".git/HEAD")));
        assert!(!filter.is_tracked(&dir.join("vendor/x.rs")));
        assert!(!filter.is_tracked(&dir.join("target/debug/x.rs")));
        assert!(!filter.is_tracked(&dir.join("out.log")));
        assert!(!filter.is_tracked(Path::new("/elsewhere/x.rs")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
