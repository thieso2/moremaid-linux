//! Omarchy palette: read `colors.toml` synchronously before first paint (§6.3),
//! derive the document's CSS custom properties and Mermaid theme variables.
//! Missing keys fall back per-key; a missing file falls back to a built-in
//! palette chosen by the light/dark bit. Never fail to render over a colour.

use std::collections::HashMap;
use std::path::PathBuf;

pub const FONT_BODY: &str = r#""iA Writer Quattro S", "Noto Sans", sans-serif"#;
/// Bare generic — resolves to the `omarchy font` choice via fontconfig (§6.4).
pub const FONT_CODE: &str = "monospace";

const PALETTE_KEYS: &[&str] = &[
    "accent", "selection", "muted",
    "background", "dark_background", "darker_background", "lighter_background",
    "foreground", "dark_foreground", "light_foreground", "bright_foreground",
    "red", "yellow", "orange", "green", "cyan", "blue", "magenta", "brown",
    "bright_red", "bright_yellow", "bright_green", "bright_cyan", "bright_blue",
    "bright_magenta",
];

const FALLBACK_DARK: &[(&str, &str)] = &[
    ("accent", "#62a0ea"), ("selection", "#32363c"), ("muted", "#454950"),
    ("background", "#1e1e22"), ("dark_background", "#17171a"),
    ("darker_background", "#101013"), ("lighter_background", "#2a2a30"),
    ("foreground", "#d8d8d4"), ("dark_foreground", "#8b8b88"),
    ("light_foreground", "#e4e4e0"), ("bright_foreground", "#f2f2ef"),
    ("red", "#f66151"), ("yellow", "#f8e45c"), ("orange", "#ffbe6f"),
    ("green", "#8ff0a4"), ("cyan", "#76c7c0"), ("blue", "#62a0ea"),
    ("magenta", "#dc8add"), ("brown", "#b5835a"),
    ("bright_red", "#ff7b6f"), ("bright_yellow", "#f9f06b"),
    ("bright_green", "#a7f4b8"), ("bright_cyan", "#93ddd5"),
    ("bright_blue", "#80b4f0"), ("bright_magenta", "#e8a2e8"),
];

const FALLBACK_LIGHT: &[(&str, &str)] = &[
    ("accent", "#1c71d8"), ("selection", "#cfe1f5"), ("muted", "#c0bfbc"),
    ("background", "#ffffff"), ("dark_background", "#fafafa"),
    ("darker_background", "#f2f1ef"), ("lighter_background", "#fcfcfc"),
    ("foreground", "#333333"), ("dark_foreground", "#77767b"),
    ("light_foreground", "#241f31"), ("bright_foreground", "#1a1a1a"),
    ("red", "#c01c28"), ("yellow", "#a08000"), ("orange", "#e66100"),
    ("green", "#26a269"), ("cyan", "#218787"), ("blue", "#1c71d8"),
    ("magenta", "#813d9c"), ("brown", "#865e3c"),
    ("bright_red", "#e01b24"), ("bright_yellow", "#b89000"),
    ("bright_green", "#2ec27e"), ("bright_cyan", "#33a7a7"),
    ("bright_blue", "#3584e4"), ("bright_magenta", "#9141ac"),
];

#[derive(Clone)]
pub struct Palette {
    pub dark: bool,
    map: HashMap<String, String>,
}

fn theme_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    vec![
        // note: .local/state, not .config — and the legacy path as fallback
        PathBuf::from(&home).join(".local/state/omarchy/current/theme/colors.toml"),
        PathBuf::from(&home).join(".config/omarchy/current/theme/colors.toml"),
    ]
}

impl Palette {
    /// `style_dark_hint` decides the fallback palette when no colors.toml
    /// exists (plain Arch + Hyprland) — the one bit AdwStyleManager gives us.
    /// It is only *evaluated* in that case: on Omarchy the hint's portal
    /// round-trip (~200 ms) must never sit on the cold-start path.
    pub fn load(style_dark_hint: impl FnOnce() -> bool) -> Palette {
        for path in theme_paths() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(table) = text.parse::<toml::Table>() {
                    return Self::from_table(&table);
                }
            }
        }
        Self::fallback(style_dark_hint())
    }

    pub fn fallback(dark: bool) -> Palette {
        let base = if dark { FALLBACK_DARK } else { FALLBACK_LIGHT };
        Palette {
            dark,
            map: base.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn from_table(table: &toml::Table) -> Palette {
        let dark = table
            .get("mode")
            .and_then(|v| v.as_str())
            .map(|m| m != "light")
            .unwrap_or(true);
        let mut palette = Self::fallback(dark);
        for key in PALETTE_KEYS {
            if let Some(value) = table.get(*key).and_then(|v| v.as_str()) {
                if parse_hex(value).is_some() {
                    palette.map.insert(key.to_string(), value.to_string());
                }
            }
        }
        palette
    }

    pub fn get(&self, key: &str) -> &str {
        self.map
            .get(key)
            .map(String::as_str)
            .unwrap_or(if self.dark { "#ffffff" } else { "#000000" })
    }

    /// Raised surface: code blocks, tables (§6.3 role contract).
    fn raised(&self) -> &str {
        if self.dark {
            self.get("lighter_background")
        } else {
            self.get("darker_background")
        }
    }

    /// The `:root` custom-property block interpolated into every page.
    pub fn css_block(&self, font_size_px: u32) -> String {
        let accent = self.get("accent");
        let raised = self.raised();
        let (r, g, b) = parse_hex(self.get("background")).unwrap_or((0, 0, 0));
        format!(
            r#":root {{
    --font-body: {font_body};
    --font-heading: {font_body};
    --font-code: {font_code};
    --font-size-base: {font_size_px}px;
    --line-height: 1.6;
    --paragraph-spacing: 1em;
    --max-width: 800px;
    --text-align: left;
    --bg-color: {bg};
    --bg-color-rgb: {r}, {g}, {b};
    --text-color: {fg};
    --heading-color: {heading};
    --heading2-color: {heading2};
    --border-color: {muted};
    --code-bg: {raised};
    --code-color: {red};
    --link-color: {accent};
    --accent-color: {accent};
    --accent-dim: {accent_dim};
    --selection-color: {selection};
    --blockquote-color: {dim_fg};
    --table-header-bg: {raised};
    --table-border: {muted};
    --file-info-bg: {raised};
    --file-info-color: {dim_fg};
    --secondary-text: {dim_fg};
    --error-color: {red};
    --mermaid-btn-bg: {accent_btn};
    --mermaid-btn-hover: {accent};
    --ansi-red: {red};
    --ansi-yellow: {yellow};
    --ansi-orange: {orange};
    --ansi-green: {green};
    --ansi-cyan: {cyan};
    --ansi-blue: {blue};
    --ansi-magenta: {magenta};
    --ansi-bright-red: {bright_red};
    --ansi-bright-yellow: {bright_yellow};
    --ansi-bright-green: {bright_green};
    --ansi-bright-cyan: {bright_cyan};
    --ansi-bright-blue: {bright_blue};
    --ansi-bright-magenta: {bright_magenta};
}}
"#,
            font_body = FONT_BODY,
            font_code = FONT_CODE,
            bg = self.get("background"),
            fg = self.get("foreground"),
            heading = self.get("bright_foreground"),
            heading2 = self.get("light_foreground"),
            muted = self.get("muted"),
            raised = raised,
            red = self.get("red"),
            accent = accent,
            accent_dim = with_alpha(accent, 0x66),
            accent_btn = with_alpha(accent, 0xcc),
            selection = self.get("selection"),
            dim_fg = self.get("dark_foreground"),
            yellow = self.get("yellow"),
            orange = self.get("orange"),
            green = self.get("green"),
            cyan = self.get("cyan"),
            blue = self.get("blue"),
            magenta = self.get("magenta"),
            bright_red = self.get("bright_red"),
            bright_yellow = self.get("bright_yellow"),
            bright_green = self.get("bright_green"),
            bright_cyan = self.get("bright_cyan"),
            bright_blue = self.get("bright_blue"),
            bright_magenta = self.get("bright_magenta"),
        )
    }

    /// Mermaid theme variables (theme 'base') derived from the same roles, so
    /// prose, code and diagrams are one palette. The explicit fontFamily is
    /// load-bearing — WebKitGTK's SVG renderer can drop every label when
    /// Mermaid uses generic families (§6.4, the Mermaid font trap).
    pub fn mermaid_vars_json(&self) -> String {
        let raised = self.raised();
        format!(
            r#"{{"fontFamily": "\"iA Writer Quattro S\", \"Noto Sans\", sans-serif", "darkMode": {dark}, "background": "{bg}", "primaryColor": "{accent}", "primaryTextColor": "{bg}", "primaryBorderColor": "{muted}", "secondaryColor": "{raised}", "secondaryTextColor": "{fg}", "tertiaryColor": "{bg}", "tertiaryTextColor": "{fg}", "lineColor": "{fg}", "textColor": "{fg}", "mainBkg": "{accent}", "clusterBkg": "{raised}", "clusterBorder": "{muted}", "titleColor": "{bright_fg}", "edgeLabelBackground": "{bg}", "noteBkgColor": "{raised}", "noteTextColor": "{fg}", "noteBorderColor": "{muted}", "errorBkgColor": "{red}", "errorTextColor": "{bg}"}}"#,
            dark = self.dark,
            bg = self.get("background"),
            accent = self.get("accent"),
            muted = self.get("muted"),
            raised = raised,
            fg = self.get("foreground"),
            bright_fg = self.get("bright_foreground"),
            red = self.get("red"),
        )
    }
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn with_alpha(hex: &str, alpha: u8) -> String {
    if parse_hex(hex).is_some() {
        format!("{hex}{alpha:02x}")
    } else {
        hex.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_covers_all_keys() {
        for dark in [true, false] {
            let p = Palette::fallback(dark);
            for key in PALETTE_KEYS {
                assert!(p.map.contains_key(*key), "missing {key} in fallback");
            }
        }
    }

    #[test]
    fn per_key_fallback() {
        let table: toml::Table = "mode = \"dark\"\naccent = \"#123456\"".parse().unwrap();
        let p = Palette::from_table(&table);
        assert_eq!(p.get("accent"), "#123456");
        // unset key falls back to the built-in dark palette, not to nothing
        assert_eq!(p.get("green"), "#8ff0a4");
    }

    #[test]
    fn invalid_hex_rejected() {
        let table: toml::Table = "accent = \"not-a-colour\"".parse().unwrap();
        let p = Palette::from_table(&table);
        assert_eq!(p.get("accent"), "#62a0ea");
    }

    #[test]
    fn alpha_suffix() {
        assert_eq!(with_alpha("#7aa2f7", 0x66), "#7aa2f766");
    }
}
