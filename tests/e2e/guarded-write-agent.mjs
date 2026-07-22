import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureSource = resolve(repositoryRoot, "tests/e2e/fixtures/read-only-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-write-fixture-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-write-db-"));
  await cp(fixtureSource, workspace, { recursive: true });
  const mock = await startMockProvider();
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: { ...process.env, OPENAI_API_KEY: "e2e-test-key", XCODING_OPENAI_BASE_URL: mock.baseUrl },
  });

  try {
    const rejected = await startPatchSession(rpc, workspace, "Prepare a rejected marker.", "ask");
    const rejectedApproval = approvalFor(rpc, rejected.session.id);
    const { detail: rejectedDetail } = await rpc.request("session.detail", { session_id: rejected.session.id });
    assert.equal(rejectedDetail.messages.length, 1);
    assert.equal(rejectedDetail.pending_actions.length, 1);
    assert.ok(rejectedDetail.events.some((item) => item.event.type === "approval_requested"));
    await rpc.request("session.resolve", { session_id: rejected.session.id, action_id: rejectedApproval.action.id, approved: false });
    await assert.rejects(readFile(resolve(workspace, "rejected.txt"), "utf8"));

    const approved = await startPatchSession(rpc, workspace, "Add a health marker file.", "ask");
    const approvedAction = approvalFor(rpc, approved.session.id);
    const completed = await rpc.request("session.resolve", {
      session_id: approved.session.id,
      action_id: approvedAction.action.id,
      approved: true,
    });
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /ready/i);
    assert.equal(await readFile(resolve(workspace, "health.txt"), "utf8"), "healthy\n");
    const { detail: completedDetail } = await rpc.request("session.detail", { session_id: approved.session.id });
    assert.equal(completedDetail.restore_points.length, 1);
    assert.equal(completedDetail.restore_points[0].applied_text, "healthy\n");
    assert.ok(completedDetail.events.some((item) => item.event.type === "tool_end"));

    const rollback = await rpc.request("session.rollback", {
      session_id: approved.session.id,
      restore_point_id: completedDetail.restore_points[0].id,
    });
    assert.equal(rollback.restore_point.path, "health.txt");
    await assert.rejects(readFile(resolve(workspace, "health.txt"), "utf8"));
    const { detail: rolledBackDetail } = await rpc.request("session.detail", { session_id: approved.session.id });
    assert.ok(rolledBackDetail.events.some((item) => item.event.type === "restore_point_rolled_back"));

    const autoEdit = await startPatchSession(rpc, workspace, "Add an auto marker file.", "auto-edit");
    assert.equal(autoEdit.session.status, "done");
    assert.equal(await readFile(resolve(workspace, "auto.txt"), "utf8"), "automatic\n");
    assert.ok(!eventsFor(rpc, autoEdit.session.id).some((event) => event.type === "approval_requested"));

    const pendingCancel = await startPatchSession(rpc, workspace, "Prepare a cancellation marker.", "ask");
    const cancellationApproval = approvalFor(rpc, pendingCancel.session.id);
    const cancelled = await rpc.request("session.cancel", { session_id: pendingCancel.session.id });
    assert.equal(cancelled.session.status, "cancelled");
    await assert.rejects(readFile(resolve(workspace, "cancel.txt"), "utf8"));
    await assert.rejects(rpc.request("session.resolve", {
      session_id: pendingCancel.session.id,
      action_id: cancellationApproval.action.id,
      approved: true,
    }));
    const { detail: cancelledDetail } = await rpc.request("session.detail", { session_id: pendingCancel.session.id });
    assert.equal(cancelledDetail.pending_actions[0].status, "rejected");
    assert.ok(cancelledDetail.events.some((item) => item.event.type === "session_cancelled"));

    console.log("Guarded write agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(workspace, { recursive: true, force: true });
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

function startPatchSession(rpc, workspace, message, mode) {
  return rpc.request("session.chat", { workspace_root: workspace, message, model: "fixture-model", mode });
}

function eventsFor(rpc, sessionId) {
  return rpc.events.filter((event) => event.session_id === sessionId);
}

function approvalFor(rpc, sessionId) {
  const approval = eventsFor(rpc, sessionId).find((event) => event.type === "approval_requested");
  assert.ok(approval, `session ${sessionId} should request approval`);
  return approval;
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], { cwd: repositoryRoot, env: environment, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  const events = [];
  let outputBuffer = "";
  let diagnostics = "";
  let requestId = 0;
  const pending = new Map();

  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    outputBuffer += chunk;
    let newlineIndex = outputBuffer.indexOf("\n");
    while (newlineIndex >= 0) {
      const line = outputBuffer.slice(0, newlineIndex).trim();
      outputBuffer = outputBuffer.slice(newlineIndex + 1);
      newlineIndex = outputBuffer.indexOf("\n");
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.method === "session.event") {
        events.push(message.params);
        continue;
      }
      const request = pending.get(message.id);
      if (!request) continue;
      pending.delete(message.id);
      if (message.error) request.reject(new Error(`RPC ${message.error.code}: ${message.error.message}`));
      else request.resolve(message.result);
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { diagnostics += chunk; });
  const rejectAll = (error) => {
    for (const request of pending.values()) request.reject(error);
    pending.clear();
  };
  child.once("error", rejectAll);
  child.once("exit", (code) => {
    if (pending.size) rejectAll(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
  });

  return {
    events,
    request(method, params) {
      const id = ++requestId;
      const response = new Promise((resolveRequest, reject) => pending.set(id, { resolve: resolveRequest, reject }));
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      return response;
    },
    async close() {
      if (child.exitCode !== null) return;
      child.stdin.end();
      await new Promise((resolveExit) => child.once("exit", resolveExit));
    },
  };
}

async function startMockProvider() {
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const hasToolResult = messages.some((message) => message.role === "tool");
    const userMessage = [...messages].reverse().find((message) => message.role === "user")?.content ?? "";

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    if (hasToolResult) {
      response.write(`data: ${JSON.stringify({ choices: [{ delta: { content: "The requested marker is ready." } }] })}\n\n`);
    } else {
      const patch = patchFor(userMessage);
      response.write(`data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, id: `call_${patch.path.replace(".", "_")}`, type: "function", function: { name: "apply_patch", arguments: JSON.stringify({ path: patch.path, old_text: "", new_text: patch.text }) } }] } }] })}\n\n`);
    }
    response.end("data: [DONE]\n\n");
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return {
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close: () => new Promise((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())),
  };
}

function patchFor(message) {
  if (message.includes("rejected")) return { path: "rejected.txt", text: "rejected\n" };
  if (message.includes("auto")) return { path: "auto.txt", text: "automatic\n" };
  if (message.includes("cancellation")) return { path: "cancel.txt", text: "cancelled\n" };
  return { path: "health.txt", text: "healthy\n" };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});