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
const PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==";

async function main() {
  const mock = await startAnthropicProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-anthropic-db-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-anthropic-home-"));
  await writeAnthropicConfig(homeDirectory, mock.baseUrl);
  const { OPENAI_API_KEY, XCODING_OPENAI_BASE_URL, ...environment } = process.env;
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...environment,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Inspect this image and list the repository root before answering.",
      model: "claude-fixture-model",
      images: [{ mime_type: "image/png", data_base64: PNG_BASE64, name: "screen.png" }],
    });

    assert.equal(result.session.status, "done");
    assert.match(result.message.content, /src\/auth\.ts/);
    assert.equal(mock.requests.length, 2, "the tool result should be returned in a second Messages request");

    const first = mock.requests[0];
    assert.equal(first.method, "POST");
    assert.equal(first.url, "/v1/messages");
    assert.equal(first.headers.authorization, undefined, "Anthropic requests must not send Bearer auth");
    assert.equal(first.headers["x-api-key"], "anthropic-test-key");
    assert.equal(first.headers["anthropic-version"], "2023-06-01");
    assert.equal(first.body.model, "claude-fixture-model");
    assert.equal(first.body.max_tokens, 8192);
    assert.equal(first.body.stream, true);
    assert.match(first.body.system, /coding agent/i);

    const firstUser = first.body.messages.find((message) => message.role === "user");
    assert.ok(firstUser, "the first request should contain the user message");
    assert.ok(
      firstUser.content.some(
        (block) => block.type === "text" && /Inspect this image/.test(block.text),
      ),
      "the user text should be converted to an Anthropic text block",
    );
    const image = firstUser.content.find((block) => block.type === "image");
    assert.deepEqual(image?.source, {
      type: "base64",
      media_type: "image/png",
      data: PNG_BASE64,
    });

    const listDirectory = first.body.tools.find((tool) => tool.name === "list_dir");
    assert.ok(listDirectory, "the Anthropic request should expose the list_dir tool");
    assert.equal(listDirectory.input_schema.type, "object");
    assert.equal(first.body.tool_choice.type, "auto");

    const second = mock.requests[1];
    assert.equal(second.url, "/v1/messages");
    assert.equal(second.headers.authorization, undefined);
    assert.equal(second.headers["x-api-key"], "anthropic-test-key");
    const toolUse = findBlock(second.body.messages, "assistant", "tool_use");
    assert.deepEqual(toolUse, {
      type: "tool_use",
      id: "toolu_list_root",
      name: "list_dir",
      input: { path: "." },
    });
    const toolResult = findBlock(second.body.messages, "user", "tool_result");
    assert.equal(toolResult.tool_use_id, "toolu_list_root");
    assert.match(textFromBlocks(toolResult.content), /src/);

    const toolStart = rpc.events.find((event) => event.type === "tool_start");
    assert.deepEqual(toolStart?.tool_call, {
      id: "toolu_list_root",
      name: "list_dir",
      arguments: { path: "." },
    });
    assert.equal(rpc.events.find((event) => event.type === "tool_end")?.success, true);
    assert.ok(rpc.events.some((event) => event.type === "text_delta"));

    const modelCalls = rpc.events.filter((event) => event.type === "model_call");
    assert.equal(modelCalls.length, 2);
    assert.ok(modelCalls.every((event) => event.success));
    assert.ok(modelCalls.every((event) => event.model_reported === "claude-fixture-reported"));
    assert.equal(modelCalls[0].tool_calls, 1);
    assert.equal(modelCalls[1].tool_calls, 0);

    console.log("Anthropic Messages E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

function findBlock(messages, role, type) {
  for (const message of messages) {
    if (message.role !== role || !Array.isArray(message.content)) continue;
    const block = message.content.find((item) => item.type === type);
    if (block) return block;
  }
  assert.fail(`expected ${role} ${type} block`);
}

function textFromBlocks(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n");
}

async function writeAnthropicConfig(homeDirectory, baseUrl) {
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    `${JSON.stringify(
      {
        locale: "en",
        mode: "ask",
        provider: "anthropic",
        model: "claude-fixture-model",
        max_provider_retries: 0,
        provider_fallback_enabled: false,
        base_url: baseUrl,
        api_key: "anthropic-test-key",
        providers: [
          {
            id: "anthropic",
            name: "Anthropic fixture",
            base_url: baseUrl,
            api_key: "anthropic-test-key",
            wire_api: "anthropic_messages",
            trust_level: "official",
          },
        ],
        active_provider_id: "anthropic",
        model_capabilities: { "claude-fixture-model": { supports_vision: true } },
      },
      null,
      2,
    )}\n`,
    "utf8",
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
  child.once("error", (error) => rejectPending(error));
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
      return new Promise((resolveRequest, rejectRequest) => {
        pending.set(id, { resolve: resolveRequest, reject: rejectRequest });
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      });
    },
    close() {
      return new Promise((resolveClose) => {
        if (child.exitCode !== null) {
          resolveClose();
          return;
        }
        child.once("exit", resolveClose);
        child.kill();
      });
    },
  };
}

async function startAnthropicProvider() {
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    requests.push({
      method: request.method,
      url: request.url,
      headers: request.headers,
      body,
    });

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    writeEvent(response, {
      type: "message_start",
      message: {
        id: `msg_${requests.length}`,
        type: "message",
        role: "assistant",
        model: "claude-fixture-reported",
        content: [],
        stop_reason: null,
        usage: { input_tokens: requests.length === 1 ? 120 : 180, output_tokens: 0 },
      },
    });

    if (requests.length === 1) {
      writeEvent(response, {
        type: "content_block_start",
        index: 0,
        content_block: { type: "tool_use", id: "toolu_list_root", name: "list_dir", input: {} },
      });
      writeEvent(response, {
        type: "content_block_delta",
        index: 0,
        delta: { type: "input_json_delta", partial_json: '{"path"' },
      });
      writeEvent(response, {
        type: "content_block_delta",
        index: 0,
        delta: { type: "input_json_delta", partial_json: ':"."}' },
      });
      writeEvent(response, { type: "content_block_stop", index: 0 });
      writeEvent(response, {
        type: "message_delta",
        delta: { stop_reason: "tool_use", stop_sequence: null },
        usage: { output_tokens: 12 },
      });
    } else {
      writeEvent(response, {
        type: "content_block_start",
        index: 0,
        content_block: { type: "thinking", thinking: "", signature: "fixture-signature" },
      });
      writeEvent(response, {
        type: "content_block_delta",
        index: 0,
        delta: { type: "thinking_delta", thinking: "Checking the tool result." },
      });
      writeEvent(response, { type: "content_block_stop", index: 0 });
      writeEvent(response, {
        type: "content_block_start",
        index: 1,
        content_block: { type: "text", text: "" },
      });
      writeEvent(response, {
        type: "content_block_delta",
        index: 1,
        delta: { type: "text_delta", text: "The repository contains src/auth.ts." },
      });
      writeEvent(response, { type: "content_block_stop", index: 1 });
      writeEvent(response, {
        type: "message_delta",
        delta: { stop_reason: "end_turn", stop_sequence: null },
        usage: { output_tokens: 21 },
      });
    }
    writeEvent(response, { type: "message_stop" });
    response.end();
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
    close: () =>
      new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      ),
  };
}

function writeEvent(response, event) {
  response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`);
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
