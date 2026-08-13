# Moremaid for Linux

A markdown reader with first-class Mermaid diagram support, built for
**Omarchy/Hyprland** as a native GTK4 + WebKitGTK application.

This is a reimplementation of the macOS Moremaid, not a port of its codebase —
see `HANDOFF.md` in the macOS repository for every decision. The rendering
layer (CSS/JS in `web/`) is extracted from the macOS app at pinned commit
`a3ab7fd` and adapted; vendored web dependencies and their exact pins are
recorded in `web/vendor/VERSIONS.md`.

**Status: Milestone 2** — it opens files and browses directories. Quick Open,
Find in Files, the `?` shortcuts overlay, and live reload land in
Milestones 3–4.

## Running

```bash
moremaid README.md            # open a markdown file
moremaid docs/                # browse a directory (Navigator + index page)
moremaid                      # browse the current directory
moremaid src/main.rs          # any text file renders as highlighted code
cat notes.md | moremaid       # stdin; relative links resolve from the CWD
```

Directory mode scans recursively, respecting `.gitignore` (also outside git
repositories) and skipping `.git`/`node_modules`. The Navigator lists
folders, markdown files, and the headings inside each file — click a heading
to jump to it. Heading anchors are guaranteed to match the rendered page:
a shared fixture (`tests/fixtures/slugs.json`) pins the slug algorithm on
both the Rust and JavaScript side.

Keyboard: `Ctrl+B` toggle the Navigator, `Ctrl` `+` / `-` / `0` zoom,
`Ctrl+Shift+R` force full render of a large document. `Ctrl+click` /
middle-click / `Shift+click` on an internal link opens it in a new window.

## Theming

Moremaid follows the active Omarchy theme, live, and ships zero themes of its
own. The palette is read from
`~/.local/state/omarchy/current/theme/colors.toml`; prose, code highlighting
and Mermaid diagrams all derive from it. Off Omarchy it falls back to a
built-in palette chosen by the system light/dark preference.

The web rendering layer is plain files on disk. To restyle without a
toolchain, copy `web/` to `~/.local/share/moremaid/web/` and edit — that copy
takes precedence over the packaged one. `MOREMAID_WEB_DIR` overrides both.

## Building

```bash
# runtime deps
pacman -S gtk4 libadwaita webkitgtk-6.0 xdg-desktop-portal-gtk ttf-ia-writer
# build
pacman -S rust
cargo build --release
```

`ttf-ia-writer` is a hard dependency, not a preference — WebKitGTK's SVG
renderer can silently drop every Mermaid label when the configured font family
is missing.

## Recommended Hyprland rules

Add to `~/.config/hypr/` config (Moremaid does not write to your config):

```conf
# Float the Mermaid diagram viewer instead of splitting your reading window
windowrule = float, class:org.moremaid.Moremaid, title:Mermaid Diagram.*

# Opt out of Omarchy's default window translucency — text wants a solid ground
windowrule = tag +no-default-opacity, class:org.moremaid.Moremaid
windowrule = opacity 1 1, class:org.moremaid.Moremaid
```

## Tests

Two suites, and both must run — the JS half won't run under `cargo test`:

```bash
cargo test
node tests/slugs.test.js     # slug fixture, once it lands with the Navigator (M2)
```

## Debugging

- `MOREMAID_STARTUP_TIME=1` prints cold-start timings to stderr.
- `MOREMAID_DEBUG=1` writes web-console messages to stdout; the WebKit
  inspector is always enabled.
