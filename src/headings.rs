//! Heading extraction for the Navigator — files not loaded in any webview.
//! Ported from MoremaidApp Sources/FileBrowser/HeadingParser.swift @ a3ab7fd,
//! with two fidelity fixes over the reference: inline markdown in heading text
//! (links, images, code, emphasis) is resolved the way markdown-it's token
//! stream resolves it, and trailing `#`s are only stripped per CommonMark.
//!
//! The slug logic MUST stay byte-identical to `slugify` in web/js/page.js —
//! the shared fixture tests/fixtures/slugs.json runs against both sides
//! (HANDOFF §9.3). The id is derived from the token *content* (which includes
//! image alt text); the display text mirrors the DOM's textContent (which
//! does not).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub id: String,
}

pub fn extract_headings(markdown: &str) -> Vec<Heading> {
    let refs = collect_link_definitions(markdown);
    let mut in_fence = false;
    let mut fence_marker = '`';
    let mut fence_count = 0usize;
    let mut id_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut results = Vec::new();

    for line in markdown.split('\n') {
        if let Some((marker, count, has_info)) = match_fence(line) {
            if !in_fence {
                in_fence = true;
                fence_marker = marker;
                fence_count = count;
                continue;
            } else if marker == fence_marker && count >= fence_count && !has_info {
                // a closing fence has no info string (CommonMark)
                in_fence = false;
                continue;
            }
        }
        if in_fence {
            continue;
        }

        let Some((level, raw)) = parse_heading(line) else {
            continue;
        };

        let slug_source = clean_inline(&raw, true, &refs);
        let display = clean_inline(&raw, false, &refs).trim().to_string();
        if display.is_empty() && slug_source.trim().is_empty() {
            continue;
        }

        let base = slugify(&slug_source);
        // Dedup exactly as page.js does: first occurrence keeps the bare
        // slug, later ones get -1, -2, …
        let id = match id_counts.get_mut(&base) {
            Some(count) => {
                let id = format!("{base}-{count}");
                *count += 1;
                id
            }
            None => {
                id_counts.insert(base.clone(), 1);
                base
            }
        };
        results.push(Heading { level, text: display, id });
    }
    results
}

/// Byte-identical port of page.js `slugify`:
///   s.toLowerCase().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-')
///    .replace(/-+/g, '-').replace(/^-|-$/g, '')
/// where JS `\w` is ASCII `[A-Za-z0-9_]`.
pub fn slugify(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut pending_dash = false;
    for c in lower.chars() {
        let keep = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-';
        if keep {
            if pending_dash {
                if !out.is_empty() {
                    out.push('-');
                }
                pending_dash = false;
            }
            out.push(c);
        } else if c.is_whitespace() {
            pending_dash = true;
        }
        // any other char: removed, contributes nothing (not even a break)
    }
    // collapse runs of '-' (literal hyphens count too) and trim the ends
    let mut collapsed = String::with_capacity(out.len());
    let mut last_dash = false;
    for c in out.chars() {
        if c == '-' {
            if !last_dash {
                collapsed.push(c);
            }
            last_dash = true;
        } else {
            collapsed.push(c);
            last_dash = false;
        }
    }
    collapsed.trim_matches('-').to_string()
}

/// ATX headings only (matches the reference; setext is a known gap).
fn parse_heading(line: &str) -> Option<(u8, String)> {
    let chars: Vec<char> = line.chars().collect();
    // up to 3 leading spaces (CommonMark); 4+ is indented code
    let mut i = 0;
    while i < 3 && i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    if i < chars.len() && chars[i] == ' ' {
        return None;
    }
    let mut level = 0u8;
    while i < chars.len() && chars[i] == '#' && level < 6 {
        level += 1;
        i += 1;
    }
    if level == 0 {
        return None;
    }
    // "###foo" and "####### x" are not headings
    if i < chars.len() && chars[i] != ' ' && chars[i] != '\t' {
        return None;
    }
    let text: String = chars[i..].iter().collect();
    let mut text = text.trim().to_string();
    // CommonMark closing sequence: trailing #s strip only when preceded by
    // whitespace (or when #s are the whole text) — "C#" keeps its hash.
    let trimmed = text.trim_end_matches('#');
    if trimmed.len() != text.len()
        && (trimmed.is_empty() || trimmed.ends_with(' ') || trimmed.ends_with('\t'))
    {
        text = trimmed.trim_end().to_string();
    }
    if text.is_empty() {
        return None;
    }
    Some((level, text))
}

/// `(marker, run length, has info string)` when the line opens/closes a fence.
fn match_fence(line: &str) -> Option<(char, usize, bool)> {
    let trimmed = line.trim_start_matches(' ');
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|&c| c == first).count();
    if count < 3 {
        return None;
    }
    let rest: String = trimmed.chars().skip(count).collect();
    Some((first, count, !rest.trim().is_empty()))
}

/// Link reference definitions (`[label]: dest`), labels normalized the way
/// CommonMark matches them (case-fold, whitespace collapsed). Needed because
/// `[text][ref]` resolves to just "text" only when `ref` is defined —
/// undefined references render literally, brackets and all.
fn collect_link_definitions(markdown: &str) -> std::collections::HashSet<String> {
    let mut refs = std::collections::HashSet::new();
    let mut in_fence = false;
    let mut fence_marker = '`';
    let mut fence_count = 0usize;
    for line in markdown.split('\n') {
        if let Some((marker, count, has_info)) = match_fence(line) {
            if !in_fence {
                in_fence = true;
                fence_marker = marker;
                fence_count = count;
                continue;
            } else if marker == fence_marker && count >= fence_count && !has_info {
                in_fence = false;
                continue;
            }
        }
        if in_fence {
            continue;
        }
        let trimmed = line.trim_start();
        if line.len() - trimmed.len() > 3 || !trimmed.starts_with('[') {
            continue;
        }
        if let Some(close) = trimmed.find(']') {
            if trimmed[close + 1..].starts_with(':') {
                refs.insert(normalize_ref(&trimmed[1..close]));
            }
        }
    }
    refs
}

fn normalize_ref(label: &str) -> String {
    label
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolve inline markdown the way markdown-it's token content does:
/// `[text](url)` → text, `[text][ref]` → text (defined refs only),
/// `![alt](url)` → alt (slug) / nothing (display), `<autolink>` → inner,
/// `\X` → X, and the `` ` ``/`*`/`~` markers dropped.
fn clean_inline(
    text: &str,
    include_image_alt: bool,
    refs: &std::collections::HashSet<String>,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((label, next)) = parse_link(&chars, i + 1, refs) {
                    if include_image_alt {
                        out.push_str(&clean_inline(&label, include_image_alt, refs));
                    }
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '[' => {
                if let Some((label, next)) = parse_link(&chars, i, refs) {
                    out.push_str(&clean_inline(&label, include_image_alt, refs));
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '<' => {
                if let Some((inner, next)) = parse_autolink(&chars, i) {
                    out.push_str(&inner);
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '`' | '*' | '~' => i += 1,
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// At `chars[start] == '['`: match `[label](dest)`, `[label][ref]` (defined
/// refs only) or `[label][]`, returning the raw label and the index after
/// the link. Undefined full-form references return None — markdown-it
/// renders those literally, brackets and all.
fn parse_link(
    chars: &[char],
    start: usize,
    refs: &std::collections::HashSet<String>,
) -> Option<(String, usize)> {
    let mut depth = 0usize;
    let mut close = None;
    for (offset, &c) in chars[start..].iter().enumerate() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(start + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let label = || chars[start + 1..close].iter().collect::<String>();

    match chars.get(close + 1) {
        Some(&'(') => {
            let mut paren_depth = 0usize;
            for (offset, &c) in chars[close + 1..].iter().enumerate() {
                match c {
                    '(' => paren_depth += 1,
                    ')' => {
                        paren_depth -= 1;
                        if paren_depth == 0 {
                            return Some((label(), close + 1 + offset + 1));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        Some(&'[') => {
            let ref_close = chars[close + 2..].iter().position(|&c| c == ']')?;
            let ref_label: String = chars[close + 2..close + 2 + ref_close].iter().collect();
            // collapsed `[label][]` resolves to the label either way; the
            // full form only when the reference is defined
            if ref_label.is_empty() || refs.contains(&normalize_ref(&ref_label)) {
                Some((label(), close + 2 + ref_close + 1))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `<scheme:...>` or `<user@host>` with no whitespace inside.
fn parse_autolink(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut end = None;
    for (offset, &c) in chars[start + 1..].iter().enumerate() {
        if c == '>' {
            end = Some(start + 1 + offset);
            break;
        }
        if c.is_whitespace() || c == '<' {
            return None;
        }
    }
    let end = end?;
    let inner: String = chars[start + 1..end].iter().collect();
    if inner.contains(':') || inner.contains('@') {
        Some((inner, end + 1))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §9.3 contract: the same fixture that tests/slugs.test.js runs
    /// against the shipped JS pipeline.
    #[test]
    fn shared_slug_fixture() {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/slugs.json"),
        )
        .expect("fixture");
        let fixture: serde_json::Value = serde_json::from_str(&raw).expect("valid json");

        for doc in fixture["documents"].as_array().expect("documents") {
            let name = doc["name"].as_str().unwrap();
            let markdown = doc["markdown"].as_str().unwrap();
            let skip_texts = doc["skipTexts"].as_bool().unwrap_or(false);
            let got = extract_headings(markdown);
            let expected = doc["headings"].as_array().unwrap();

            let got_ids: Vec<(u64, &str)> =
                got.iter().map(|h| (h.level as u64, h.id.as_str())).collect();
            let expected_ids: Vec<(u64, &str)> = expected
                .iter()
                .map(|h| (h["level"].as_u64().unwrap(), h["id"].as_str().unwrap()))
                .collect();
            assert_eq!(got_ids, expected_ids, "ids mismatch in {name:?}");

            if !skip_texts {
                let got_texts: Vec<&str> = got.iter().map(|h| h.text.as_str()).collect();
                let expected_texts: Vec<&str> = expected
                    .iter()
                    .map(|h| h["text"].as_str().unwrap())
                    .collect();
                assert_eq!(got_texts, expected_texts, "texts mismatch in {name:?}");
            }
        }
    }

    #[test]
    fn slugify_matches_js_semantics() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("a -- b"), "a-b");
        assert_eq!(slugify("--x--"), "x");
        assert_eq!(slugify("???"), "");
        assert_eq!(slugify("snake_case ok"), "snake_case-ok");
    }

    #[test]
    fn fence_close_requires_no_info_string() {
        let md = "```text\n```python\n# still fenced\n```\n\n# real\n";
        let hs = extract_headings(md);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].id, "real");
    }
}
