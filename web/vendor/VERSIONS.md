# Vendored web dependencies — exact pins

Per HANDOFF §4: these versions are pinned here because nothing else will remember them.

| library | version | source | files |
|---|---|---|---|
| markdown-it | **14.1.0** | `https://cdn.jsdelivr.net/npm/markdown-it@14.1.0/dist/markdown-it.min.js` | `markdown-it.min.js` |
| Mermaid | **10.9.8** | `https://cdn.jsdelivr.net/npm/mermaid@10.9.8/dist/mermaid.min.js` | `mermaid.min.js` |
| Prism.js | **1.29.0** | npm tarball `prismjs-1.29.0.tgz` + CDN minified core | `prism/prism.min.js`, `prism/prism-autoloader.min.js`, `prism/components/` (298 files, complete) |
| markdown-it-task-lists | **2.1.1** | `https://cdn.jsdelivr.net/npm/markdown-it-task-lists@2.1.1/dist/markdown-it-task-lists.min.js` | `markdown-it-task-lists.min.js` — GFM task lists are not in markdown-it core |

Notes:

- markdown-it 15.0.0 exists but **dropped the browser UMD bundle** (`dist/markdown-it.min.js`
  is a 404). 14.1.0 is the last version usable without a bundler; do not bump past it
  without introducing a build step, which HANDOFF §4 forbids.
- Mermaid 11 exists; the config schema changed. Do not silently jump majors (HANDOFF §4).
- Prism `components/` is vendored **in full** because the autoloader fetches grammars
  lazily at render time (HANDOFF §4, §12.5). `languages_path` is pointed at the custom
  URI scheme by the HTML generator.
- Downloaded 2026-08-13.
