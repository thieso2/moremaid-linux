/* Moremaid page scripts.
 * Ported from MoremaidApp Sources/Rendering/PageScripts.swift @ a3ab7fd with the
 * HANDOFF edits: the 10-theme picker machinery is deleted (palette custom
 * properties are interpolated by the app, §6.3), Mermaid is initialized with
 * theme 'base' + derived variables and an explicit fontFamily (§6.4), and
 * diagram rendering goes through a source-hash cache so a prose-only edit
 * re-renders zero diagrams (§9.2).
 *
 * Expects these globals, defined before this file runs:
 *   rawMarkdown    — the markdown source (string), markdown pages only
 *   documentTitle  — the document title (string)
 *   __MOREMAID__   — { mermaidVars: {...}, plain: bool }
 */

/* ---------------------------------------------------------------- bridge */

/* Messages cross the bridge as JSON strings — the host parses cheaply and
 * never touches JSC object APIs. */
function postToHost(msg) {
    try {
        window.webkit.messageHandlers.moremaid.postMessage(JSON.stringify(msg));
    } catch (e) {
        console.error('moremaid bridge unavailable: ' + e);
    }
}

/* ---------------------------------------------------------------- lazy libraries
 * Cold start is the whole reason this app is native (§9.1). The document
 * paints with only markdown-it parsed; Prism and Mermaid (3.3 MB) load
 * afterwards — and Mermaid not at all when the document has no diagrams. */

function loadScript(src) {
    return new Promise(function(resolve, reject) {
        var s = document.createElement('script');
        s.src = src;
        s.onload = resolve;
        s.onerror = function() { reject(new Error('failed to load ' + src)); };
        document.head.appendChild(s);
    });
}

var __prismPromise = null;
function ensurePrism() {
    if (!__prismPromise) {
        __prismPromise = loadScript('moremaid://assets/vendor/prism/prism.min.js')
            .then(function() { return loadScript('moremaid://assets/vendor/prism/prism-autoloader.min.js'); })
            .then(function() {
                Prism.plugins.autoloader.languages_path = 'moremaid://assets/vendor/prism/components/';
            });
    }
    return __prismPromise;
}

var __mermaidPromise = null;
function ensureMermaid() {
    if (!__mermaidPromise) {
        __mermaidPromise = loadScript('moremaid://assets/vendor/mermaid.min.js')
            .then(function() { initializeMermaid(__MOREMAID__.mermaidVars); });
    }
    return __mermaidPromise;
}

/* ---------------------------------------------------------------- Mermaid init */

function initializeMermaid(vars) {
    mermaid.initialize({ startOnLoad: false, theme: 'base', themeVariables: vars });
}

/* Palette hot-swap (§6.3): update :root custom properties in place, re-init
 * Mermaid and recolour the diagrams where they stand. Never reloads, never
 * rebuilds the document — a theme switch must not move the scroll position. */
async function applyPalette(cssVars, mermaidVars, mode) {
    var root = document.documentElement;
    for (var k in cssVars) root.style.setProperty(k, cssVars[k]);
    if (mode) root.setAttribute('data-theme', mode);
    __MOREMAID__.mermaidVars = mermaidVars;
    if (typeof mermaid !== 'undefined') {
        initializeMermaid(mermaidVars);
        __diagramCache.clear();
        var contentDiv = document.getElementById('content');
        if (contentDiv) {
            await renderDiagrams(contentDiv);
        }
    }
}

/* ---------------------------------------------------------------- live banner (§8) */

function moremaidShowBanner(text) {
    moremaidClearBanner();
    var banner = document.createElement('div');
    banner.className = 'moremaid-banner';
    banner.id = 'moremaid-live-banner';
    var label = document.createElement('span');
    label.textContent = text;
    banner.appendChild(label);
    var dismiss = document.createElement('button');
    dismiss.className = 'moremaid-banner-dismiss';
    dismiss.textContent = '✕';
    dismiss.onclick = function() { moremaidClearBanner(); };
    banner.appendChild(dismiss);
    document.body.insertBefore(banner, document.body.firstChild);
}

function moremaidClearBanner() {
    var banner = document.getElementById('moremaid-live-banner');
    if (banner) banner.remove();
}

/* ---------------------------------------------------------------- Mermaid fullscreen */

function openMermaidInNewWindow(graphDefinition) {
    try {
        // raw definition string — the host builds the diagram page from it
        window.webkit.messageHandlers.openDiagram.postMessage(graphDefinition);
    } catch (e) {
        console.error('openDiagram bridge unavailable: ' + e);
    }
}

/* ---------------------------------------------------------------- copy buttons */

function addCopyButtons(container) {
    container = container || document;
    var codeBlocks = container.querySelectorAll('pre');
    codeBlocks.forEach(function(pre) {
        if (pre.querySelector('.copy-btn')) return;
        if (pre.closest('.mermaid') || pre.closest('.mermaid-error')) return;
        var wrapper = document.createElement('div');
        wrapper.className = 'code-block-wrapper';
        pre.parentNode.insertBefore(wrapper, pre);
        wrapper.appendChild(pre);
        var button = document.createElement('button');
        button.className = 'copy-btn';
        button.textContent = 'Copy';
        button.onclick = function() {
            var code = pre.querySelector('code') ? pre.querySelector('code').textContent : pre.textContent;
            navigator.clipboard.writeText(code).then(function() {
                button.textContent = 'Copied!';
                setTimeout(function() { button.textContent = 'Copy'; }, 2000);
            }).catch(function(err) {
                console.error('Failed to copy:', err);
                button.textContent = 'Failed';
                setTimeout(function() { button.textContent = 'Copy'; }, 2000);
            });
        };
        wrapper.appendChild(button);
    });
}

/* ---------------------------------------------------------------- markdown-it */

// Absent on code pages, which don't load markdown-it.
var md = (typeof markdownit !== 'undefined')
    ? markdownit({ html: true, breaks: true, linkify: true, langPrefix: 'language-' })
    : null;
if (md && typeof markdownitTaskLists !== 'undefined') {
    md.use(markdownitTaskLists);
}

// Heading ID generation (slugify + deduplicate).
// KEEP BYTE-IDENTICAL to the Rust HeadingParser — shared fixture in
// tests/fixtures/slugs.json runs against both sides (HANDOFF §9.3).
var _headingIds = {};
function slugify(s) {
    return s.toLowerCase().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');
}
if (md) {
md.renderer.rules.heading_open = function(tokens, idx) {
    var token = tokens[idx];
    var level = token.tag;
    var content = '';
    for (var i = idx + 1; i < tokens.length && tokens[i].type !== 'heading_close'; i++) {
        if (tokens[i].children) {
            tokens[i].children.forEach(function(c) { content += c.content; });
        }
    }
    var base = slugify(content);
    var id = base;
    if (_headingIds[id]) { id = base + '-' + _headingIds[base]++; } else { _headingIds[base] = 1; }
    return '<' + level + ' id="' + id + '">';
};

var langAliases = {
    js: 'javascript', ts: 'typescript', py: 'python', rb: 'ruby',
    yml: 'yaml', sh: 'bash', shell: 'bash', zsh: 'bash',
    cs: 'csharp', dockerfile: 'docker', objc: 'objectivec',
    'objective-c': 'objectivec', tex: 'latex', ps1: 'powershell',
    bat: 'batch', cmd: 'batch', proto: 'protobuf',
    tf: 'hcl', terraform: 'hcl', gql: 'graphql',
    patch: 'diff', 'f#': 'fsharp'
};

// Override fence renderer to apply language aliases
var defaultFence = md.renderer.rules.fence || function(tokens, idx, options, env, self) {
    return self.renderToken(tokens, idx, options);
};
md.renderer.rules.fence = function(tokens, idx, options, env, self) {
    var token = tokens[idx];
    var info = token.info ? token.info.trim() : '';
    if (info) {
        var lang = info.split(/\s+/)[0];
        token.info = langAliases[lang] || lang;
    }
    return defaultFence(tokens, idx, options, env, self);
};
}

function renderMarkdown(src) {
    _headingIds = {};
    return md.render(src);
}

/* ---------------------------------------------------------------- diagram cache (§9.2) */

// definition source → rendered SVG string. Cleared on palette change.
// A prose-only edit must call mermaid.render() zero times — that invariant
// is the cache's test (HANDOFF §9.2).
var __diagramCache = new Map();
var __diagramSeq = 0;

function mermaidErrorBlock(message, definition) {
    var block = document.createElement('div');
    block.className = 'mermaid-error';
    var title = document.createElement('div');
    title.className = 'mermaid-error-title';
    title.textContent = 'Mermaid: ' + message;
    block.appendChild(title);
    var src = document.createElement('pre');
    src.textContent = definition;
    block.appendChild(src);
    return block;
}

async function renderDiagrams(container) {
    var diagrams = container.querySelectorAll('.mermaid');
    for (var i = 0; i < diagrams.length; i++) {
        var diagram = diagrams[i];
        // freshly-rendered markdown carries the source as textContent; an
        // already-rendered diagram (palette hot-swap) keeps it in data-src
        var graphDefinition = diagram.dataset.src || diagram.textContent;
        diagram.dataset.src = graphDefinition;
        var svg = __diagramCache.get(graphDefinition);
        if (svg === undefined) {
            try {
                var result = await mermaid.render('mermaid-' + (++__diagramSeq), graphDefinition);
                svg = result.svg;
                __diagramCache.set(graphDefinition, svg);
                // the M4 invariant is checked against this line: a
                // prose-only edit must log zero of these (§9.2)
                console.log('[moremaid] diagram rendered (cache miss)');
            } catch (error) {
                console.error('Error rendering mermaid diagram:', error);
                diagram.innerHTML = '';
                diagram.appendChild(mermaidErrorBlock(error.message, graphDefinition));
                continue;
            }
        }
        var containerDiv = document.createElement('div');
        containerDiv.className = 'mermaid-container';
        var svgContainer = document.createElement('div');
        svgContainer.innerHTML = svg;
        containerDiv.appendChild(svgContainer);
        var fullscreenBtn = document.createElement('button');
        fullscreenBtn.className = 'mermaid-fullscreen-btn';
        fullscreenBtn.innerHTML = '⛶';
        fullscreenBtn.title = 'Open in new window';
        (function(def) {
            fullscreenBtn.onclick = function(e) { e.stopPropagation(); openMermaidInNewWindow(def); };
        })(graphDefinition);
        containerDiv.appendChild(fullscreenBtn);
        diagram.innerHTML = '';
        diagram.appendChild(containerDiv);
    }
}

/* ---------------------------------------------------------------- render pipeline */

function markdownToDom(src) {
    var htmlContent = renderMarkdown(src);
    htmlContent = htmlContent.replace(/<pre><code class="language-mermaid">([\s\S]*?)<\/code><\/pre>/g,
        function(match, code) {
            return '<div class="mermaid">' + code.replace(/&lt;/g,'<').replace(/&gt;/g,'>').replace(/&amp;/g,'&').replace(/&quot;/g,'"').replace(/&#39;/g,"'") + '</div>';
        });
    return htmlContent;
}

async function renderDocument(src) {
    var contentDiv = document.getElementById('content');
    if (!contentDiv) return;

    contentDiv.innerHTML = markdownToDom(src);
    postToHost({ type: 'firstRender' });
    postToHost({ type: 'headings', headings: JSON.parse(getHeadingList()) });

    if (__MOREMAID__.plain) {
        addCopyButtons();
        return;
    }

    ensurePrism().then(function() {
        try { Prism.highlightAll(); addCopyButtons(); } catch(e) { console.error('Prism error:', e); }
        if (typeof setupAutoIndexSort === 'function') setupAutoIndexSort();
    }).catch(function(e) { console.error(e); });

    if (contentDiv.querySelector('.mermaid')) {
        try {
            await ensureMermaid();
            await renderDiagrams(contentDiv);
        } catch (e) {
            console.error(e);
        }
    }
}

document.addEventListener('DOMContentLoaded', async function() {
    if (typeof rawMarkdown !== 'undefined' && document.getElementById('content')) {
        await renderDocument(rawMarkdown);
    } else {
        // Code and auto-index pages: content is server-rendered; paint
        // stands, highlighting arrives when Prism does.
        postToHost({ type: 'firstRender' });
        if (typeof setupAutoIndexSort === 'function') setupAutoIndexSort();
        if (document.querySelector('pre code')) {
            ensurePrism().then(function() {
                try { Prism.highlightAll(); addCopyButtons(); } catch(e) { console.error('Prism error:', e); }
            }).catch(function(e) { console.error(e); });
        }
    }
    postToHost({ type: 'loadComplete' });
});

/* ---------------------------------------------------------------- live re-render */

async function reRenderMarkdown(newMarkdown) {
    rawMarkdown = newMarkdown;
    await renderDocument(newMarkdown);
}

function reRenderCode(newContent, language) {
    var pre = document.querySelector('pre');
    if (!pre) return;
    var code = pre.querySelector('code');
    if (!code) return;
    code.textContent = newContent;
    code.className = 'language-' + language;
    try { Prism.highlightAll(); } catch(e) {}
}

/* ---------------------------------------------------------------- auto-index sort */

(function() {
    var aiSortColumn = 'modified';
    var aiSortAscending = false;

    function setupAutoIndexSort() {
        var table = document.querySelector('table.auto-index');
        if (!table) return;

        // Re-apply current sort after content re-render
        sortAutoIndexTable(table);

        var ths = table.querySelectorAll('th.ai-sortable');
        ths.forEach(function(th) {
            var col = th.getAttribute('data-sort');
            var indicator = th.querySelector('.sort-indicator');
            if (indicator) indicator.remove();

            if (col === aiSortColumn) {
                var span = document.createElement('span');
                span.className = 'sort-indicator';
                span.textContent = aiSortAscending ? ' ▲' : ' ▼';
                th.appendChild(span);
            }

            th.onclick = function() {
                if (aiSortColumn === col) {
                    aiSortAscending = !aiSortAscending;
                } else {
                    aiSortColumn = col;
                    aiSortAscending = col === 'name';
                }
                sortAutoIndexTable(table);
                setupAutoIndexSort();
            };
        });
    }

    function sortAutoIndexTable(table) {
        var tbody = table.querySelector('tbody');
        if (!tbody) return;
        var rows = Array.from(tbody.querySelectorAll('tr'));

        rows.sort(function(a, b) {
            var aVal, bVal;
            switch (aiSortColumn) {
                case 'name':
                    aVal = (a.getAttribute('data-name') || '').toLowerCase();
                    bVal = (b.getAttribute('data-name') || '').toLowerCase();
                    return aiSortAscending
                        ? aVal.localeCompare(bVal)
                        : bVal.localeCompare(aVal);
                case 'size':
                    aVal = parseInt(a.getAttribute('data-size') || '0', 10);
                    bVal = parseInt(b.getAttribute('data-size') || '0', 10);
                    return aiSortAscending ? aVal - bVal : bVal - aVal;
                case 'modified':
                    aVal = parseInt(a.getAttribute('data-date') || '0', 10);
                    bVal = parseInt(b.getAttribute('data-date') || '0', 10);
                    return aiSortAscending ? aVal - bVal : bVal - aVal;
                default:
                    return 0;
            }
        });

        rows.forEach(function(row) { tbody.appendChild(row); });
    }

    window.setupAutoIndexSort = setupAutoIndexSort;
})();

/* ---------------------------------------------------------------- heading list */

function getHeadingList() {
    var headings = document.querySelectorAll('h1, h2, h3, h4, h5, h6');
    var result = [];
    headings.forEach(function(h) {
        result.push({ level: parseInt(h.tagName[1]), text: h.textContent, id: h.id });
    });
    return JSON.stringify(result);
}

function getCurrentHeadingId() {
    var headings = document.querySelectorAll('h1, h2, h3, h4, h5, h6');
    if (headings.length === 0) return '';
    var scrollY = window.scrollY;
    // If we're at (or within a couple px of) the bottom of the page, the
    // last heading is "current" even though it can't physically scroll to
    // the top — otherwise tail headings near the document end never light
    // up in the sidebar.
    var atBottom = (window.innerHeight + scrollY) >= (document.body.scrollHeight - 4);
    if (atBottom) return headings[headings.length - 1].id;
    var current = '';
    for (var i = 0; i < headings.length; i++) {
        if (headings[i].getBoundingClientRect().top + scrollY <= scrollY + 60) {
            current = headings[i].id;
        }
    }
    return current;
}

/* ---------------------------------------------------------------- find in page */

(function() {
    var findMatches = [];
    var findCurrentIndex = -1;

    function findClearHighlights() {
        document.querySelectorAll('mark.find-highlight').forEach(function(mark) {
            var parent = mark.parentNode;
            parent.replaceChild(document.createTextNode(mark.textContent), mark);
            parent.normalize();
        });
        findMatches = [];
        findCurrentIndex = -1;
    }

    function findInPage(query) {
        findClearHighlights();
        if (!query || query.length === 0) return JSON.stringify({ total: 0, current: 0 });

        var container = document.getElementById('content') || document.body;
        var walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
            acceptNode: function(node) {
                var p = node.parentNode;
                if (p.tagName === 'SCRIPT' || p.tagName === 'STYLE') return NodeFilter.FILTER_REJECT;
                return NodeFilter.FILTER_ACCEPT;
            }
        }, false);

        var textNodes = [];
        var n;
        while (n = walker.nextNode()) textNodes.push(n);

        var lowerQuery = query.toLowerCase();
        textNodes.forEach(function(textNode) {
            var text = textNode.nodeValue;
            var lowerText = text.toLowerCase();
            var idx = lowerText.indexOf(lowerQuery);
            if (idx === -1) return;

            var matches = [];
            while (idx !== -1) {
                matches.push({ start: idx, end: idx + lowerQuery.length });
                idx = lowerText.indexOf(lowerQuery, idx + 1);
            }

            var fragment = document.createDocumentFragment();
            var lastIndex = 0;
            matches.forEach(function(m) {
                if (m.start > lastIndex) fragment.appendChild(document.createTextNode(text.substring(lastIndex, m.start)));
                var mark = document.createElement('mark');
                mark.className = 'find-highlight';
                mark.style.cssText = 'background:#ffeb3b;color:#333;padding:0 2px;border-radius:2px;';
                mark.textContent = text.substring(m.start, m.end);
                fragment.appendChild(mark);
                findMatches.push(mark);
                lastIndex = m.end;
            });
            if (lastIndex < text.length) fragment.appendChild(document.createTextNode(text.substring(lastIndex)));
            textNode.parentNode.replaceChild(fragment, textNode);
        });

        if (findMatches.length > 0) {
            findCurrentIndex = 0;
            findMatches[0].style.backgroundColor = '#ff9800';
            findMatches[0].scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
        return JSON.stringify({ total: findMatches.length, current: findCurrentIndex + 1 });
    }

    function findNext() {
        if (findMatches.length === 0) return JSON.stringify({ total: 0, current: 0 });
        findMatches[findCurrentIndex].style.backgroundColor = '#ffeb3b';
        findCurrentIndex = (findCurrentIndex + 1) % findMatches.length;
        findMatches[findCurrentIndex].style.backgroundColor = '#ff9800';
        findMatches[findCurrentIndex].scrollIntoView({ behavior: 'smooth', block: 'center' });
        return JSON.stringify({ total: findMatches.length, current: findCurrentIndex + 1 });
    }

    function findPrevious() {
        if (findMatches.length === 0) return JSON.stringify({ total: 0, current: 0 });
        findMatches[findCurrentIndex].style.backgroundColor = '#ffeb3b';
        findCurrentIndex = (findCurrentIndex - 1 + findMatches.length) % findMatches.length;
        findMatches[findCurrentIndex].style.backgroundColor = '#ff9800';
        findMatches[findCurrentIndex].scrollIntoView({ behavior: 'smooth', block: 'center' });
        return JSON.stringify({ total: findMatches.length, current: findCurrentIndex + 1 });
    }

    function findClear() {
        findClearHighlights();
        return JSON.stringify({ total: 0, current: 0 });
    }

    function findJumpToIndex(idx) {
        if (findMatches.length === 0 || idx < 0 || idx >= findMatches.length) return JSON.stringify({ total: findMatches.length, current: 0 });
        if (findCurrentIndex >= 0 && findCurrentIndex < findMatches.length) {
            findMatches[findCurrentIndex].style.backgroundColor = '#ffeb3b';
        }
        findCurrentIndex = idx;
        findMatches[findCurrentIndex].style.backgroundColor = '#ff9800';
        findMatches[findCurrentIndex].scrollIntoView({ behavior: 'smooth', block: 'center' });
        return JSON.stringify({ total: findMatches.length, current: findCurrentIndex + 1 });
    }

    window.findInPage = findInPage;
    window.findNext = findNext;
    window.findPrevious = findPrevious;
    window.findClear = findClear;
    window.findJumpToIndex = findJumpToIndex;
    window.getSelection2 = function() { return window.getSelection().toString(); };
})();

/* ---------------------------------------------------------------- search highlight */

function highlightSearchQuery(searchQuery) {
    if (!searchQuery) return;
    var searchTerms = searchQuery.toLowerCase().split(/\s+/).filter(function(t) { return t.length >= 2; });
    if (searchTerms.length === 0) return;

    var container = document.getElementById('content');
    if (!container) return;

    var walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
        acceptNode: function(node) {
            var parent = node.parentNode;
            if (parent.tagName === 'SCRIPT' || parent.tagName === 'STYLE' || parent.tagName === 'MARK' || parent.closest('mark')) return NodeFilter.FILTER_REJECT;
            return NodeFilter.FILTER_ACCEPT;
        }
    }, false);

    var textNodes = [];
    var node;
    while (node = walker.nextNode()) textNodes.push(node);

    var allMarks = [];
    textNodes.forEach(function(textNode) {
        var text = textNode.nodeValue;
        var lowerText = text.toLowerCase();
        var hasMatch = searchTerms.some(function(t) { return lowerText.includes(t); });
        if (!hasMatch) return;

        var matches = [];
        searchTerms.forEach(function(term) {
            var idx = lowerText.indexOf(term, 0);
            while (idx !== -1) {
                matches.push({ start: idx, end: idx + term.length });
                idx = lowerText.indexOf(term, idx + 1);
            }
        });
        matches.sort(function(a, b) { return a.start - b.start; });

        var merged = [];
        matches.forEach(function(m) {
            if (merged.length === 0 || m.start > merged[merged.length - 1].end) merged.push(m);
            else merged[merged.length - 1].end = Math.max(merged[merged.length - 1].end, m.end);
        });

        var fragment = document.createDocumentFragment();
        var lastIndex = 0;
        merged.forEach(function(m) {
            if (m.start > lastIndex) fragment.appendChild(document.createTextNode(text.substring(lastIndex, m.start)));
            var mark = document.createElement('mark');
            mark.style.cssText = 'background:#ffeb3b;color:#333;padding:0 2px;border-radius:2px;';
            mark.textContent = text.substring(m.start, m.end);
            fragment.appendChild(mark);
            allMarks.push(mark);
            lastIndex = m.end;
        });
        if (lastIndex < text.length) fragment.appendChild(document.createTextNode(text.substring(lastIndex)));
        textNode.parentNode.replaceChild(fragment, textNode);
    });

    if (allMarks.length > 0) {
        setTimeout(function() {
            allMarks[0].scrollIntoView({ behavior: 'smooth', block: 'center' });
            allMarks[0].style.transition = 'background-color 0.5s ease';
            allMarks[0].style.backgroundColor = '#ffd54f';
            setTimeout(function() { allMarks[0].style.backgroundColor = '#ffeb3b'; }, 500);
        }, 100);
    }
}
