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
assert.ok(
  appSource.includes("node.scrollHeight > node.clientHeight + 1") &&
    appSource.includes("void loadOlderMessages()"),
  "A short collapsed history must keep loading until it fills the viewport",
);
assert.ok(
  appSource.includes("historyHydrationRequestIdRef") &&
    appSource.includes("historyHydrationRequestIdRef.current === requestId") &&
    appSource.includes("activeSessionIdRef.current === sessionId"),
  "Session switches must ignore stale history detail responses",
);
assert.ok(
  appSource.includes("olderMessagesRequestIdRef") &&
    appSource.includes("olderMessagesRequestIdRef.current === requestId"),
  "Older-page responses must be ignored after another session or page request supersedes them",
);

assert.ok(
  appSource.includes("const switchingSession = activeSessionIdRef.current !== session.id"),
  "Reselecting the active session must not clear its loaded history",
);

console.log("Desktop history auto-load checks passed.");
