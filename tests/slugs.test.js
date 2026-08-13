#!/usr/bin/env node
// Slug-coupling test, JS half (HANDOFF §9.3).
//
// Runs the SHIPPED web/js/page.js — not a copy of its slugify — inside a vm
// with the vendored markdown-it, renders each fixture document through the
// real pipeline, and compares the heading ids (and texts) the DOM would get
// against tests/fixtures/slugs.json. The Rust test in src/headings.rs runs
// the same fixture; if both pass, sidebar clicks land on anchors that exist.
//
// node is a TEST-ONLY dependency: nothing is compiled, bundled or installed.

'use strict';
const fs = require('fs');
const path = require('path');
const vm = require('vm');
const assert = require('assert');

const root = path.join(__dirname, '..');
const markdownit = require(path.join(root, 'web/vendor/markdown-it.min.js'));
const markdownitTaskLists = require(path.join(root, 'web/vendor/markdown-it-task-lists.min.js'));
const pageSrc = fs.readFileSync(path.join(root, 'web/js/page.js'), 'utf8');
const fixture = JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures/slugs.json'), 'utf8'));

// Minimal DOM stubs — just enough for page.js's top level to evaluate.
const sandbox = {
    markdownit,
    markdownitTaskLists,
    console,
    __MOREMAID__: { mermaidVars: {}, plain: true },
    document: {
        addEventListener() {},
        getElementById() { return null; },
        querySelectorAll() { return []; },
        createElement() { return { style: {} }; },
    },
    Map,
    JSON,
    Promise,
    setTimeout,
};
sandbox.window = sandbox;
vm.createContext(sandbox);
vm.runInContext(pageSrc, sandbox, { filename: 'web/js/page.js' });
assert.strictEqual(typeof sandbox.renderMarkdown, 'function', 'page.js did not define renderMarkdown');

function decodeEntities(s) {
    return s
        .replace(/&lt;/g, '<').replace(/&gt;/g, '>')
        .replace(/&quot;/g, '"').replace(/&#39;/g, "'")
        .replace(/&amp;/g, '&');
}

function extractHeadings(html) {
    const headings = [];
    const re = /<h([1-6]) id="([^"]*)">([\s\S]*?)<\/h\1>/g;
    let m;
    while ((m = re.exec(html)) !== null) {
        const text = decodeEntities(m[3].replace(/<[^>]+>/g, '')).trim();
        headings.push({ level: Number(m[1]), text, id: m[2] });
    }
    return headings;
}

let failures = 0;
for (const doc of fixture.documents) {
    const html = sandbox.renderMarkdown(doc.markdown);
    const got = extractHeadings(html);
    const expected = doc.headings;
    try {
        assert.deepStrictEqual(
            got.map(h => ({ level: h.level, id: h.id })),
            expected.map(h => ({ level: h.level, id: h.id })),
            `ids mismatch in "${doc.name}"`
        );
        if (!doc.skipTexts) {
            assert.deepStrictEqual(
                got.map(h => h.text),
                expected.map(h => h.text),
                `texts mismatch in "${doc.name}"`
            );
        }
        console.log(`ok   ${doc.name}`);
    } catch (e) {
        failures++;
        console.error(`FAIL ${doc.name}`);
        console.error(e.message);
    }
}

if (failures > 0) {
    console.error(`\n${failures} fixture document(s) failed`);
    process.exit(1);
}
console.log(`\nall ${fixture.documents.length} fixture documents pass against the shipped pipeline`);
