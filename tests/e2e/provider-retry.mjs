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

async function main() {
  await assertEmptyResponseFailsClearly();
  await assertRetryThenFail();
  await assertRetryThenSucceed();
  await assertFallsBackToAnotherConfiguredProvider();
  await assertSupportsMoreThanElevenToolRounds();
  await assertSkipsModelIncompatibleProviderWithinSession();
  await assertReconnectAfterSseDisconnect();
  await assertRestartsAfterPartialAnswerWasStreamed();
  await assertOpenCircuitDoesNotLockOutTheOnlyProvider();
  console.log("Provider retry E2E passed.");
}

function modelCallEvents(events, sessionId) {
  return events.filter((event) => event.type === "model_call" && event.session_id === sessionId);
}

async function configureSingleMockProvider(homeDirectory, baseUrl) {
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    `${JSON.stringify(
      {
        max_provider_retries: 5,
        base_url: baseUrl,
        providers: [{ id: "default", name: "openai", base_url: baseUrl }],
        active_provider_id: "default",
      },
      null,
      2,
    )}\n`,
    "utf8",
  );
}

async function persistedModelCallEvents(rpc, sessionId) {
  const { detail } = await rpc.request("session.detail", { session_id: sessionId });
  return detail.events
    .filter((item) => item.event.type === "model_call")
    .map((item) => item.event);
}

async function assertEmptyResponseFailsClearly() {
  const mock = await startFlakyProvider({
    succeedAfter: null,
    alwaysStatus: 503,
    emptyDone: true,
  });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-empty-response-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-empty-response-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    await assert.rejects(
      () =>
        rpc.request("session.chat", {
          workspace_root: fixtureRoot,
          message: "Explain this repository",
          model: "fixture-model",
        }),
      (error) => {
        assert.ok(error instanceof Error, "expected Error");
        assert.match(error.message, /model returned an empty response; please retry/i);
        assert.match(error.message, /HTTP 200/i);
        assert.match(error.message, /data: \[DONE\]/i);
        return true;
      },
    );

    // A clean SSE completion with no text/tool calls is a transient upstream failure.
    // The agent retries it before exposing a final failure to the user.
    assert.equal(mock.requests.length, 6, "expected initial attempt plus five empty-stream retries");
    const retryEvents = rpc.events.filter((event) => event.type === "retrying");
    assert.equal(retryEvents.length, 5, "expected one retry event for each empty stream");
    assert.deepEqual(retryEvents.map((event) => event.attempt), [1, 2, 3, 4, 5]);
    assert.ok(
      retryEvents.every(
        (event) =>
          /model returned an empty response; please retry/i.test(event.message) &&
          /HTTP 200/i.test(event.message) &&
          /data: \[DONE\]/i.test(event.message),
      ),
      "retry events should include the empty provider response diagnostics",
    );
    const listed = await rpc.request("session.list", { workspace_root: fixtureRoot });
    const sessions = listed.sessions ?? listed;
    assert.equal(sessions[0]?.status, "failed");
    assert.equal(
      rpc.events.filter((event) => event.type === "message_completed").length,
      0,
      "must not create a blank assistant message",
    );
    assert.equal(
      rpc.events.filter((event) => event.type === "task_completed").length,
      0,
      "failed sessions must not report task completion",
    );
    const errors = rpc.events.filter((event) => event.type === "error");
    assert.equal(errors.length, 1, "must emit one visible terminal error");
    assert.match(errors[0].message, /model returned an empty response; please retry/i);
    assert.match(errors[0].message, /HTTP 200/i);
    assert.match(errors[0].message, /data: \[DONE\]/i);
    const emptyCallEvents = modelCallEvents(rpc.events, sessions[0].id);
    assert.equal(emptyCallEvents.length, 6, "should log every empty-stream retry attempt");
    assert.deepEqual(emptyCallEvents.map((event) => event.attempt), [1, 2, 3, 4, 5, 6]);
    assert.ok(
      emptyCallEvents.every(
        (event) =>
          !event.success &&
          /HTTP 200/i.test(event.error ?? "") &&
          /data: \[DONE\]/i.test(event.error ?? ""),
      ),
      "empty-stream logs should retain the HTTP response diagnostics",
    );
    const persistedEmptyCalls = await persistedModelCallEvents(rpc, sessions[0].id);
    assert.equal(persistedEmptyCalls.length, 6, "should persist every empty-stream retry attempt");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertRetryThenFail() {
  const mock = await startFlakyProvider({ succeedAfter: null, alwaysStatus: 503 });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-fail-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-fail-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    await assert.rejects(
      () =>
        rpc.request("session.chat", {
          workspace_root: fixtureRoot,
          message: "Explain this repository",
          model: "fixture-model",
        }),
      (error) => {
        assert.ok(error instanceof Error, "expected Error");
        assert.match(error.message, /RPC 11\d{2}:/);
        assert.match(error.message, /Cloud provider request failed \(HTTP 503\)/);
        return true;
      },
    );

    // Initial attempt + 5 retries.
    assert.equal(mock.requests.length, 6, `expected 6 attempts, got ${mock.requests.length}`);

    const listed = await rpc.request("session.list", {
      workspace_root: fixtureRoot,
    });
    const sessions = listed.sessions ?? listed;
    assert.ok(Array.isArray(sessions));
    assert.ok(sessions.length >= 1);
    assert.equal(sessions[0].status, "failed", `expected failed, got ${sessions[0].status}`);

    const retryEvents = rpc.events.filter((event) => event.type === "retrying");
    assert.equal(retryEvents.length, 5, "expected one retry event for each reconnect");
    assert.deepEqual(retryEvents.map((event) => event.attempt), [1, 2, 3, 4, 5]);
    const errorEvents = rpc.events.filter((event) => event.type === "error");
    assert.ok(errorEvents.length >= 1, "expected session.event error notification");
    const failedCallEvents = modelCallEvents(rpc.events, sessions[0].id);
    assert.equal(failedCallEvents.length, 6, "should log every HTTP retry attempt");
    assert.ok(failedCallEvents.every((event) => !event.success && /HTTP 503/i.test(event.error ?? "") && /temporary upstream failure/i.test(event.error ?? "")));
    const persistedFailedCalls = await persistedModelCallEvents(rpc, sessions[0].id);
    assert.equal(persistedFailedCalls.length, 6);
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertRetryThenSucceed() {
  const mock = await startFlakyProvider({ succeedAfter: 3, alwaysStatus: 503 });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-ok-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-ok-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Say hello",
      model: "fixture-model",
    });
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.equal(mock.requests.length, 3, `expected 3 attempts, got ${mock.requests.length}`);
    const retryEvents = rpc.events.filter((event) => event.type === "retrying");
    assert.equal(retryEvents.length, 2);
    assert.deepEqual(retryEvents.map((event) => event.attempt), [1, 2]);
    const successfulCallEvents = modelCallEvents(rpc.events, result.session.id);
    assert.equal(successfulCallEvents.length, 3, "should log failures and the later successful call");
    assert.deepEqual(successfulCallEvents.map((event) => event.attempt), [1, 2, 3]);
    assert.ok(!successfulCallEvents[0].success && !successfulCallEvents[1].success);
    assert.ok(successfulCallEvents[2].success && successfulCallEvents[2].output_chars > 0);
    const persistedSuccessfulCalls = await persistedModelCallEvents(rpc, result.session.id);
    assert.equal(persistedSuccessfulCalls.length, 3);
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertFallsBackToAnotherConfiguredProvider() {
  const primary = await startFlakyProvider({ succeedAfter: null, alwaysStatus: 503 });
  const backup = await startFlakyProvider({ succeedAfter: 1, alwaysStatus: 503 });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-provider-fallback-db-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-provider-fallback-home-"));
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    JSON.stringify({
      locale: "en",
      mode: "ask",
      provider: "openai",
      model: "fixture-model",
      max_provider_retries: 0,
      circuit_failure_threshold: 1,
      stream_first_event_timeout_secs: 120,
      stream_idle_timeout_secs: 120,
      circuit_recovery_success_threshold: 2,
      circuit_recovery_wait_secs: 60,
      circuit_error_rate_threshold_percent: 100,
      circuit_min_request_count: 100,
      provider_fallback_enabled: true,
      base_url: primary.baseUrl,
      providers: [
        { id: "primary", name: "Primary", base_url: primary.baseUrl, api_key: "primary-test-key" },
        { id: "backup", name: "Backup", base_url: backup.baseUrl, api_key: "backup-test-key" },
      ],
      active_provider_id: "primary",
    }, null, 2) + "\n",
    "utf8",
  );
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
      message: "Use the configured backup provider",
      model: "fixture-model",
    });
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.equal(primary.requests.length, 1, "primary provider should receive one configured attempt");
    assert.equal(backup.requests.length, 1, "backup provider should receive the fallback attempt");
    assert.equal(primary.requests[0].model, "fixture-model");
    assert.equal(backup.requests[0].model, "fixture-model", "fallback must keep the session model");
    const switchEvent = rpc.events.find(
      (event) => event.type === "retrying" && /switching to backup provider/i.test(event.message),
    );
    assert.ok(switchEvent, "expected a visible provider-switch event");
  } finally {
    await rpc.close();
    await primary.close();
    await backup.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}
async function assertSupportsMoreThanElevenToolRounds() {
  const mock = await startFlakyProvider({
    succeedAfter: 1,
    alwaysStatus: 503,
    toolRoundsBeforeAnswer: 11,
  });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-tool-round-limit-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-tool-round-limit-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Inspect the workspace before answering.",
      model: "fixture-model",
    });
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.equal(mock.requests.length, 12, "eleven tool rounds must leave room for a final answer");
    const modelCalls = modelCallEvents(rpc.events, result.session.id);
    assert.equal(modelCalls.length, 12);
    assert.equal(modelCalls.filter((event) => event.tool_calls === 1).length, 11);
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertSkipsModelIncompatibleProviderWithinSession() {
  const incompatible = await startFlakyProvider({
    succeedAfter: null,
    alwaysStatus: 400,
    errorBody: { error: { message: "Unsupported model (model=fixture-model)", type: "invalid_parameter" } },
  });
  const compatible = await startFlakyProvider({
    succeedAfter: 1,
    alwaysStatus: 503,
    toolRoundsBeforeAnswer: 1,
  });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-model-incompatible-db-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-model-incompatible-home-"));
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    JSON.stringify({
      locale: "en",
      mode: "ask",
      provider: "openai",
      model: "fixture-model",
      max_provider_retries: 0,
      circuit_failure_threshold: 1,
      stream_first_event_timeout_secs: 120,
      stream_idle_timeout_secs: 120,
      circuit_recovery_success_threshold: 2,
      circuit_recovery_wait_secs: 60,
      circuit_error_rate_threshold_percent: 100,
      circuit_min_request_count: 100,
      provider_fallback_enabled: true,
      base_url: incompatible.baseUrl,
      providers: [
        { id: "incompatible", name: "Incompatible", base_url: incompatible.baseUrl, api_key: "incompatible-test-key" },
        { id: "compatible", name: "Compatible", base_url: compatible.baseUrl, api_key: "compatible-test-key" },
      ],
      active_provider_id: "incompatible",
    }, null, 2) + "\n",
    "utf8",
  );
  const { OPENAI_API_KEY, XCODING_OPENAI_BASE_URL, ...environment } = process.env;
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: { ...environment, HOME: homeDirectory, USERPROFILE: homeDirectory },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Inspect once, then answer.",
      model: "fixture-model",
    });
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.equal(incompatible.requests.length, 1, "unsupported provider must be skipped for later tool rounds");
    assert.equal(compatible.requests.length, 2);
    assert.equal(incompatible.requests[0].model, "fixture-model");
    assert.ok(
      rpc.events.some(
        (event) => event.type === "retrying" && /does not support model .*skipping it for this session/i.test(event.message),
      ),
      "expected a visible session-local model incompatibility event",
    );
  } finally {
    await rpc.close();
    await incompatible.close();
    await compatible.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertReconnectAfterSseDisconnect() {
  const mock = await startFlakyProvider({
    succeedAfter: 2,
    alwaysStatus: 503,
    disconnectBeforeDone: 1,
  });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-stream-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-stream-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Reconnect after a stream interruption",
      model: "fixture-model",
    });
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.equal(mock.requests.length, 2, `expected 2 attempts, got ${mock.requests.length}`);
    const retryEvents = rpc.events.filter((event) => event.type === "retrying");
    assert.equal(retryEvents.length, 1);
    assert.equal(retryEvents[0].attempt, 1);
    assert.match(retryEvents[0].message, /stream disconnected before completion/i);
    const reconnectCallEvents = modelCallEvents(rpc.events, result.session.id);
    assert.equal(reconnectCallEvents.length, 2, "should log the disconnect and reconnect success");
    assert.ok(!reconnectCallEvents[0].success && /stream disconnected before completion/i.test(reconnectCallEvents[0].error ?? ""));
    assert.ok(reconnectCallEvents[1].success);
    const persistedReconnectCalls = await persistedModelCallEvents(rpc, result.session.id);
    assert.equal(persistedReconnectCalls.length, 2, "should persist the disconnect and reconnect success");
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertRestartsAfterPartialAnswerWasStreamed() {
  const mock = await startFlakyProvider({
    succeedAfter: 2,
    alwaysStatus: 503,
    partialTextBeforeDisconnect: 1,
  });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-partial-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-retry-partial-home-"));
  await configureSingleMockProvider(homeDirectory, mock.baseUrl);
  const rpc = startRpcClient({
    databasePath: resolve(databaseDirectory, "xcoding.db"),
    environment: {
      ...process.env,
      OPENAI_API_KEY: "e2e-test-key",
      XCODING_OPENAI_BASE_URL: mock.baseUrl,
      HOME: homeDirectory,
      USERPROFILE: homeDirectory,
    },
  });

  try {
    const result = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "Restart after a half-streamed answer",
      model: "fixture-model",
    });
    // Losing the stream mid-answer used to fail the whole turn on the first hit.
    assert.equal(result.session.status, "done");
    assert.match(result.message?.content ?? "", /hello after retries/i);
    assert.doesNotMatch(result.message?.content ?? "", /partial answer that never finished/i);
    assert.equal(mock.requests.length, 2, `expected 2 attempts, got ${mock.requests.length}`);
    const retryEvents = rpc.events.filter((event) => event.type === "retrying");
    assert.equal(retryEvents.length, 1, "the interrupted attempt must be retried, not failed");
    assert.match(retryEvents[0].message, /stream disconnected before completion/i);
    const resetEvents = rpc.events.filter((event) => event.type === "stream_reset");
    assert.equal(resetEvents.length, 1, "clients must be told to drop the partial text");
    assert.ok(resetEvents[0].discarded_chars > 0, "reset should report the discarded length");
    assert.ok(
      rpc.events.findIndex((event) => event.type === "stream_reset")
        < rpc.events.findIndex((event) => event.type === "retrying"),
      "the reset must arrive before the retry so no duplicate text renders",
    );
    assert.equal(
      rpc.events.filter((event) => event.type === "error").length,
      0,
      "a recovered turn must not surface a terminal error",
    );
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
    await rm(homeDirectory, { recursive: true, force: true });
  }
}

async function assertOpenCircuitDoesNotLockOutTheOnlyProvider() {
  const mock = await startFlakyProvider({ succeedAfter: 2, alwaysStatus: 503 });
  const databaseDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-circuit-lockout-"));
  const homeDirectory = await mkdtemp(resolve(tmpdir(), "xcoding-e2e-circuit-lockout-home-"));
  const configDirectory = resolve(homeDirectory, ".xcoding");
  await mkdir(configDirectory, { recursive: true });
  await writeFile(
    resolve(configDirectory, "config.json"),
    JSON.stringify({
      locale: "en",
      mode: "ask",
      provider: "openai",
      model: "fixture-model",
      max_provider_retries: 0,
      // One failure opens the circuit for ten minutes, which used to make every
      // later turn fail with "circuit is open" until the process was restarted.
      circuit_failure_threshold: 1,
      circuit_recovery_wait_secs: 600,
      circuit_recovery_success_threshold: 2,
      circuit_error_rate_threshold_percent: 100,
      circuit_min_request_count: 100,
      stream_first_event_timeout_secs: 120,
      stream_idle_timeout_secs: 120,
      provider_fallback_enabled: false,
      base_url: mock.baseUrl,
      providers: [
        { id: "only", name: "Only", base_url: mock.baseUrl, api_key: "only-test-key" },
      ],
      active_provider_id: "only",
    }, null, 2) + "\n",
    "utf8",
  );
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
    await assert.rejects(() =>
      rpc.request("session.chat", {
        workspace_root: fixtureRoot,
        message: "First turn hits the upstream failure",
        model: "fixture-model",
      }),
    );
    assert.equal(mock.requests.length, 1, "the first turn spends the single configured attempt");

    const recovered = await rpc.request("session.chat", {
      workspace_root: fixtureRoot,
      message: "The next turn must still reach the provider",
      model: "fixture-model",
    });
    assert.equal(recovered.session.status, "done");
    assert.match(recovered.message?.content ?? "", /hello after retries/i);
    assert.equal(mock.requests.length, 2, "the open circuit must still allow a probe request");
    assert.equal(
      rpc.events.filter((event) => /circuit is open/i.test(event.message ?? "")).length,
      0,
      "a single configured provider must never be locked out by its own circuit",
    );
  } finally {
    await rpc.close();
    await mock.close();
    await rm(databaseDirectory, { recursive: true, force: true });
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

/**
 * @param {{ succeedAfter: number | null, alwaysStatus: number, disconnectBeforeDone?: number, emptyDone?: boolean, toolRoundsBeforeAnswer?: number, errorBody?: object }} options
 * succeedAfter: 1-based attempt number that starts returning SSE success.
 * null means never succeed.
 */
async function startFlakyProvider({
  succeedAfter,
  alwaysStatus,
  disconnectBeforeDone = 0,
  partialTextBeforeDisconnect = 0,
  emptyDone = false,
  toolRoundsBeforeAnswer = 0,
  errorBody = { error: { message: "temporary upstream failure", type: "server_error" } },
}) {
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) {
      chunks.push(chunk);
    }
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    const attempt = requests.length;
    // Streams a visible answer prefix and then drops the connection, which is how
    // a gateway that stops forwarding events mid-answer looks to the client.
    if (attempt <= partialTextBeforeDisconnect) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(
        `data: ${JSON.stringify({ choices: [{ delta: { content: "partial answer that never finished" } }] })}\n\n`,
      );
      response.end();
      return;
    }
    if (attempt <= disconnectBeforeDone) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end();
      return;
    }
    if (emptyDone) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.end("data: [DONE]\n\n");
      return;
    }
    if (attempt <= toolRoundsBeforeAnswer) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(
        `data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, id: `call_list_root_${attempt}`, type: "function", function: { name: "list_dir", arguments: JSON.stringify({ path: "." }) } }] } }] })}\n\n`,
      );
      response.end("data: [DONE]\n\n");
      return;
    }
    if (succeedAfter != null && attempt >= succeedAfter) {
      response.writeHead(200, { "content-type": "text/event-stream" });
      response.write(
        `data: ${JSON.stringify({ choices: [{ delta: { content: "hello after retries" } }] })}\n\n`,
      );
      response.end("data: [DONE]\n\n");
      return;
    }
    response.writeHead(alwaysStatus, { "content-type": "application/json" });
    response.end(JSON.stringify(errorBody));
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
