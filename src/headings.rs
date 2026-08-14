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

        // Empty headings ("##") stay: markdown-it renders <h2 id=""> for
        // them, and skipping would shift every later dedup counter.
        let slug_source = clean_inline(&raw, true, &refs);
        let display = clean_inline(&raw, false, &refs).trim().to_string();

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

/// Headings of an HTML document, for the Navigator. Ported from
/// HeadingParser.extractHeadings(fromHTML:) @ a3ab7fd: prefer `<main>`,
/// else drop aside/nav/header/footer; always drop script/style/pre;
/// existing `id` attributes win, otherwise the text is slugified with
/// "heading" as the empty fallback.
pub fn extract_headings_html(html: &str) -> Vec<Heading> {
    let structural = first_tag_body(html, "main")
        .unwrap_or_else(|| remove_blocks(html, &["aside", "nav", "header", "footer"]));
    let cleaned = remove_blocks(&structural, &["script", "style", "pre"]);
    let cleaned_lower = cleaned.to_ascii_lowercase();

    let open_re = regex::Regex::new(r"(?is)<h([1-6])\b([^>]*)>").unwrap();
    let tag_re = regex::Regex::new(r"<[^>]+>").unwrap();
    let id_re =
        regex::Regex::new(r#"(?i)\bid\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#).unwrap();

    let mut id_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut results = Vec::new();

    for cap in open_re.captures_iter(&cleaned) {
        let level: u8 = cap[1].parse().unwrap_or(1);
        let attrs = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let body_start = cap.get(0).unwrap().end();
        // Rust regex has no backreferences — find the matching close by hand
        let close_pat = format!("</h{level}");
        let Some(rel) = cleaned_lower[body_start..].find(&close_pat) else {
            continue;
        };
        let body = &cleaned[body_start..body_start + rel];

        let text = decode_entities(&tag_re.replace_all(body, " "))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }

        let existing_id = id_re.captures(attrs).and_then(|c| {
            c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)).map(|m| m.as_str())
        });
        let id = match existing_id.filter(|id| !id.is_empty()) {
            Some(id) => {
                let id = decode_entities(id);
                *id_counts.entry(id.clone()).or_insert(0) += 1;
                id
            }
            None => {
                let slug = slugify(&text);
                let base = if slug.is_empty() { "heading".to_string() } else { slug };
                match id_counts.get_mut(&base) {
                    Some(count) => {
                        let id = format!("{base}-{count}");
                        *count += 1;
                        id
                    }
                    None => {
                        id_counts.insert(base.clone(), 1);
                        base
                    }
                }
            }
        };
        results.push(Heading { level, text, id });
    }
    results
}

fn first_tag_body(html: &str, tag: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>(.*?)</{tag}\s*>")).unwrap();
    re.captures(html).map(|c| c[1].to_string())
}

fn remove_blocks(html: &str, tags: &[&str]) -> String {
    let mut out = html.to_string();
    for tag in tags {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")).unwrap();
        out = re.replace_all(&out, "").to_string();
    }
    out
}

fn decode_entities(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' {
            if let Some((decoded, next)) = parse_entity(&chars, i) {
                out.push(decoded);
                i = next;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
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
        } else if is_js_whitespace(c) {
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

/// JS `\s`, not Rust `char::is_whitespace` — the classes differ: U+FEFF is
/// `\s` only in JS, U+0085 (NEL) is White_Space only in Rust. The slug must
/// match what the JS regex does.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' '
            | '\u{a0}' | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
            | '\u{feff}'
    )
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
    // an empty text is still a heading — "##" renders as <h2 id="">
    Some((level, text))
}

/// `(marker, run length, has info string)` when the line opens/closes a fence.
/// At most 3 leading spaces — 4+ is an indented code block (CommonMark), and
/// treating it as a fence would swallow every heading after it.
fn match_fence(line: &str) -> Option<(char, usize, bool)> {
    let mut spaces = 0;
    for c in line.chars() {
        if c == ' ' && spaces < 4 {
            spaces += 1;
        } else {
            break;
        }
    }
    if spaces > 3 {
        return None;
    }
    let trimmed = &line[spaces..];
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
            '&' => {
                if let Some((decoded, next)) = parse_entity(&chars, i) {
                    out.push(decoded);
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            '_' => {
                // markdown-it resolves _em_/__strong__ but disallows
                // intraword `_` emphasis — snake_case survives untouched
                if let Some((inner, next)) = parse_underscore_emphasis(&chars, i) {
                    out.push_str(&clean_inline(&inner, include_image_alt, refs));
                    i = next;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `&amp;` and friends — markdown-it's token content is entity-decoded.
fn parse_entity(chars: &[char], start: usize) -> Option<(char, usize)> {
    let semi = chars[start + 1..start + 1 + 32.min(chars.len() - start - 1)]
        .iter()
        .position(|&c| c == ';')?;
    let entity: String = chars[start + 1..start + 1 + semi].iter().collect();
    if entity.is_empty() || entity.contains(char::is_whitespace) {
        return None;
    }
    let decoded = match entity.as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let value = if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                entity.strip_prefix('#')?.parse::<u32>().ok()?
            };
            char::from_u32(value)?
        }
    };
    Some((decoded, start + 1 + semi + 1))
}

/// A balanced `_…_` / `__…__` pair at word boundaries. Returns the inner
/// text and the index after the closing run, or None when the run is
/// intraword (snake_case) or unbalanced (renders literally).
fn parse_underscore_emphasis(chars: &[char], start: usize) -> Option<(String, usize)> {
    let prev_alnum = start > 0 && chars[start - 1].is_alphanumeric();
    if prev_alnum {
        return None; // intraword — not emphasis in markdown-it
    }
    let open_len = chars[start..].iter().take_while(|&&c| c == '_').count();
    let after_open = start + open_len;
    match chars.get(after_open) {
        Some(c) if !c.is_whitespace() && *c != '_' => {}
        _ => return None, // "_ x" or trailing run — literal
    }
    // find a closing run: preceded by non-space, followed by non-alnum/end
    let mut j = after_open;
    while j < chars.len() {
        if chars[j] == '_' && !chars[j - 1].is_whitespace() {
            let close_len = chars[j..].iter().take_while(|&&c| c == '_').count();
            let after_close = j + close_len;
            let next_ok = chars
                .get(after_close)
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            if next_ok {
                let inner: String = chars[after_open..j].iter().collect();
                return Some((inner, after_close));
            }
            j = after_close;
        } else {
            j += 1;
        }
    }
    None
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
    fn html_headings_basics() {
        let html = r#"<html><body>
            <nav><h1>Site nav</h1></nav>
            <h1>Title</h1>
            <h2 id="custom-id">Has &amp; keeps its id</h2>
            <h2>Dup</h2>
            <h2>Dup</h2>
            <pre><h3>inside pre, ignored</h3></pre>
            <script>document.write("<h4>scripted</h4>")</script>
            <h3><em>Nested</em> markup</h3>
        </body></html>"#;
        let hs = extract_headings_html(html);
        let got: Vec<(u8, &str, &str)> =
            hs.iter().map(|h| (h.level, h.text.as_str(), h.id.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (1, "Title", "title"),
                (2, "Has & keeps its id", "custom-id"),
                (2, "Dup", "dup"),
                (2, "Dup", "dup-1"),
                (3, "Nested markup", "nested-markup"),
            ]
        );
    }

    #[test]
    fn html_main_tag_wins() {
        let html = r#"<header><h1>Chrome</h1></header>
            <main><h1>Real</h1></main>
            <footer><h2>Footer</h2></footer>"#;
        let hs = extract_headings_html(html);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].text, "Real");
    }

    #[test]
    fn fence_close_requires_no_info_string() {
        let md = "```text\n```python\n# still fenced\n```\n\n# real\n";
        let hs = extract_headings(md);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].id, "real");
    }
}
