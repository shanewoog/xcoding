import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = resolve(repositoryRoot, "tests/e2e/fixtures/read-only-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

await assertStableSystemInstructions("responses");
await assertStableSystemInstructions("chat_completions");
console.log("Stable system instructions E2E passed (Responses and Chat Completions).");

async function assertStableSystemInstructions(wireApi) {
  const requests = [];
  const urls = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    urls.push(request.url);
    response.writeHead(200, { "content-type": "text/event-stream" });
    const first = requests.length === 1;
    const call = { name: "list_dir", arguments: JSON.stringify({ path: "." }) };
    if (wireApi === "responses") {
      const event = first
        ? { type: "response.output_item.done", item: { type: "function_call", call_id: "call_list", ...call } }
        : { type: "response.output_text.delta", delta: "Directory inspected." };
      response.write(`data: ${JSON.stringify(event)}\n\n`);
      response.end('data: {"type":"response.completed","response":{"status":"completed"}}\n\n');
    } else {
      const delta = first
        ? { tool_calls: [{ index: 0, id: "call_list", type: "function", function: call }] }
        : { content: "Directory inspected." };
      response.write(`data: ${JSON.stringify({ choices: [{ delta }] })}\n\n`);
      response.end("data: [DONE]\n\n");
    }
  });
  await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
  const baseUrl = `http://127.0.0.1:${server.address().port}/v1`;
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-stable-instructions-"));
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(resolve(configDirectory, "config.json"), JSON.stringify({
    base_url: baseUrl,
    providers: [{ id: "default", name: "mock", base_url: baseUrl, wire_api: wireApi, api_key: "e2e-test-key" }],
    active_provider_id: "default",
    local_memory_enabled: false,
    lossy_context_compaction_enabled: false,
    provider_fallback_enabled: false,
    max_provider_retries: 0,
  }), "utf8");
  const rpc = startRpcClient({
    databasePath: resolve(homeDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: baseUrl,
      XCODING_HTTP_PROXY: "direct",
    },
  });
  try {
    const first = await rpc.request("session.chat", {
      workspace_root: fixtureRoot, message: "List the workspace directory.", model: "gpt-6-astra", mode: "ask",
    });
    assert.equal(first.session.status, "done");
    assert.equal(requests.length, 2, "a tool round must produce a second model request");
    const followUp = await rpc.request("session.chat", {
      workspace_root: fixtureRoot, session_id: first.session.id,
      message: "Confirm you inspected the directory.", model: "gpt-6-astra", mode: "ask",
    });
    assert.equal(followUp.session.id, first.session.id);
    assert.equal(followUp.session.status, "done");
    assert.equal(requests.length, 3);
    const expectedUrl = wireApi === "responses" ? "/v1/responses" : "/v1/chat/completions";
    assert.ok(urls.every((url) => url === expectedUrl));
    const instructions = requests.map((body) => body.instructions ?? body.messages?.[0]?.content);
    assert.ok(instructions[0].length > 0, "the real system prompt must still be sent");
    assert.ok(instructions.every((text) => text === instructions[0]),
      `${wireApi}: tool rounds and follow-ups must not rewrite system instructions with changing token counts`);
    assert.ok(instructions.every((text) => !text.includes("<token_budget>")),
      "dynamic token budgets must stay local, outside the model's system instructions");
    for (const body of requests) {
      assert.deepEqual(body.tools, requests[0].tools);
      assert.equal(body.prompt_cache_key, first.session.id);
    }
    if (wireApi === "responses") {
      assert.ok(requests[1].input.length > requests[0].input.length);
      assert.deepEqual(requests[1].input.slice(0, requests[0].input.length), requests[0].input);
    } else {
      assert.ok(requests[1].messages.length > requests[0].messages.length);
      assert.deepEqual(requests[1].messages.slice(0, requests[0].messages.length), requests[0].messages);
    }
  } finally {
    await rpc.close();
    await new Promise((resolveClose) => server.close(resolveClose));
    // Only remove the absolute directory allocated by mkdtemp for this test.
    assert.equal(dirname(homeDirectory), resolve(tmpdir()));
    await rm(homeDirectory, { recursive: true, force: true });
  }
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
      if (!line) {
        continue;
      }
      const message = JSON.parse(line);
      if (message.method === "session.event") {
        events.push(message.params);
        continue;
      }
      const request = pending.get(message.id);
      if (!request) {
        continue;
      }
      pending.delete(message.id);
      if (message.error) {
        request.reject(new Error(`RPC ${message.error.code}: ${message.error.message}`));
      } else {
        request.resolve(message.result);
      }
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => {
    diagnostics += chunk;
  });
  child.once("error", (error) => rejectPending(error));
  child.once("exit", (code) => {
    if (pending.size > 0) {
      rejectPending(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
    }
  });

  function rejectPending(error) {
    for (const { reject } of pending.values()) {
      reject(error);
    }
    pending.clear();
  }

  return {
    events,
    request(method, params) {
      const id = ++requestId;
      const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params });
      return new Promise((resolveRequest, rejectRequest) => {
        pending.set(id, { resolve: resolveRequest, reject: rejectRequest });
        child.stdin.write(`${payload}\n`);
      });
    },
    close() {
      return new Promise((resolveClose) => {
        child.once("exit", () => resolveClose());
        if (!child.killed) {
          child.kill();
        }
      });
    },
  };
}
