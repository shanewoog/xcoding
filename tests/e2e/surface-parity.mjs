import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

const SHARED_WORKFLOWS = [
  { name: "ping", cliNeedle: 'case "ping"', desktopCommand: "ping", desktopUiNeedle: null },
  { name: "config.get", cliNeedle: '"config.get"', desktopCommand: "workspace_config", desktopUiNeedle: 'invoke<WorkspaceConfig>("workspace_config"' },
  { name: "config.set", cliNeedle: '"config.set"', desktopCommand: "set_workspace_config", desktopUiNeedle: 'invoke<WorkspaceConfig>("set_workspace_config"' },
  { name: "session.list", cliNeedle: '"session.list"', desktopCommand: "list_sessions", desktopUiNeedle: 'invoke<Session[]>("list_sessions"' },
  { name: "session.detail", cliNeedle: '"session.detail"', desktopCommand: "session_detail", desktopUiNeedle: 'invoke<SessionDetail>("session_detail"' },
  { name: "session.chat", cliNeedle: '"session.chat"', desktopCommand: "chat", desktopUiNeedle: 'invoke<ChatResult>("chat"' },
  { name: "session.resolve", cliNeedle: '"session.resolve"', desktopCommand: "resolve_action", desktopUiNeedle: 'invoke<ResolveActionResult>("resolve_action"' },
  { name: "session.rollback", cliNeedle: '"session.rollback"', desktopCommand: "rollback_restore_point", desktopUiNeedle: 'invoke<RollbackRestorePointResult>("rollback_restore_point"' },
  { name: "session.cancel", cliNeedle: '"session.cancel"', desktopCommand: "cancel_session", desktopUiNeedle: 'invoke<CancelSessionResult>("cancel_session"' },
  { name: "session.replay", cliNeedle: '"session.replay"', desktopCommand: "session_replay", desktopUiNeedle: 'invoke<ReplaySessionResult>("session_replay"' },
];

async function main() {
  const cliSource = await readFile(resolve(repositoryRoot, "apps/cli/src/index.ts"), "utf8");
  const desktopMain = await readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8");
  const desktopUi = await readFile(resolve(repositoryRoot, "apps/desktop/src/App.tsx"), "utf8");
  const serverMain = await readFile(resolve(repositoryRoot, "crates/xcoding-server/src/main.rs"), "utf8");
  const coreSource = await readFile(resolve(repositoryRoot, "crates/xcoding-core/src/lib.rs"), "utf8");
  const missing = [];

  for (const workflow of SHARED_WORKFLOWS) {
    if (!cliSource.includes(workflow.cliNeedle)) {
      missing.push("CLI missing " + workflow.name);
    }
    if (!desktopMain.includes("fn " + workflow.desktopCommand)) {
      missing.push("Desktop fn missing " + workflow.name);
    }
    if (!desktopMain.includes(workflow.desktopCommand)) {
      missing.push("Desktop handler missing " + workflow.name);
    }
    if (workflow.desktopUiNeedle && !desktopUi.includes(workflow.desktopUiNeedle)) {
      missing.push("Desktop UI missing " + workflow.name);
    }
  }

  for (const method of ["session.chat", "session.resolve", "session.rollback", "session.cancel"]) {
    assert.ok(
      serverMain.includes('"' + method + '"') || serverMain.includes('method == "' + method + '"'),
      "server should handle " + method,
    );
  }

  assert.ok(cliSource.includes("session.create"), "CLI session.create");
  assert.ok(cliSource.includes('case "chat"'), "CLI chat");
  assert.ok(cliSource.includes('case "replay"'), "CLI replay");
  assert.ok(desktopUi.toLowerCase().includes("replay"), "Desktop replay UI");
  assert.ok(coreSource.includes("fn ping") || coreSource.includes("pub fn ping"), "core ping");
  assert.equal(missing.length, 0, missing.join("; "));
  console.log("Surface parity check passed (" + SHARED_WORKFLOWS.length + " shared workflows).");
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
