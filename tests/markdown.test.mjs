// Markdown in the chat's answers (src/views/markdown.ts): the block parser
// follows ChatMarkdown.swift, inline syntax is parsed to a tree, and rendering
// builds DOM nodes from text only — never HTML — with http(s)-only links.

import { beforeEach, test } from "node:test";
import assert from "node:assert/strict";
import { installFakeDom } from "./fakedom.mjs";
import { calls, sent } from "./tauri.mjs";

const dom = installFakeDom();
const { parseMarkdown, parseInline, renderMarkdown, safeWebUrl } = await import("../src/views/markdown.ts");

beforeEach(() => {
  calls.length = 0;
});

// ── Blocks ────────────────────────────────────────────────────────────────────

test("headings need a space after the hashes", () => {
  assert.deepEqual(parseMarkdown("# Title\n## Sub\n####### Deep"), [
    { kind: "heading", level: 1, text: "Title" },
    { kind: "heading", level: 2, text: "Sub" },
    { kind: "heading", level: 6, text: "Deep" },
  ]);
  assert.deepEqual(parseMarkdown("#hashtag"), [{ kind: "paragraph", text: "#hashtag" }]);
});

test("a fenced code block keeps its lines as they are, language apart", () => {
  const md = "Before\n```python\nprint('hi')\n  indented\n\n# not a heading\n```\nAfter";
  assert.deepEqual(parseMarkdown(md), [
    { kind: "paragraph", text: "Before" },
    { kind: "code", lang: "python", code: "print('hi')\n  indented\n\n# not a heading" },
    { kind: "paragraph", text: "After" },
  ]);
  // An unclosed fence runs to the end, as while an answer is still streaming.
  assert.deepEqual(parseMarkdown("```\nlet x"), [{ kind: "code", lang: "", code: "let x" }]);
});

test("lists keep their markers and nesting", () => {
  assert.deepEqual(parseMarkdown("- one\n* two\n  + nested\n1. first\n12. twelfth"), [
    { kind: "item", prefix: "•", text: "one", indent: 0 },
    { kind: "item", prefix: "•", text: "two", indent: 0 },
    { kind: "item", prefix: "•", text: "nested", indent: 1 },
    { kind: "item", prefix: "1.", text: "first", indent: 0 },
    { kind: "item", prefix: "12.", text: "twelfth", indent: 0 },
  ]);
});

test("quotes, rules and paragraphs that stop at the next block", () => {
  assert.deepEqual(parseMarkdown("> quoted\n>\n---\nline one\nline two\n- item\n\npara"), [
    { kind: "quote", text: "quoted" },
    { kind: "quote", text: "" },
    { kind: "rule" },
    { kind: "paragraph", text: "line one\nline two" },
    { kind: "item", prefix: "•", text: "item", indent: 0 },
    { kind: "paragraph", text: "para" },
  ]);
  assert.deepEqual(parseMarkdown("a\r\nb"), [{ kind: "paragraph", text: "a\nb" }]);
  assert.deepEqual(parseMarkdown(""), []);
});

// ── Inline ────────────────────────────────────────────────────────────────────

test("inline code, bold, italic and links nest as a tree", () => {
  assert.deepEqual(parseInline("Use `npm ci` **now**, *really* or __not__ _ever_."), [
    { kind: "text", text: "Use " },
    { kind: "code", text: "npm ci" },
    { kind: "text", text: " " },
    { kind: "strong", children: [{ kind: "text", text: "now" }] },
    { kind: "text", text: ", " },
    { kind: "em", children: [{ kind: "text", text: "really" }] },
    { kind: "text", text: " or " },
    { kind: "strong", children: [{ kind: "text", text: "not" }] },
    { kind: "text", text: " " },
    { kind: "em", children: [{ kind: "text", text: "ever" }] },
    { kind: "text", text: "." },
  ]);
  assert.deepEqual(parseInline("[**Docs**](https://example.com/a?b=1)"), [
    { kind: "link", url: "https://example.com/a?b=1", children: [{ kind: "strong", children: [{ kind: "text", text: "Docs" }] }] },
  ]);
});

test("snake_case words and code spans are not italics", () => {
  assert.deepEqual(parseInline("call my_long_name now"), [{ kind: "text", text: "call my_long_name now" }]);
  assert.deepEqual(parseInline("`a *b* c`"), [{ kind: "code", text: "a *b* c" }]);
  assert.deepEqual(parseInline("2 * 3 * 4"), [{ kind: "text", text: "2 * 3 * 4" }]);
});

test("only http and https addresses become links", () => {
  assert.equal(safeWebUrl("https://example.com"), "https://example.com/");
  assert.equal(safeWebUrl("http://localhost:3000/x"), "http://localhost:3000/x");
  for (const bad of ["javascript:alert(1)", "JaVaScRiPt:alert(1)", "file:///etc/passwd", "data:text/html,<b>x</b>", "vbscript:x", "/relative", "mailto:a@b.c", "not a url"]) {
    assert.equal(safeWebUrl(bad), null, bad);
  }
  assert.deepEqual(parseInline("[click](javascript:alert(1))"), [
    { kind: "link", url: null, children: [{ kind: "text", text: "click" }] },
    { kind: "text", text: ")" },
  ]);
});

// ── Rendering ─────────────────────────────────────────────────────────────────

test("markup in an answer is shown as text, never parsed", () => {
  const root = dom.root();
  const evil = '<img src=x onerror="alert(1)"> **<script>alert(2)</script>**\n```\n<b>code</b>\n```';
  renderMarkdown(root, evil); // fakedom throws if anything touches innerHTML
  assert.equal(root.find("IMG").length, 0);
  assert.equal(root.find("SCRIPT").length, 0);
  assert.equal(root.find("B").length, 0);
  assert.match(root.textContent, /<img src=x onerror="alert\(1\)">/);
  assert.equal(root.find("STRONG")[0].textContent, "<script>alert(2)</script>");
  assert.equal(root.find("PRE")[0].textContent, "<b>code</b>");
});

test("each block becomes its own element", () => {
  const root = dom.root();
  renderMarkdown(root, "## Answer\n\nHere:\n\n- **item 1**\n- item 2\n\n> note\n\n---\n\n```js\nx()\n```");
  const classes = root.children.map((c) => c.className);
  assert.deepEqual(classes, ["md-h md-h1", "md-p", "md-item", "md-item", "md-quote", "md-rule", "md-pre"]);
  assert.equal(root.find(".md-bullet")[0].textContent, "•");
  assert.equal(root.find(".md-copy").length, 1);
  // Rendering again replaces, it never appends.
  renderMarkdown(root, "plain");
  assert.equal(root.children.length, 1);
});

test("a web link opens through Rust; anything else is plain text", () => {
  const root = dom.root();
  renderMarkdown(root, "See [the docs](https://example.com/docs) or [this](file:///etc/passwd).");
  const links = root.find("A");
  assert.equal(links.length, 1);
  assert.equal(links[0].getAttribute("href"), "https://example.com/docs");
  const event = links[0].fire("click");
  assert.equal(event.defaultPrevented, true);
  assert.deepEqual(sent("open_url"), [{ url: "https://example.com/docs" }]);
  assert.match(root.textContent, /or this\.$/);
});

// ── The finished card ─────────────────────────────────────────────────────────
//
// An agent's last words on its card (renderInline): text, bold, `code` and
// http(s) links, and nothing else, whatever the agent wrote.

const { firstParagraph, inlineLite, isLongAnswer, renderInline } = await import("../src/views/markdown.ts");

/** Every node kind in an inline tree, links with their address. */
function kinds(nodes, out = new Set()) {
  for (const n of nodes) {
    out.add(n.kind === "link" ? `link:${new URL(n.url).protocol}` : n.kind);
    if (n.children) kinds(n.children, out);
  }
  return out;
}

const HOSTILE = [
  "A [link](javascript:alert(1)) and [another](JaVaScRiPt:alert(2)).",
  "[data](data:text/html,<script>alert(1)</script>) [file](file:///C:/Windows/win.ini) [vb](vbscript:msgbox(1))",
  '<img src=x onerror="alert(1)"> <script>alert(2)</script> <a href="javascript:alert(3)">x</a>',
  "<b>bold?</b> <iframe src=//evil></iframe> &lt;script&gt; &#x3C;svg onload=alert(1)&#x3E;",
  "An unclosed `code span and **unclosed bold and [unclosed](http://",
  "`` ` ``` ** __ * _ [] () [](<) [x]() [x](  ) ![img](https://example.com/a.png)",
  "**bold with `code` and a [web link](https://example.com/a?b=1&c=2#d)** then *italic* and _more_",
  "[spaced](https://example.com/ a) [ok](HTTP://EXAMPLE.COM) [ftp](ftp://example.com) [rel](/etc/passwd) [mail](mailto:a@b.c)",
];

test("the finished card's line is text, bold, code and web links, nothing else", () => {
  const allowed = new Set(["text", "strong", "code", "link:http:", "link:https:"]);
  for (const text of HOSTILE) {
    for (const kind of kinds(inlineLite(text))) assert.ok(allowed.has(kind), `${kind} from ${text}`);
  }
  // Italics are not part of it: what was between the marks stays, as text.
  assert.deepEqual(inlineLite("an *italic* word"), [{ kind: "text", text: "an " }, { kind: "text", text: "italic" }, { kind: "text", text: " word" }]);
  // A link that is not a web address keeps its words and loses the link.
  assert.deepEqual(inlineLite("[run](javascript:alert(1))")[0], { kind: "text", text: "run" });
});

test("what the finished card draws is those four things, built from text", () => {
  for (const text of HOSTILE) {
    const root = dom.root();
    renderInline(root, text); // fakedom throws if anything touches innerHTML
    const tags = new Set([...root.walk()].map((e) => e.tagName));
    for (const tag of tags) assert.ok(["STRONG", "CODE", "A"].includes(tag), `${tag} from ${text}`);
    for (const a of root.find("A")) {
      assert.match(a.getAttribute("href"), /^https?:\/\//);
      // Its click goes through the app's own open-URL path, which checks the scheme again.
      calls.length = 0;
      assert.equal(a.fire("click").defaultPrevented, true);
      assert.deepEqual(sent("open_url"), [{ url: a.getAttribute("href") }]);
    }
  }
  // Raw HTML comes out as the characters that were written.
  const root = dom.root();
  renderInline(root, '<img src=x onerror="alert(1)"> **<script>alert(2)</script>**');
  assert.equal(root.find("IMG").length + root.find("SCRIPT").length, 0);
  assert.equal(root.textContent, '<img src=x onerror="alert(1)"> <script>alert(2)</script>');
  // An unclosed code span or bold is just its characters.
  renderInline(root, "run `npm test and **wait");
  assert.equal(root.children.length, 0);
  assert.equal(root.textContent, "run `npm test and **wait");
  // And the good case: bold, code, a web link.
  renderInline(root, "**Done.** Ran `npm test`, see [the log](https://example.com/log).");
  assert.deepEqual(root.children.map((e) => e.tagName), ["STRONG", "CODE", "A"]);
  // Drawing again replaces, it never appends.
  renderInline(root, "plain");
  assert.equal(root.childNodes.length, 1);
});

test("one line holds a short answer; a long one gets the box that scrolls", () => {
  assert.equal(firstParagraph("## Summary\n\nAll **done**.\nTwo lines.\n\n- item"), "Summary");
  assert.equal(firstParagraph("All **done**.\n  Two lines.\n\nMore."), "All **done**. Two lines.");
  assert.equal(firstParagraph("```\ncode\n```\n\nThen words."), "Then words.");
  assert.equal(firstParagraph(""), "");
  assert.equal(isLongAnswer("All done."), false);
  assert.equal(isLongAnswer(""), false);
  assert.equal(isLongAnswer(null), false);
  assert.equal(isLongAnswer("First.\n\nSecond."), true);
  assert.equal(isLongAnswer("- one\n- two"), true);
  assert.equal(isLongAnswer("x".repeat(200)), true);
  assert.equal(isLongAnswer("```\nls\n```"), true);
});
