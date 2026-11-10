//! `.gitignore`-compatible exclusion rules, powered by the same matcher ripgrep uses.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use serde::Serialize;

pub struct Rules {
    global: Option<Gitignore>,
    pair: Gitignore,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RuleError {
    pub line: usize,
    pub message: String,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub ignored: bool,
    pub rule: Option<String>,
    pub source: Option<String>,
}

fn build(text: &str, errors: &mut Vec<RuleError>) -> Gitignore {
    let mut b = GitignoreBuilder::new("");
    // NTFS is case-insensitive, so `thumbs.db` must match `Thumbs.db` on Windows.
    b.case_insensitive(cfg!(windows)).ok();
    for (i, line) in text.lines().enumerate() {
        if let Err(e) = b.add_line(None, line) {
            errors.push(RuleError { line: i + 1, message: e.to_string() });
        }
    }
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

impl Rules {
    pub fn new(global: Option<&str>, pair: &str) -> (Rules, Vec<RuleError>) {
        let mut errors = Vec::new();
        let global = global.map(|g| build(g, &mut errors));
        let pair = build(pair, &mut errors);
        (Rules { global, pair }, errors)
    }

    pub fn empty() -> Rules {
        Rules { global: None, pair: Gitignore::empty() }
    }

    /// Matches one path whose ancestors are already known not to be ignored (used while walking).
    #[inline]
    pub fn is_ignored_shallow(&self, rel: &str, name: &str, is_dir: bool) -> bool {
        if crate::fsutil::is_temp_name(name) {
            return true;
        }
        self.decide(|g| g.matched(rel, is_dir))
    }

    /// Matches a path checking every ancestor directory as well.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        if rel.is_empty() {
            return false;
        }
        if crate::fsutil::is_temp_name(crate::fsutil::last_component(rel)) {
            return true;
        }
        self.decide(|g| g.matched_path_or_any_parents(rel, is_dir))
    }

    /// Pair rules take precedence over global rules, so a pair can re-include something (`!x`)
    /// that the global list excludes.
    fn decide<'a>(&'a self, f: impl Fn(&'a Gitignore) -> Match<&'a ignore::gitignore::Glob>) -> bool {
        match f(&self.pair) {
            Match::Ignore(_) => true,
            Match::Whitelist(_) => false,
            Match::None => match &self.global {
                Some(g) => f(g).is_ignore(),
                None => false,
            },
        }
    }

    pub fn explain(&self, rel: &str, is_dir: bool) -> MatchResult {
        let rel = rel.trim_matches('/');
        if crate::fsutil::is_temp_name(crate::fsutil::last_component(rel)) {
            return MatchResult { ignored: true, rule: Some("(built-in temporary file)".into()), source: Some("built-in".into()) };
        }
        let describe = |m: Match<&ignore::gitignore::Glob>, src: &str| -> Option<MatchResult> {
            match m {
                Match::Ignore(g) => Some(MatchResult { ignored: true, rule: Some(g.original().to_string()), source: Some(src.into()) }),
                Match::Whitelist(g) => Some(MatchResult { ignored: false, rule: Some(g.original().to_string()), source: Some(src.into()) }),
                Match::None => None,
            }
        };
        if let Some(r) = describe(self.pair.matched_path_or_any_parents(rel, is_dir), "pair") {
            return r;
        }
        if let Some(g) = &self.global {
            if let Some(r) = describe(g.matched_path_or_any_parents(rel, is_dir), "global") {
                return r;
            }
        }
        MatchResult { ignored: false, rule: None, source: None }
    }
}

/// Escapes a path so it can be used literally in a gitignore pattern.
pub fn escape_pattern(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 2);
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '\\' | '!' | '#') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitignore_semantics() {
        let (r, errs) = Rules::new(Some("node_modules/\n*.log\n"), "!keep.log\n/build/\n");
        assert!(errs.is_empty());
        assert!(r.is_ignored("a/node_modules", true));
        assert!(r.is_ignored("a/node_modules/x/y.js", false));
        assert!(!r.is_ignored("a/node_modules", false)); // a *file* called node_modules is fine
        assert!(r.is_ignored("x/debug.log", false));
        assert!(!r.is_ignored("keep.log", false));
        assert!(r.is_ignored("build/out.bin", false));
        assert!(!r.is_ignored("src/build/out.bin", false));
        assert!(r.is_ignored("dir/.~sync-0001.tmp", false));
        let e = r.explain("x/debug.log", false);
        assert!(e.ignored && e.rule.as_deref() == Some("*.log"));
    }

    #[test]
    fn git_lock_default() {
        let (r, _) = Rules::new(Some(crate::config::DEFAULT_GLOBAL_IGNORE), "");
        assert!(r.is_ignored("proj/.git/index.lock", false));
        assert!(r.is_ignored("proj/.git/refs/heads/main.lock", false));
        assert!(!r.is_ignored("proj/.git/index", false));
        assert!(!r.is_ignored("proj/package.lock", false));
        assert!(r.is_ignored("web/node_modules/react/index.js", false));
    }

    #[test]
    fn escaping() {
        assert_eq!(escape_pattern("a[1]/b*"), "a\\[1\\]/b\\*");
    }
}
