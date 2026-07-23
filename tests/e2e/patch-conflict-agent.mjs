import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  assert.ok(await pathExists(serverPath), `missing server binary at ${serverPath}; run cargo build -p xcoding-server`);

  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-patch-conflict-"));
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-patch-conflict-db-"));
  const mock = await startMockProvider();
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    await runAskRaceConflict(rpc, mock, workspace);
    await runAutoEditStaleConflict(rpc, mock, workspace);
    console.log("Patch conflict agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(workspace, { recursive: true, force: true });
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

async function runAskRaceConflict(rpc, mock, workspace) {
  const notesPath = resolve(workspace, "notes.txt");
  await writeFile(notesPath, "base\n", "utf8");
  mock.scenario = "ask-race";
  rpc.events.length = 0;

  const started = await rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Update notes with next content.",
    model: "fixture-model",
    mode: "ask",
  });
  assert.equal(started.session.status, "need_user");
  const approval = eventFor(rpc, started.session.id, "approval_requested");
  assert.equal(approval.action.tool_call.name, "apply_patch");
  assert.equal(approval.action.tool_call.arguments.path, "notes.txt");
  assert.equal(approval.action.tool_call.arguments.old_text, "base\n");
  assert.equal(approval.action.tool_call.arguments.new_text, "next\n");

  // External edit between preview and approve.
  await writeFile(notesPath, "external\n", "utf8");

  const completed = await rpc.request("session.resolve", {
    session_id: started.session.id,
    action_id: approval.action.id,
    approved: true,
  });
  assert.equal(completed.session.status, "done");
  assert.match(completed.message?.content ?? "", /conflict/i);
  assert.equal(await readFile(notesPath, "utf8"), "external\n");

  const failedEnd = eventsFor(rpc, started.session.id).find(
    (event) =>
      event.type === "tool_end" &&
      event.tool_call?.name === "apply_patch" &&
      event.success === false,
  );
  assert.ok(failedEnd, "expected failed apply_patch tool_end");
  assert.match(String(failedEnd.summary), /patch conflict on notes\.txt/i);
  assert.match(String(failedEnd.summary), /re-read the file/i);

  const { detail } = await rpc.request("session.detail", { session_id: started.session.id });
  const toolMessage = detail.messages.find((message) => message.role === "tool");
  assert.ok(toolMessage, "expected tool role message after conflict");
  const payload = JSON.parse(toolMessage.content);
  assert.equal(payload.code, "patch_conflict");
  assert.equal(payload.path, "notes.txt");
  assert.match(String(payload.hint ?? ""), /read_file/i);
  assert.match(String(payload.error ?? ""), /patch conflict/i);
}

async function runAutoEditStaleConflict(rpc, mock, workspace) {
  const notesPath = resolve(workspace, "stale.txt");
  await writeFile(notesPath, "current\n", "utf8");
  mock.scenario = "auto-stale";
  rpc.events.length = 0;

  const completed = await rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Apply a stale patch to stale.txt.",
    model: "fixture-model",
    mode: "auto-edit",
  });
  assert.equal(completed.session.status, "done");
  assert.match(completed.message?.content ?? "", /conflict/i);
  assert.equal(await readFile(notesPath, "utf8"), "current\n");
  assert.equal(
    eventsFor(rpc, completed.session.id).some((event) => event.type === "approval_requested"),
    false,
  );

  const failedEnd = eventsFor(rpc, completed.session.id).find(
    (event) =>
      event.type === "tool_end" &&
      event.tool_call?.name === "apply_patch" &&
      event.success === false,
  );
  assert.ok(failedEnd, "expected failed apply_patch tool_end in auto-edit");
  assert.match(String(failedEnd.summary), /patch conflict on stale\.txt/i);

  const { detail } = await rpc.request("session.detail", { session_id: completed.session.id });
  const toolMessage = detail.messages.find((message) => message.role === "tool");
  assert.ok(toolMessage, "expected structured tool error message");
  const payload = JSON.parse(toolMessage.content);
  assert.equal(payload.code, "patch_conflict");
  assert.equal(payload.path, "stale.txt");
  assert.match(String(payload.hint ?? ""), /old_text/i);
}

function eventsFor(rpc, sessionId) {
  return rpc.events.filter((event) => event.session_id === sessionId);
}

function eventFor(rpc, sessionId, type) {
  const event = eventsFor(rpc, sessionId).find((item) => item.type === type);
  assert.ok(event, `session ${sessionId} should emit ${type}`);
  return event;
}

function startRpcClient({ databasePath, environment }) {
  const child = spawn(serverPath, ["--db", databasePath], {
    cwd: repositoryRoot,
    env: environment,
    stdio: ["pipe", "pipe", "pipe"],
    windowsHide: true,
  });
  const events = [];
  let outputBuffer = "";
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
  child.stderr.on("data", () => {});

  return {
    events,
    request(method, params = {}) {
      const id = ++requestId;
      const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params });
      return new Promise((resolveRequest, rejectRequest) => {
        pending.set(id, { resolve: resolveRequest, reject: rejectRequest });
        child.stdin.write(`${payload}\n`);
      });
    },
    async close() {
      for (const request of pending.values()) {
        request.reject(new Error("RPC client closed"));
      }
      pending.clear();
      if (child.exitCode !== null) return;
      child.stdin.end();
      await new Promise((resolveExit) => child.once("exit", resolveExit));
    },
  };
}

async function startMockProvider() {
  const state = { scenario: "ask-race" };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const messages = payload.messages ?? [];
    const toolMessages = messages.filter((message) => message.role === "tool");
    const hasToolResult = toolMessages.length > 0;

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

    if (state.scenario === "ask-race") {
      if (hasToolResult) {
        writeText(response, "Patch conflict detected; notes.txt was not modified.");
      } else {
        writeTool(response, "call_ask_patch", "apply_patch", {
          path: "notes.txt",
          old_text: "base\n",
          new_text: "next\n",
        });
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    if (state.scenario === "auto-stale") {
      if (hasToolResult) {
        writeText(response, "Patch conflict detected; stale.txt was not modified.");
      } else {
        writeTool(response, "call_auto_patch", "apply_patch", {
          path: "stale.txt",
          old_text: "stale\n",
          new_text: "next\n",
        });
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    writeText(response, "Unhandled scenario.");
    response.end("data: [DONE]\n\n");
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", resolveListen);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return {
    get scenario() {
      return state.scenario;
    },
    set scenario(value) {
      state.scenario = value;
    },
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close: () =>
      new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      ),
  };
}

function writeText(response, content) {
  response.write(`data: ${JSON.stringify({ choices: [{ delta: { content } }] })}\n\n`);
}

function writeTool(response, id, name, args) {
  response.write(
    `data: ${JSON.stringify({
      choices: [
        {
          delta: {
            tool_calls: [
              {
                index: 0,
                id,
                type: "function",
                function: { name, arguments: JSON.stringify(args) },
              },
            ],
          },
        },
      ],
    })}\n\n`,
  );
}

async function pathExists(path) {
  try {
    await readFile(path);
    return true;
  } catch {
    return false;
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
