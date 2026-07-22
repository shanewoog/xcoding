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
    const pending = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Add a health marker file.",
      model: "fixture-model",
      mode: "ask",
    });
    assert.equal(pending.session.status, "need_user");
    const preview = rpc.events.find((event) => event.type === "patch_preview");
    assert.deepEqual(preview?.preview, { path: "health.txt", file_existed: false, old_text: "", new_text: "healthy\n" });
    const approval = rpc.events.find((event) => event.type === "approval_requested");
    assert.ok(approval);
    await assert.rejects(readFile(resolve(workspace, "health.txt"), "utf8"));

    const completed = await rpc.request("session.resolve", {
      session_id: pending.session.id,
      action_id: approval.action.id,
      approved: true,
    });
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /health marker/i);
    assert.equal(await readFile(resolve(workspace, "health.txt"), "utf8"), "healthy\n");
    assert.ok(rpc.events.some((event) => event.type === "tool_end" && event.tool_call.name === "apply_patch" && event.success));
    console.log("Guarded write agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(workspace, { recursive: true, force: true });
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], { cwd: repositoryRoot, env: environment, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  const events = []; let outputBuffer = ""; let diagnostics = ""; let requestId = 0; const pending = new Map();
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    outputBuffer += chunk;
    let newlineIndex = outputBuffer.indexOf("\n");
    while (newlineIndex >= 0) {
      const line = outputBuffer.slice(0, newlineIndex).trim(); outputBuffer = outputBuffer.slice(newlineIndex + 1); newlineIndex = outputBuffer.indexOf("\n");
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.method === "session.event") { events.push(message.params); continue; }
      const request = pending.get(message.id); if (!request) continue;
      pending.delete(message.id); message.error ? request.reject(new Error(`RPC ${message.error.code}: ${message.error.message}`)) : request.resolve(message.result);
    }
  });
  child.stderr.setEncoding("utf8"); child.stderr.on("data", (chunk) => { diagnostics += chunk; });
  const rejectAll = (error) => { for (const request of pending.values()) request.reject(error); pending.clear(); };
  child.once("error", rejectAll); child.once("exit", (code) => { if (pending.size) rejectAll(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`)); });
  return {
    events,
    request(method, params) { const id = ++requestId; const response = new Promise((resolveRequest, reject) => pending.set(id, { resolve: resolveRequest, reject })); child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`); return response; },
    async close() { if (child.exitCode !== null) return; child.stdin.end(); await new Promise((resolveExit) => child.once("exit", resolveExit)); },
  };
}

async function startMockProvider() {
  let turn = 0;
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST"); assert.equal(request.url, "/v1/chat/completions");
    for await (const _ of request) { /* consume */ }
    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    if (turn++ === 0) {
      response.write('data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_write_health","type":"function","function":{"name":"apply_patch","arguments":"{\\"path\\":\\"health.txt\\",\\"old_text\\":\\"\\",\\"new_text\\":\\"healthy\\\\n\\"}"}}]}}]}\n\n');
    } else {
      response.write('data: {"choices":[{"delta":{"content":"Added the health marker and it is ready for verification."}}]}\n\n');
    }
    response.end("data: [DONE]\n\n");
  });
  await new Promise((resolveListen, rejectListen) => { server.once("error", rejectListen); server.listen(0, "127.0.0.1", resolveListen); });
  const address = server.address(); assert.ok(address && typeof address !== "string");
  return { baseUrl: `http://127.0.0.1:${address.port}/v1`, close: () => new Promise((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())) };
}

main().catch((error) => { console.error(error instanceof Error ? error.stack : String(error)); process.exitCode = 1; });