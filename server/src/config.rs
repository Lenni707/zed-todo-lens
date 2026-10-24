use serde::Deserialize;
use serde_json::Value;

pub const DEFAULT_TAGS: [&str; 4] = ["TODO", "BUG", "FIXME", "NOTE"];
pub const MAX_TAGS: usize = 32;

/// User settings, read from `lsp."todo-lens".settings` in Zed's settings.json.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Keywords to look for. Matching is case-sensitive.
    pub tags: Vec<String>,
    /// Only match tags that appear inside a comment (ignored for prose files like Markdown).
    pub comment_only: bool,
    /// Also report every tag as a hint diagnostic (shows up in Project Diagnostics).
    pub diagnostics: bool,
    /// Glob patterns (gitignore syntax) to leave out of the workspace scan.
    pub exclude: Vec<String>,
    /// Files bigger than this are not scanned.
    pub max_file_size_kb: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tags: DEFAULT_TAGS.iter().map(|s| s.to_string()).collect(),
            comment_only: true,
            diagnostics: false,
            exclude: Vec::new(),
            max_file_size_kb: 1024,
        }
    }
}

impl Config {
    /// Parses settings sent by the client. Accepts either the settings object itself
    /// or one wrapped in a `"todo-lens"` key. Anything unparsable falls back to defaults.
    pub fn from_value(value: Option<&Value>) -> Self {
        let Some(mut value) = value.filter(|v| v.is_object()) else {
            return Self::default();
        };
        if let Some(inner) = value.get("todo-lens").filter(|v| v.is_object()) {
            value = inner;
        }
        let mut config: Config = serde_json::from_value(value.clone()).unwrap_or_default();
        config.tags.retain(|t| !t.trim().is_empty());
        config.tags.dedup();
        // Each tag is one bit in the semantic token modifier bitset.
        config.tags.truncate(MAX_TAGS);
        if config.tags.is_empty() {
            config.tags = Self::default().tags;
        }
        config
    }

    /// The semantic token modifier for a tag, e.g. `TODO` -> `todo`.
    pub fn modifier(tag: &str) -> String {
        tag.to_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_when_missing() {
        assert_eq!(Config::from_value(None), Config::default());
        assert_eq!(Config::from_value(Some(&json!(null))), Config::default());
    }

    #[test]
    fn partial_settings() {
        let c = Config::from_value(Some(&json!({ "tags": ["TODO", "HACK"], "diagnostics": true })));
        assert_eq!(c.tags, vec!["TODO", "HACK"]);
        assert!(c.diagnostics);
        assert!(c.comment_only);
    }

    #[test]
    fn wrapped_settings() {
        let c = Config::from_value(Some(&json!({ "todo-lens": { "comment_only": false } })));
        assert!(!c.comment_only);
    }

    #[test]
    fn empty_tags_fall_back() {
        let c = Config::from_value(Some(&json!({ "tags": [] })));
        assert_eq!(c.tags, Config::default().tags);
    }
}
