import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureSource = resolve(repositoryRoot, "tests/e2e/fixtures/read-only-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

async function main() {
  const mock = await startMockProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-cancel-e2e-"));
  const workspace = await mkdtemp(resolve(tmpdir(), "xcoding-cancel-workspace-"));
  await cp(fixtureSource, workspace, { recursive: true });
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    await testMidStreamCancel(rpc, mock, workspace);
    await testMidCommandCancel(rpc, mock, workspace);
    await testFailedCommandRefeed(rpc, mock, workspace);
    await testAutoEditStillRequiresCommandApproval(rpc, mock, workspace);
    console.log("Running cancel / command-path E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(workspace, { recursive: true, force: true });
  }
}

async function testMidStreamCancel(rpc, mock, workspace) {
  mock.scenario = "slow-stream";
  const chatPromise = rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Stream slowly please.",
    model: "fixture-model",
    mode: "ask",
  });
  const delta = await waitForEvent(rpc, (event) => event.type === "text_delta", 10_000);
  const cancel = await rpc.request("session.cancel", { session_id: delta.session_id });
  assert.equal(cancel.session.status, "cancelled");
  const chat = await chatPromise;
  assert.equal(chat.session.status, "cancelled");
  assert.equal(chat.message ?? null, null);
  assert.ok(rpc.events.some((event) => event.type === "session_cancelled" && event.session_id === delta.session_id));
}

async function testMidCommandCancel(rpc, mock, workspace) {
  mock.scenario = "long-command";
  const started = await rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Run a long command.",
    model: "fixture-model",
    mode: "ask",
  });
  assert.equal(started.session.status, "need_user");
  const approval = waitForExisting(rpc, started.session.id, "approval_requested");
  assert.equal(approval.action.tool_call.name, "run_command");

  const resolvePromise = rpc.request("session.resolve", {
    session_id: started.session.id,
    action_id: approval.action.id,
    approved: true,
  });
  await waitForEvent(
    rpc,
    (event) => event.type === "tool_start" && event.session_id === started.session.id && event.tool_call?.name === "run_command",
    10_000,
  );
  // Give the child process a moment to start before cancelling.
  await delay(200);
  const cancel = await rpc.request("session.cancel", { session_id: started.session.id });
  assert.equal(cancel.session.status, "cancelled");
  const resolved = await resolvePromise;
  assert.equal(resolved.session.status, "cancelled");
  assert.equal(resolved.message ?? null, null);
}

async function testFailedCommandRefeed(rpc, mock, workspace) {
  mock.scenario = "failed-command";
  const started = await rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Run a failing command.",
    model: "fixture-model",
    mode: "ask",
  });
  assert.equal(started.session.status, "need_user");
  const approval = waitForExisting(rpc, started.session.id, "approval_requested");
  const completed = await rpc.request("session.resolve", {
    session_id: started.session.id,
    action_id: approval.action.id,
    approved: true,
  });
  assert.equal(completed.session.status, "done");
  assert.match(completed.message?.content ?? "", /failed command was reported/i);
  const toolEnd = rpc.events.find(
    (event) => event.type === "tool_end" && event.session_id === started.session.id,
  );
  assert.equal(toolEnd?.success, true);
  const { detail } = await rpc.request("session.detail", { session_id: started.session.id });
  const toolMessage = detail.messages.find((message) => message.role === "tool");
  assert.match(toolMessage?.content ?? "", /"success":false/);
}

async function testAutoEditStillRequiresCommandApproval(rpc, mock, workspace) {
  mock.scenario = "auto-edit-command";
  const started = await rpc.request("session.chat", {
    workspace_root: workspace,
    message: "Auto edit then command.",
    model: "fixture-model",
    mode: "auto-edit",
  });
  // First tool is apply_patch and should auto-run; then run_command should pause.
  assert.equal(started.session.status, "need_user");
  const approval = waitForExisting(rpc, started.session.id, "approval_requested");
  assert.equal(approval.action.tool_call.name, "run_command");
  assert.ok(
    rpc.events.some(
      (event) =>
        event.type === "tool_end" &&
        event.session_id === started.session.id &&
        event.tool_call?.name === "apply_patch" &&
        event.success === true,
    ),
  );
  const rejected = await rpc.request("session.resolve", {
    session_id: started.session.id,
    action_id: approval.action.id,
    approved: false,
  });
  assert.equal(rejected.session.status, "done");
  assert.match(rejected.message?.content ?? "", /command remains gated/i);
}

function waitForExisting(rpc, sessionId, type) {
  const event = rpc.events.find((item) => item.session_id === sessionId && item.type === type);
  assert.ok(event, `expected existing event ${type} for ${sessionId}`);
  return event;
}

function waitForEvent(rpc, predicate, timeoutMs) {
  const existing = rpc.events.find(predicate);
  if (existing) {
    return Promise.resolve(existing);
  }
  return new Promise((resolveEvent, reject) => {
    const started = Date.now();
    const timer = setInterval(() => {
      const match = rpc.events.find(predicate);
      if (match) {
        clearInterval(timer);
        resolveEvent(match);
        return;
      }
      if (Date.now() - started > timeoutMs) {
        clearInterval(timer);
        reject(new Error(`timed out waiting for event after ${timeoutMs}ms`));
      }
    }, 25);
  });
}

function delay(ms) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms));
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
  child.stderr.on("data", (chunk) => {
    diagnostics += chunk;
  });
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
  const state = { scenario: "slow-stream", requests: [] };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    state.requests.push(payload);
    const messages = payload.messages ?? [];
    const hasToolResult = messages.some((message) => message.role === "tool");

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });

    if (state.scenario === "slow-stream") {
      response.write(
        `data: ${JSON.stringify({ choices: [{ delta: { content: "Streaming slowly..." } }] })}\n\n`,
      );
      // Keep the SSE stream open so the agent stays inside stream.next() until cancel.
      await delay(15_000);
      if (!response.writableEnded) {
        response.end("data: [DONE]\n\n");
      }
      return;
    }

    if (state.scenario === "long-command") {
      if (hasToolResult) {
        response.write(
          `data: ${JSON.stringify({ choices: [{ delta: { content: "Command finished." } }] })}\n\n`,
        );
      } else {
        const args =
          process.platform === "win32"
            ? ["127.0.0.1", "-n", "30"]
            : ["30"];
        const executable = process.platform === "win32" ? "ping" : "sleep";
        response.write(
          `data: ${JSON.stringify({
            choices: [
              {
                delta: {
                  tool_calls: [
                    {
                      index: 0,
                      id: "call_long_command",
                      type: "function",
                      function: {
                        name: "run_command",
                        arguments: JSON.stringify({ executable, args }),
                      },
                    },
                  ],
                },
              },
            ],
          })}\n\n`,
        );
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    if (state.scenario === "failed-command") {
      if (hasToolResult) {
        response.write(
          `data: ${JSON.stringify({
            choices: [{ delta: { content: "The failed command was reported to the agent loop." } }],
          })}\n\n`,
        );
      } else {
        const executable = process.platform === "win32" ? "cmd" : "false";
        const args = process.platform === "win32" ? ["/c", "exit", "1"] : [];
        response.write(
          `data: ${JSON.stringify({
            choices: [
              {
                delta: {
                  tool_calls: [
                    {
                      index: 0,
                      id: "call_failed_command",
                      type: "function",
                      function: {
                        name: "run_command",
                        arguments: JSON.stringify({ executable, args }),
                      },
                    },
                  ],
                },
              },
            ],
          })}\n\n`,
        );
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    if (state.scenario === "auto-edit-command") {
      const toolMessages = messages.filter((message) => message.role === "tool");
      const hasRejection = toolMessages.some((message) =>
        String(message.content ?? "").includes('"rejected":true'),
      );
      if (hasRejection || toolMessages.length >= 2) {
        response.write(
          `data: ${JSON.stringify({
            choices: [{ delta: { content: "The command remains gated behind approval." } }],
          })}\n\n`,
        );
      } else if (hasToolResult) {
        // After auto patch, request a command that still needs approval.
        response.write(
          `data: ${JSON.stringify({
            choices: [
              {
                delta: {
                  tool_calls: [
                    {
                      index: 0,
                      id: "call_gated_command",
                      type: "function",
                      function: {
                        name: "run_command",
                        arguments: JSON.stringify({
                          executable: process.platform === "win32" ? "cmd" : "true",
                          args: process.platform === "win32" ? ["/c", "echo", "gated"] : [],
                        }),
                      },
                    },
                  ],
                },
              },
            ],
          })}\n\n`,
        );
      } else {
        response.write(
          `data: ${JSON.stringify({
            choices: [
              {
                delta: {
                  tool_calls: [
                    {
                      index: 0,
                      id: "call_auto_patch",
                      type: "function",
                      function: {
                        name: "apply_patch",
                        arguments: JSON.stringify({
                          path: "auto-command.txt",
                          old_text: "",
                          new_text: "auto then command\n",
                        }),
                      },
                    },
                  ],
                },
              },
            ],
          })}\n\n`,
        );
      }
      response.end("data: [DONE]\n\n");
      return;
    }

    response.write(
      `data: ${JSON.stringify({ choices: [{ delta: { content: "Unexpected mock scenario." } }] })}\n\n`,
    );
    response.end("data: [DONE]\n\n");
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once("error", rejectListen);
    server.listen(0, "127.0.0.1", () => resolveListen());
  });
  const address = server.address();
  return {
    get scenario() {
      return state.scenario;
    },
    set scenario(value) {
      state.scenario = value;
    },
    requests: state.requests,
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close() {
      return new Promise((resolveClose) => server.close(() => resolveClose()));
    },
  };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
