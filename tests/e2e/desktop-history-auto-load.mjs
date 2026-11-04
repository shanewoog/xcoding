import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");

assert.ok(
  appSource.includes("node.scrollTop <= 48") &&
    appSource.includes("void loadOlderMessages()"),
  "Earlier messages must load automatically when the conversation reaches the top",
);
assert.ok(
  !appSource.includes('className="history-load-older"'),
  "Earlier-message loading must not require a visible button",
);

console.log("Desktop history auto-load checks passed.");