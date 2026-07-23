import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureSource = resolve(repositoryRoot, "tests/e2e/fixtures/write-loop-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

const INITIAL_CALC = `export function add(a, b) {
  return a + b;
}

// Intentionally wrong for the bugfix acceptance path.
export function subtract(a, b) {
  return a + b;
}
`;

const FEATURE_CALC = `export function add(a, b) {
  return a + b;
}

// Intentionally wrong for the bugfix acceptance path.
export function subtract(a, b) {
  return a + b;
}

export function multiply(a, b) {
  return a * b;
}
`;

const FIXED_CALC = `export function add(a, b) {
  return a + b;
}

export function subtract(a, b) {
  return a - b;
}
`;

const REFACTORED_CALC = `export function add(a, b) {
  // behavior-preserving refactor: same arithmetic result
  const left = Number(a);
  const right = Number(b);
  return left + right;
}

// Intentionally wrong for the bugfix acceptance path.
export function subtract(a, b) {
  return a + b;
}
`;

async function main() {
  assert.ok(await pathExists(serverPath), `missing server binary at ${serverPath}; run cargo build -p xcoding-server`);

  const mock = await startMockProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-write-loop-db-"));
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    await runFeatureLoop(rpc, mock);
    await runBugfixLoop(rpc, mock);
    await runRefactorLoop(rpc, mock);
    console.log("Write-loop agent E2E passed (feature + bugfix + refactor).");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
  }
}

async function runFeatureLoop(rpc, mock) {
  const workspace = await prepareWorkspace("feature");
  mock.scenario = "feature";
  try {
    const started = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Add multiply(a, b) and run the feature tests.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(started.session.status, "need_user");
    assert.equal(approvalFor(rpc, started.session.id).action.tool_call.name, "run_command");
    assert.equal(await readFile(resolve(workspace, "src/calc.mjs"), "utf8"), FEATURE_CALC);

    const completed = await approveUntilDone(rpc, started.session.id);
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /feature complete/i);

    const { detail } = await rpc.request("session.detail", { session_id: started.session.id });
    const summary = detail.events.find((item) => item.event.type === "task_completed")?.event.summary;
    assert.ok(summary);
    assert.deepEqual(summary.changed_files, ["src/calc.mjs"]);
    assert.equal(summary.commands_run, 1);
    assert.equal(summary.commands_succeeded, 1);
    assert.equal(summary.commands_failed, 0);
    assert.ok(
      detail.events.some(
        (item) => item.event.type === "tool_start" && item.event.tool_call?.name === "apply_patch",
      ),
    );
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}

async function runBugfixLoop(rpc, mock) {
  const workspace = await prepareWorkspace("bugfix");
  mock.scenario = "bugfix";
  try {
    const started = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Reproduce the subtract bug with tests, then fix it.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(started.session.status, "need_user");
    assert.equal(approvalFor(rpc, started.session.id).action.tool_call.name, "run_command");
    assert.equal(await readFile(resolve(workspace, "src/calc.mjs"), "utf8"), INITIAL_CALC);

    const afterFail = await approveOnce(rpc, started.session.id);
    assert.equal(afterFail.session.status, "need_user");
    assert.equal(await readFile(resolve(workspace, "src/calc.mjs"), "utf8"), FIXED_CALC);
    assert.equal(latestPendingApproval(rpc, started.session.id).action.tool_call.name, "run_command");

    const completed = await approveUntilDone(rpc, started.session.id);
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /bug fixed/i);

    const { detail } = await rpc.request("session.detail", { session_id: started.session.id });
    const summary = detail.events.find((item) => item.event.type === "task_completed")?.event.summary;
    assert.ok(summary);
    assert.deepEqual(summary.changed_files, ["src/calc.mjs"]);
    assert.equal(summary.commands_run, 2);
    assert.equal(summary.commands_succeeded, 1);
    assert.equal(summary.commands_failed, 1);
    assert.ok(
      detail.events.some(
        (item) =>
          item.event.type === "tool_end" &&
          item.event.tool_call?.name === "run_command" &&
          item.event.success === false,
      ),
    );
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}

async function runRefactorLoop(rpc, mock) {
  const workspace = await prepareWorkspace("refactor");
  mock.scenario = "refactor";
  try {
    const started = await rpc.request("session.chat", {
      workspace_root: workspace,
      message: "Refactor add without changing behavior and re-run tests.",
      model: "fixture-model",
      mode: "auto-edit",
    });
    assert.equal(started.session.status, "need_user");
    assert.equal(approvalFor(rpc, started.session.id).action.tool_call.name, "run_command");

    const afterBaseline = await approveOnce(rpc, started.session.id);
    assert.equal(afterBaseline.session.status, "need_user");
    assert.equal(await readFile(resolve(workspace, "src/calc.mjs"), "utf8"), REFACTORED_CALC);

    const completed = await approveUntilDone(rpc, started.session.id);
    assert.equal(completed.session.status, "done");
    assert.match(completed.message?.content ?? "", /refactor complete/i);

    const { detail } = await rpc.request("session.detail", { session_id: started.session.id });
    const summary = detail.events.find((item) => item.event.type === "task_completed")?.event.summary;
    assert.ok(summary);
    assert.deepEqual(summary.changed_files, ["src/calc.mjs"]);
    assert.equal(summary.commands_run, 2);
    assert.equal(summary.commands_succeeded, 2);
    assert.equal(summary.commands_failed, 0);
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
}

async function prepareWorkspace(label) {
  const workspace = await mkdtemp(resolve(tmpdir(), `xcoding-write-loop-${label}-`));
  await cp(fixtureSource, workspace, { recursive: true });
  return workspace;
}

async function approveOnce(rpc, sessionId) {
  const action = latestPendingApproval(rpc, sessionId);
  return rpc.request("session.resolve", {
    session_id: sessionId,
    action_id: action.action.id,
    approved: true,
  });
}

async function approveUntilDone(rpc, sessionId) {
  let result = { session: { id: sessionId, status: "need_user" }, message: null };
  for (let attempt = 0; attempt < 8; attempt += 1) {
    if (result.session.status === "done") {
      return result;
    }
    if (result.session.status !== "need_user") {
      throw new Error(`unexpected session status while driving approvals: ${result.session.status}`);
    }
    result = await approveOnce(rpc, sessionId);
  }
  throw new Error(`session ${sessionId} did not reach done after approvals`);
}

function eventsFor(rpc, sessionId) {
  return rpc.events.filter((event) => event.session_id === sessionId);
}

function approvalFor(rpc, sessionId) {
  const approval = eventsFor(rpc, sessionId).find((event) => event.type === "approval_requested");
  assert.ok(approval, `session ${sessionId} should request approval`);
  return approval;
}

function latestPendingApproval(rpc, sessionId) {
  const approvals = eventsFor(rpc, sessionId).filter((event) => event.type === "approval_requested");
  assert.ok(approvals.length > 0, `session ${sessionId} should have an approval request`);
  return approvals[approvals.length - 1];
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
  const state = { scenario: "feature", requests: [] };
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    state.requests.push(payload);
    const messages = payload.messages ?? [];
    // After session.resolve, historical tool rows are rehydrated as assistant notes.
    // Include both role:tool and those notes so multi-step mocks stay deterministic.
    const toolContents = collectToolContents(messages);

    response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    writeScenario(response, state.scenario, toolContents);
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
    requests: state.requests,
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    close() {
      return new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      );
    },
  };
}

function collectToolContents(messages) {
  const contents = [];
  for (const message of messages) {
    const content = String(message.content ?? "");
    if (message.role === "tool") {
      contents.push(content);
      continue;
    }
    if (message.role === "assistant" && content.startsWith("Previously recorded tool output: ")) {
      contents.push(content.slice("Previously recorded tool output: ".length));
    }
  }
  return contents;
}

function writeScenario(response, scenario, toolContents) {
  if (scenario === "feature") {
    if (toolContents.some((content) => content.includes('"success":true') && content.includes("feature.test.mjs"))) {
      writeText(response, "Feature complete: multiply added and tests passed.");
      return;
    }
    if (toolContents.some((content) => content.includes('"changed":true') && content.includes("src/calc.mjs"))) {
      writeTool(response, "call_feature_test", "run_command", {
        executable: "node",
        args: ["tests/feature.test.mjs"],
      });
      return;
    }
    writeTool(response, "call_feature_patch", "apply_patch", {
      path: "src/calc.mjs",
      old_text: INITIAL_CALC,
      new_text: FEATURE_CALC,
    });
    return;
  }

  if (scenario === "bugfix") {
    if (toolContents.some((content) => content.includes('"success":true') && content.includes("bugfix.test.mjs"))) {
      writeText(response, "Bug fixed: subtract now passes the failing tests.");
      return;
    }
    // Prefer the successful patch over the earlier failed repro command.
    if (toolContents.some((content) => content.includes('"changed":true') && content.includes("src/calc.mjs"))) {
      writeTool(response, "call_bugfix_retest", "run_command", {
        executable: "node",
        args: ["tests/bugfix.test.mjs"],
      });
      return;
    }
    if (toolContents.some((content) => content.includes('"success":false') && content.includes("bugfix.test.mjs"))) {
      writeTool(response, "call_bugfix_patch", "apply_patch", {
        path: "src/calc.mjs",
        old_text: INITIAL_CALC,
        new_text: FIXED_CALC,
      });
      return;
    }
    writeTool(response, "call_bugfix_repro", "run_command", {
      executable: "node",
      args: ["tests/bugfix.test.mjs"],
    });
    return;
  }

  if (scenario === "refactor") {
    const successfulTests = toolContents.filter(
      (content) => content.includes('"success":true') && content.includes("refactor.test.mjs"),
    ).length;
    if (successfulTests >= 2) {
      writeText(response, "Refactor complete: tests still pass after the rewrite.");
      return;
    }
    if (toolContents.some((content) => content.includes('"changed":true') && content.includes("src/calc.mjs"))) {
      writeTool(response, "call_refactor_retest", "run_command", {
        executable: "node",
        args: ["tests/refactor.test.mjs"],
      });
      return;
    }
    if (successfulTests >= 1) {
      writeTool(response, "call_refactor_patch", "apply_patch", {
        path: "src/calc.mjs",
        old_text: INITIAL_CALC,
        new_text: REFACTORED_CALC,
      });
      return;
    }
    writeTool(response, "call_refactor_baseline", "run_command", {
      executable: "node",
      args: ["tests/refactor.test.mjs"],
    });
    return;
  }

  writeText(response, "Unexpected write-loop mock scenario.");
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
                function: {
                  name,
                  arguments: JSON.stringify(args),
                },
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
