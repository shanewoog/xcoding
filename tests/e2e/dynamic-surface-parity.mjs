import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const driverName = process.platform === "win32" ? "desktop_trajectory.exe" : "desktop_trajectory";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);
const desktopDriverPath = resolve(repositoryRoot, "target/debug/examples", driverName);
const prompt = "What source files are in this repository?";

async function main() {
  const root = await mkdtemp(resolve(tmpdir(), "xcoding-dynamic-parity-"));
  const workspace = resolve(root, "workspace");
  await mkdir(resolve(workspace, "src"), { recursive: true });
  await writeFile(resolve(workspace, "src/auth.ts"), "export const login = () => true;\n", "utf8");

  const cliProvider = await startMockProvider();
  const desktopProvider = await startMockProvider();
  const cliHome = resolve(root, "cli-home");
  const desktopHome = resolve(root, "desktop-home");
  await Promise.all([
    writeProviderConfig(cliHome, cliProvider.baseUrl),
    writeProviderConfig(desktopHome, desktopProvider.baseUrl),
  ]);
  const cli = startRpcClient({
    databasePath: resolve(root, "cli.db"),
    environment: fixtureEnvironment(cliProvider.baseUrl, cliHome),
  });

  try {
    const cliResult = await cli.request("session.chat", chatParams(workspace));
    const cliReplay = await cli.request("session.replay", { session_id: cliResult.session.id });
    const desktop = await runDesktopTrajectory({
      databasePath: resolve(root, "desktop.db"),
      workspace,
      environment: fixtureEnvironment(desktopProvider.baseUrl, desktopHome),
    });

    assert.equal(cliResult.session.status, "done");
    assert.equal(desktop.result.session.status, "done");
    assert.equal(cliProvider.requests.length, 2);
    assert.equal(desktopProvider.requests.length, 2);
    assert.deepEqual(
      normalizeProviderRequests(cliProvider.requests),
      normalizeProviderRequests(desktopProvider.requests),
      "CLI/server and Desktop core should send the same provider trajectory",
    );
    assert.deepEqual(
      normalizeLiveEvents(cli.events),
      normalizeLiveEvents(desktop.events),
      "CLI/server and Desktop core should emit the same live event trajectory",
    );
    assert.deepEqual(
      normalizeReplay(cliReplay),
      normalizeReplay(desktop.replay),
      "CLI/server and Desktop core should persist the same replay and task summary",
    );
    assert.deepEqual(
      normalizeResult(cliResult),
      normalizeResult(desktop.result),
      "CLI/server and Desktop core should return the same terminal result",
    );

    const replayKinds = cliReplay.steps.map((step) => step.kind);
    for (const kind of ["plan", "tool_start", "tool_end", "assistant_message", "task_completed"]) {
      assert.ok(replayKinds.includes(kind), `dynamic parity replay missing ${kind}`);
    }
    console.log("Dynamic CLI/Desktop trajectory parity passed.");
  } finally {
    await cli.close();
    await Promise.all([cliProvider.close(), desktopProvider.close()]);
    await rm(root, { recursive: true, force: true });
  }
}

async function writeProviderConfig(home, baseUrl) {
  const configDirectory = resolve(home, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    `${JSON.stringify({
      max_provider_retries: 0,
      provider_fallback_enabled: false,
      providers: [{
        id: "default",
        name: "openai",
        base_url: baseUrl,
        api_key: "dynamic-parity-key",
        trust_level: "official",
      }],
      active_provider_id: "default",
    })}\n`,
    "utf8",
  );
}

function fixtureEnvironment(baseUrl, home) {
  return {
    ...process.env,
    HOME: home,
    USERPROFILE: home,
    OPENAI_API_KEY: "dynamic-parity-key",
    XCODING_OPENAI_BASE_URL: baseUrl,
    XCODING_OPENAI_MODEL: "fixture-model",
  };
}

function chatParams(workspace) {
  return {
    workspace_root: workspace,
    message: prompt,
    model: "fixture-model",
    mode: "ask",
  };
}

function normalizeResult(result) {
  return {
    session: {
      workspace_root: "<workspace>",
      title: result.session.title,
      mode: result.session.mode,
      provider: result.session.provider,
      model: result.session.model,
      status: result.session.status,
    },
    message: result.message && {
      role: result.message.role,
      content: result.message.content,
    },
  };
}

function normalizeProviderRequests(requests) {
  return requests.map((request) => ({
    model: request.model,
    stream: request.stream,
    messages: request.messages,
    tools: request.tools,
    tool_choice: request.tool_choice,
  }));
}

function normalizeLiveEvents(events) {
  return events
    .filter((event) => !["model_call"].includes(event.type))
    .map(normalizeEvent);
}

function normalizeReplay(replay) {
  return {
    session: normalizeResult({ session: replay.session, message: null }).session,
    events: replay.events
      .map((item) => item.event)
      .filter((event) => !["model_call"].includes(event.type))
      .map(normalizeEvent),
    steps: replay.steps,
  };
}

function normalizeEvent(event) {
  return normalizeValue(event, new Set([
    "session_id",
    "id",
    "created_at",
    "updated_at",
    "workspace_root",
  ]));
}

function normalizeValue(value, omittedKeys) {
  if (Array.isArray(value)) return value.map((entry) => normalizeValue(entry, omittedKeys));
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value)
      .filter(([key]) => !omittedKeys.has(key))
      .map(([key, entry]) => [key, normalizeValue(entry, omittedKeys)]),
  );
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
  child.once("error", rejectPending);
  child.once("exit", (code) => {
    if (pending.size > 0) rejectPending(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
  });

  function rejectPending(error) {
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }

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

function runDesktopTrajectory({ databasePath, workspace, environment }) {
  return new Promise((resolveRun, rejectRun) => {
    const child = spawn(desktopDriverPath, [databasePath, workspace, prompt, "fixture-model"], {
      cwd: repositoryRoot,
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.once("error", rejectRun);
    child.once("exit", (code) => {
      if (code !== 0) {
        rejectRun(new Error(`desktop trajectory driver exited with ${code}: ${stderr.trim()}`));
        return;
      }
      try {
        resolveRun(JSON.parse(stdout));
      } catch (error) {
        rejectRun(new Error(`invalid desktop trajectory output: ${error.message}\n${stdout}\n${stderr}`));
      }
    });
  });
}

async function startMockProvider() {
  const requests = [];
  let turn = 0;
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    if (turn++ === 0) {
      response.write('data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_list_root","type":"function","function":{"name":"list_dir","arguments":"{\\"path\\":\\".\\"}"}}]}}]}\n\n');
    } else {
      response.write('data: {"choices":[{"delta":{"content":"The repository contains src/auth.ts."}}]}\n\n');
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
    requests,
    close: () => new Promise((resolveClose, rejectClose) => server.close((error) => error ? rejectClose(error) : resolveClose())),
  };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
