# Moremaid for Linux

A markdown reader with first-class Mermaid diagram support, built for
**Omarchy/Hyprland** as a native GTK4 + WebKitGTK application.

This is a reimplementation of the macOS Moremaid, not a port of its codebase —
see `HANDOFF.md` in the macOS repository for every decision. The rendering
layer (CSS/JS in `web/`) is extracted from the macOS app at pinned commit
`a3ab7fd` and adapted; vendored web dependencies and their exact pins are
recorded in `web/vendor/VERSIONS.md`.

**Status: all five milestones of the build plan implemented.** It opens
files, browses directories, finds things, live-reloads, and packages.

## Running

```bash
moremaid README.md            # open a markdown file
mm README.md                  # same binary, shorter to type
moremaid docs/                # browse a directory (Navigator + index page)
moremaid                      # browse the current directory
moremaid src/main.rs          # any text file renders as highlighted code
cat notes.md | moremaid       # stdin; relative links resolve from the CWD
```

Files can also be dropped onto the window, or opened from a file manager
(the package associates with `text/markdown` without grabbing the default).

## Keyboard

`?` or `F1` shows the complete shortcuts overlay. The essentials:

| binding | action |
|---|---|
| `Ctrl+P` or `/` | Quick Open — fuzzy filename finder |
| `Ctrl+Shift+F` | Find in Files — full-text, streamed |
| `Tab` | switch search mode (filename ↔ content) |
| `Ctrl+B` | toggle the Navigator |
| `j` `k`, `gg` `G`, `Ctrl+D` `Ctrl+U` | scroll (vim keys are baseline) |
| `Ctrl` `+` `-` `0` | zoom (on top of the system text scale) |
| `Ctrl+N` | new window |
| `Ctrl+Shift+R` | force full render of a large document |
| `Ctrl+click` / middle-click | open link in new window |

## Live behaviour

- An open file that changes on disk re-renders in place — scroll position
  survives, and unchanged Mermaid diagrams are served from cache (a
  prose-only edit re-renders zero diagrams).
- A deleted or replaced file keeps the last good render behind a dismissible
  banner, and recovers when the path returns (branch switches).
- An Omarchy theme switch recolours prose, code highlighting, diagrams and
  the Navigator live — no reload, scroll untouched. Off Omarchy the system
  light/dark preference drives a built-in palette.

## Configuration

`~/.config/moremaid/config.toml` — optional, every key defaulted, key names
stable. A commented example ships at
`/usr/share/doc/moremaid/config.toml.example`:

```toml
[font]
body = "iA Writer Quattro S"
mono = "monospace"          # bare generic = follow the `omarchy font` choice
size = 16                   # base px, before text-scaling-factor
```

The web rendering layer is plain files on disk. To restyle without a
toolchain, copy `/usr/share/moremaid/web/` to `~/.local/share/moremaid/web/`
and edit — the user copy takes precedence. `MOREMAID_WEB_DIR` overrides both.

## Installing

```bash
# from a checkout
cd packaging && makepkg -si

# or by hand
cargo build --release
sudo install -Dm755 target/release/moremaid /usr/bin/moremaid
sudo cp -r web /usr/share/moremaid/web
```

Runtime dependencies: `gtk4 libadwaita webkitgtk-6.0 xdg-desktop-portal-gtk
ttf-ia-writer`. Every one is load-bearing — `xdg-desktop-portal-gtk` because
Hyprland's portal implements no file chooser, and `ttf-ia-writer` because
WebKitGTK's SVG renderer can silently drop every Mermaid label when the
configured font family is missing.

## Recommended Hyprland rules

Add to your Hyprland config (Moremaid does not write to your config):

```conf
# Float the Mermaid diagram viewer instead of splitting your reading window
windowrule = float, class:org.moremaid.Moremaid, title:Mermaid Diagram.*

# Opt out of Omarchy's default window translucency — text wants a solid ground
windowrule = tag +no-default-opacity, class:org.moremaid.Moremaid
windowrule = opacity 1 1, class:org.moremaid.Moremaid
```

## Tests

Two suites, and both must run — the JS half won't run under `cargo test`
and is the half that gets forgotten (§9.6):

```bash
cargo test
node tests/slugs.test.js    # slug fixture through the SHIPPED page.js pipeline
node tests/page.test.js     # the page.js surface Rust calls by name
```

The release checklist is: run all three, tag, bump `pkgver` in
`packaging/PKGBUILD`.

## Debugging

- `MOREMAID_STARTUP_TIME=1` prints cold-start timings to stderr.
- `MOREMAID_DEBUG=1` writes web-console messages to stdout; the WebKit
  inspector is always enabled.
