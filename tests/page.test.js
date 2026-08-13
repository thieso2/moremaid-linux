#!/usr/bin/env node
// Smoke tests for the shipped page.js runtime surface that the Rust side
// calls via evaluate_javascript — applyPalette (theme hot-swap, M4), the
// live banner, and the re-render entry points. Runs the real file in a vm
// with DOM stubs; a rename or syntax slip here would otherwise only
// surface as a silent console error inside WebKit.

'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert');

const root = path.join(__dirname, '..');
const markdownit = require(path.join(root, 'web/vendor/markdown-it.min.js'));
const markdownitTaskLists = require(path.join(root, 'web/vendor/markdown-it-task-lists.min.js'));
const pageSrc = fs.readFileSync(path.join(root, 'web/js/page.js'), 'utf8');

function makeElement(tag) {
    return {
        tagName: String(tag).toUpperCase(),
        children: [],
        style: {},
        dataset: {},
        className: '',
        id: '',
        textContent: '',
        innerHTML: '',
        appendChild(c) { this.children.push(c); return c; },
        insertBefore(c) { this.children.unshift(c); return c; },
        remove() { removed.push(this.id); },
        querySelector() { return null; },
        querySelectorAll() { return []; },
        setAttribute(k, v) { this[k] = v; },
        getAttribute(k) { return this[k]; },
    };
}

const removed = [];
let bannerElement = null;

const documentElement = {
    style: { set: {}, setProperty(k, v) { this.set[k] = v; } },
    attrs: { 'data-theme': 'dark' },
    setAttribute(k, v) { this.attrs[k] = v; },
    getAttribute(k) { return this.attrs[k]; },
};

const body = makeElement('body');
body.firstChild = null;

const sandbox = {
    markdownit,
    markdownitTaskLists,
    console: { log() {}, error(msg) { throw new Error('console.error: ' + msg); }, debug() {} },
    __MOREMAID__: { mermaidVars: {}, plain: true },
    document: {
        documentElement,
        body,
        addEventListener() {},
        getElementById(id) { return id === 'moremaid-live-banner' ? bannerElement : null; },
        querySelector() { return null; },
        querySelectorAll() { return []; },
        createElement(tag) { return makeElement(tag); },
    },
    Map,
    JSON,
    Promise,
    setTimeout,
};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(pageSrc, sandbox, { filename: 'web/js/page.js' });

// the full surface the Rust side invokes by name
for (const fn of [
    'applyPalette', 'reRenderMarkdown', 'reRenderCode',
    'moremaidShowBanner', 'moremaidClearBanner',
    'highlightSearchQuery', 'getHeadingList',
]) {
    assert.strictEqual(typeof sandbox[fn], 'function', `${fn} must exist — Rust calls it by name`);
}

// applyPalette: sets :root vars + data-theme, tolerates mermaid being absent
(async () => {
    await sandbox.applyPalette({ '--bg-color': '#101010', '--text-color': '#eee' }, { darkMode: false }, 'light');
    assert.strictEqual(documentElement.style.set['--bg-color'], '#101010');
    assert.strictEqual(documentElement.attrs['data-theme'], 'light');
    assert.deepStrictEqual(sandbox.__MOREMAID__.mermaidVars, { darkMode: false });

    // banner: show inserts at top, clear removes
    sandbox.moremaidShowBanner('file is gone');
    bannerElement = body.children[0];
    assert.strictEqual(bannerElement.id, 'moremaid-live-banner');
    assert.strictEqual(bannerElement.children[0].textContent, 'file is gone');
    sandbox.moremaidClearBanner();
    assert.deepStrictEqual(removed, ['moremaid-live-banner']);

    console.log('page.js runtime surface ok');
})().catch((e) => {
    console.error('FAIL:', e.message);
    process.exit(1);
});
