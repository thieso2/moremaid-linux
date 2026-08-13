//! `$XDG_CONFIG_HOME/moremaid/config.toml` — one hand-editable file (§10).
//! This is a user-facing interface: key names are stable, parsing is
//! defensive (a malformed file falls back to defaults, never a crash), and
//! the commented default ships as a static file because the toml crate
//! cannot emit comments.

use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_BODY: &str = "iA Writer Quattro S";
pub const DEFAULT_MONO: &str = "monospace";
pub const DEFAULT_SIZE: u32 = 16;

#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct FontConfig {
    /// prose face; becomes `"<body>", "Noto Sans", sans-serif`
    pub body: String,
    /// bare `monospace` follows the Omarchy font via fontconfig (§6.4)
    pub mono: String,
    /// base px, before text-scaling-factor
    pub size: u32,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig {
            body: DEFAULT_BODY.into(),
            mono: DEFAULT_MONO.into(),
            size: DEFAULT_SIZE,
        }
    }
}

/// Resolved font stacks, ready for CSS interpolation.
#[derive(Clone, Debug, PartialEq)]
pub struct Fonts {
    pub body_stack: String,
    pub mono_stack: String,
    /// bare family name for Mermaid's explicit fontFamily (§6.4 font trap)
    pub body_family: String,
    pub size: u32,
}

impl Config {
    pub fn fonts(&self) -> Fonts {
        let body = self.font.body.trim();
        let body = if body.is_empty() { DEFAULT_BODY } else { body };
        let mono = self.font.mono.trim();
        let mono = if mono.is_empty() { DEFAULT_MONO } else { mono };
        let size = self.font.size.clamp(6, 64);
        Fonts {
            body_stack: format!("\"{body}\", \"Noto Sans\", sans-serif"),
            // a generic family stays bare — that is what makes it follow
            // the `omarchy font` choice (§6.4)
            mono_stack: if mono == "monospace" {
                mono.to_string()
            } else {
                format!("\"{mono}\", monospace")
            },
            body_family: body.to_string(),
            size,
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("moremaid/config.toml")
}

pub fn load() -> Config {
    load_from(&config_path())
}

fn load_from(path: &std::path::Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("moremaid: {}: {e}; using defaults", path.display());
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_absent_or_malformed() {
        let missing = load_from(std::path::Path::new("/nonexistent/config.toml"));
        assert_eq!(missing.fonts().size, 16);

        let dir = std::env::temp_dir().join(format!("moremaid-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[font\nbody = ???").unwrap();
        assert_eq!(load_from(&bad).fonts().size, 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let config: Config = toml::from_str("[font]\nsize = 18").unwrap();
        let fonts = config.fonts();
        assert_eq!(fonts.size, 18);
        assert_eq!(fonts.body_family, DEFAULT_BODY);
        assert_eq!(fonts.mono_stack, "monospace");
    }

    #[test]
    fn named_mono_gets_generic_fallback() {
        let config: Config =
            toml::from_str("[font]\nmono = \"JetBrains Mono\"\nbody = \"Inter\"").unwrap();
        let fonts = config.fonts();
        assert_eq!(fonts.mono_stack, "\"JetBrains Mono\", monospace");
        assert_eq!(fonts.body_stack, "\"Inter\", \"Noto Sans\", sans-serif");
        assert_eq!(fonts.body_family, "Inter");
    }

    #[test]
    fn size_clamped() {
        let config: Config = toml::from_str("[font]\nsize = 500").unwrap();
        assert_eq!(config.fonts().size, 64);
    }
}
