import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function sourceBlock(source, startNeedle, endNeedle) {
  const start = source.indexOf(startNeedle);
  const end = source.indexOf(endNeedle, start + startNeedle.length);
  assert.ok(start >= 0 && end > start, `missing source block: ${startNeedle}`);
  return source.slice(start, end);
}

async function main() {
  const appSource = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const packageSource = await readFile(resolve(repositoryRoot, "package.json"), "utf8");
  const cancelSessionSource = sourceBlock(
    appSource,
    "async function cancelSession(): Promise<void>",
    "function unhideProjectPath",
  );
  const hydrateSessionSource = sourceBlock(
    appSource,
    "const hydrateSession = useCallback(async (sessionId: string) => {",
    "const loadModelCallLogs = useCallback",
  );

  assert.ok(
    !cancelSessionSource.includes("await previousInFlight"),
    "cancel UI must not wait for the blocked chat promise before leaving running state",
  );
  assert.ok(
    cancelSessionSource.includes("const result = await invoke<CancelSessionResult>(\"cancel_session\""),
    "cancel must use the authoritative cancelled session returned by the backend",
  );
  assert.ok(
    cancelSessionSource.includes("session.id === sessionId ? result.session : session"),
    "cancel must immediately replace the local session with the cancelled backend state",
  );
  assert.ok(
    cancelSessionSource.includes("if (activeSessionIdRef.current === sessionId) setPlan([])"),
    "cancel must clear stale run steps immediately",
  );
  assert.ok(
    hydrateSessionSource.includes("detail.session.status === \"cancelled\" ? [] : latestPlan(activityEvents)"),
    "hydrating a cancelled session must not restore the previous turn's plan",
  );
  assert.ok(
    packageSource.includes("desktop-cancel.mjs"),
    "the fast verification suite must include the cancellation regression",
  );

  console.log("Desktop cancellation regression passed.");
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
