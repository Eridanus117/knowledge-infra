#!/usr/bin/env node
import fs from "node:fs";
import { createRequire } from "node:module";
import { JSDOM } from "jsdom";

const require = createRequire(import.meta.url);
const createDOMPurify = require("dompurify");

const window = new JSDOM("").window;
Object.defineProperty(globalThis, "window", { value: window, configurable: true });
Object.defineProperty(globalThis, "document", { value: window.document, configurable: true });
Object.defineProperty(globalThis, "navigator", { value: window.navigator, configurable: true });
Object.defineProperty(globalThis, "DOMPurify", { value: createDOMPurify(window), configurable: true });

const { default: mermaid } = await import("mermaid");

function stringifyError(err) {
  if (err?.str) return err.str;
  if (err?.message) return err.message;
  return String(err);
}

let blocks;
try {
  blocks = JSON.parse(fs.readFileSync(0, "utf8"));
} catch (err) {
  console.log(JSON.stringify({
    tool_error: `invalid validator input: ${stringifyError(err)}`,
  }));
  process.exit(2);
}

mermaid.initialize({ startOnLoad: false, securityLevel: "loose" });

const findings = [];
for (const block of blocks) {
  try {
    await mermaid.parse(block.code, { suppressErrors: false });
  } catch (err) {
    findings.push({
      line: block.line,
      message: stringifyError(err),
    });
  }
}

console.log(JSON.stringify({ findings }));
process.exit(findings.length ? 1 : 0);
