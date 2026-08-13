//! Finding things (§1, M3): full-text search over the scanned tree with the
//! grep-* crates — the same engine as ripgrep, in-process, no subprocess —
//! and fuzzy filename ranking for Quick Open via nucleo-matcher.

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::SearcherBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher as NucleoMatcher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line_number: u64,
    pub line: String,
    /// byte span of the first query occurrence within `line`
    pub span: (usize, usize),
}

/// Escape a literal query for the regex engine. Over-escaping is harmless;
/// under-escaping turns "a+b" into a different search.
fn escape_literal(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for c in query.chars() {
        if c.is_ascii() && !c.is_ascii_alphanumeric() && c != ' ' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Search `files` for a literal, case-insensitive `query`, invoking
/// `on_match` per matching line (with the first match's span). Returns early
/// if `on_match` returns false (result cap) or the generation moves on
/// (query changed / overlay closed).
pub fn search_into(
    files: &[PathBuf],
    query: &str,
    generation: &Arc<AtomicU64>,
    my_generation: u64,
    mut on_match: impl FnMut(SearchMatch) -> bool,
) {
    if query.is_empty() {
        return;
    }
    let Ok(matcher) = RegexMatcherBuilder::new()
        .case_insensitive(true)
        .build(&escape_literal(query))
    else {
        return;
    };
    let mut searcher = SearcherBuilder::new().line_number(true).build();

    let mut stop = false;
    for path in files {
        if stop || generation.load(Ordering::Relaxed) != my_generation {
            return;
        }
        let _ = searcher.search_path(
            &matcher,
            path,
            UTF8(|line_number, line| {
                let span = matcher
                    .find(line.as_bytes())
                    .ok()
                    .flatten()
                    .map(|m| (m.start(), m.end()))
                    .unwrap_or((0, 0));
                let keep_going = on_match(SearchMatch {
                    path: path.clone(),
                    line_number,
                    line: line.trim_end().to_string(),
                    span,
                });
                if !keep_going {
                    stop = true;
                }
                Ok(keep_going)
            }),
        );
    }
}

/// Rank candidate paths against a fuzzy query. Returns indices into
/// `candidates`, best match first. Candidates are (display, haystack)
/// where haystack is the relative path string that gets matched.
pub fn fuzzy_rank(candidates: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..candidates.len()).collect();
    }
    let mut matcher = NucleoMatcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut scored: Vec<(u32, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, cand)| {
            let haystack = nucleo_matcher::Utf32String::from(cand.as_str());
            pattern
                .score(haystack.slice(..), &mut matcher)
                .map(|score| (score, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("moremaid-search-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn collect(files: &[PathBuf], query: &str) -> Vec<SearchMatch> {
        let generation = Arc::new(AtomicU64::new(1));
        let mut out = Vec::new();
        search_into(files, query, &generation, 1, |m| {
            out.push(m);
            true
        });
        out
    }

    #[test]
    fn finds_case_insensitive_literal_with_span() {
        let root = tempdir("literal");
        let a = root.join("a.md");
        fs::write(&a, "# Title\n\nThe Mermaid diagram cache.\n").unwrap();
        let matches = collect(&[a.clone()], "mermaid");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 3);
        assert_eq!(matches[0].line, "The Mermaid diagram cache.");
        assert_eq!(&matches[0].line[matches[0].span.0..matches[0].span.1], "Mermaid");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn regex_metacharacters_are_literal() {
        let root = tempdir("meta");
        let a = root.join("a.md");
        fs::write(&a, "value is a+b (not ab)\nplain aab here\n").unwrap();
        let matches = collect(&[a.clone()], "a+b");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn result_cap_stops_search() {
        let root = tempdir("cap");
        let a = root.join("a.md");
        fs::write(&a, "hit\nhit\nhit\nhit\n").unwrap();
        let generation = Arc::new(AtomicU64::new(1));
        let mut n = 0;
        search_into(&[a.clone()], "hit", &generation, 1, |_| {
            n += 1;
            n < 2
        });
        assert_eq!(n, 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_generation_aborts() {
        let root = tempdir("gen");
        let a = root.join("a.md");
        fs::write(&a, "hit\n").unwrap();
        let generation = Arc::new(AtomicU64::new(2)); // already moved on
        let mut n = 0;
        search_into(&[a.clone()], "hit", &generation, 1, |_| {
            n += 1;
            true
        });
        assert_eq!(n, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fuzzy_ranking_prefers_tighter_matches() {
        let candidates = vec![
            "docs/architecture/decisions.md".to_string(),
            "readme.md".to_string(),
            "docs/readme-first.md".to_string(),
        ];
        let ranked = fuzzy_rank(&candidates, "readme");
        assert_eq!(ranked[0], 1, "exact filename should rank first");
        assert!(ranked.contains(&2));
        assert!(!ranked.contains(&0), "no 'readme' subsequence in candidate 0");

        let all = fuzzy_rank(&candidates, "");
        assert_eq!(all.len(), 3, "empty query returns everything");
    }
}
