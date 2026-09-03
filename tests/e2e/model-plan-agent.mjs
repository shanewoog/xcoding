import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtureRoot = resolve(repositoryRoot, "tests/e2e/fixtures/model-plan-agent");
const binaryName = process.platform === "win32" ? "xcoding-server.exe" : "xcoding-server";
const serverPath = resolve(repositoryRoot, "target/debug", binaryName);

// Deliberately not three steps: the model owns the count, the backend must not reshape it.
const MODEL_PLAN = [
  { description: "Read src/session.ts and locate resumeSession", status: "done" },
  { description: "List the callers that depend on the resumed prefix", status: "done" },
  { description: "Patch resumeSession to keep the prefix stable", status: "in_progress" },
  { description: "Add a regression test for the resumed prefix" },
  { description: "Run the session tests and report the result" },
];

async function main() {
  const mock = await startMockProvider();
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-model-plan-"));
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Keep the resumed prefix stable in src/session.ts.",
      model: "fixture-model",
    });

    assert.equal(result.session.status, "done");

    const planEvents = rpc.events.filter((event) => event.type === "plan");
    assert.equal(planEvents.length, 2, "the scaffold plan then the model-authored plan");
    assert.equal(planEvents[0].steps.length, 3, "pre-turn scaffold stays a three-step plan");

    const modelPlan = planEvents[1].steps;
    assert.equal(modelPlan.length, MODEL_PLAN.length, "model chooses the step count");
    assert.deepEqual(
      modelPlan.map((step) => step.description),
      MODEL_PLAN.map((step) => step.description),
    );
    assert.deepEqual(
      modelPlan.map((step) => step.status),
      ["done", "done", "in_progress", "pending", "pending"],
      "explicit statuses survive, omitted status defaults to pending",
    );
    assert.deepEqual(
      modelPlan.map((step) => step.id),
      ["step_1", "step_2", "step_3", "step_4", "step_5"],
    );

    const planToolEnd = rpc.events.find(
      (event) => event.type === "tool_end" && event.tool_call.name === "update_plan",
    );
    assert.equal(planToolEnd?.success, true);
    assert.equal(planToolEnd?.summary, "Updated plan (2/5 done)");

    const { detail } = await rpc.request("session.detail", { session_id: result.session.id });
    const persisted = detail.events.filter((item) => item.event.type === "plan").at(-1)?.event.steps;
    assert.equal(persisted?.length, MODEL_PLAN.length, "the plan is replayable from history");

    assert.equal(mock.requests.length, 2);
    assert.ok(
      mock.requests[0].tools.some((tool) => tool.function.name === "update_plan"),
      "update_plan is offered to the model",
    );
    const planTool = mock.requests[0].tools.find((tool) => tool.function.name === "update_plan");
    assert.equal(planTool.function.parameters.properties.steps.minItems, 1);
    assert.ok(
      !/exactly (three|six|ten|\d+) steps/i.test(planTool.function.description),
      "the tool must not prescribe a fixed step count",
    );

    console.log("Model-authored plan agent E2E passed.");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
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
  const rejectPending = (error) => {
    for (const { reject } of pending.values()) {
      reject(error);
    }
    pending.clear();
  };
  child.once("error", rejectPending);
  child.once("exit", (code) => {
    if (pending.size > 0) {
      rejectPending(new Error(`xcoding-server exited with ${code}: ${diagnostics.trim()}`));
    }
  });

  return {
    events,
    request(method, params) {
      const id = ++requestId;
      const response = new Promise((resolveRequest, reject) => {
        pending.set(id, { resolve: resolveRequest, reject });
      });
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      return response;
    },
    async close() {
      if (child.exitCode !== null) {
        return;
      }
      child.stdin.end();
      await new Promise((resolveExit) => child.once("exit", resolveExit));
    },
  };
}

async function startMockProvider() {
  const requests = [];
  let turn = 0;
  const server = createServer(async (request, response) => {
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    const chunks = [];
    for await (const chunk of request) {
      chunks.push(chunk);
    }
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));

    response.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
    });
    if (turn++ === 0) {
      response.write(
        `data: ${JSON.stringify({
          choices: [
            {
              delta: {
                tool_calls: [
                  {
                    index: 0,
                    id: "call_update_plan",
                    type: "function",
                    function: {
                      name: "update_plan",
                      arguments: JSON.stringify({ steps: MODEL_PLAN }),
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
          choices: [{ delta: { content: "Plan recorded for src/session.ts." } }],
        })}\n\n`,
      );
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
    close: () =>
      new Promise((resolveClose, rejectClose) =>
        server.close((error) => (error ? rejectClose(error) : resolveClose())),
      ),
  };
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : String(error));
  process.exitCode = 1;
});
