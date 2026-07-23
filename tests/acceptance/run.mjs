import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const args = process.argv.slice(2);
const runLive = args.includes("--live");
const deterministicOnly = args.includes("--deterministic") || !runLive;
const pnpmCommand = process.platform === "win32" ? "pnpm.cmd" : "pnpm";

function loadDotEnv() {
  const path = resolve(repositoryRoot, ".env");
  if (!existsSync(path)) {
    return;
  }
  for (const rawLine of readFileSync(path, "utf8").split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#") || !line.includes("=")) {
      continue;
    }
    const index = line.indexOf("=");
    const key = line.slice(0, index).trim();
    let value = line.slice(index + 1).trim();
    if (
      (value.startsWith("\"") && value.endsWith("\"")) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }
    if (!process.env[key]) {
      process.env[key] = value;
    }
  }
}

function run(command, commandArgs, options = {}) {
  return new Promise((resolvePromise, reject) => {
    // Avoid shell:true with paths containing spaces on Windows.
    const child = spawn(command, commandArgs, {
      cwd: repositoryRoot,
      env: process.env,
      stdio: "inherit",
      shell: false,
      windowsHide: true,
      ...options,
    });
    child.on("error", reject);
    child.on("exit", (code) => {
      if (code === 0) {
        resolvePromise();
      } else {
        reject(new Error(`${command} ${commandArgs.join(" ")} exited with ${code}`));
      }
    });
  });
}

async function runDeterministic() {
  console.log("== V1 acceptance: deterministic e2e ==");
  await run("cargo", ["build", "-p", "xcoding-server"]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/read-only-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/guarded-write-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/auto-edit-mode.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/command-allowlist.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/running-cancel-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/session-replay-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/write-loop-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/git-tools-agent.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/provider-auth-error.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/provider-status.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/doctor.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/desktop-review.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/desktop-activity.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/desktop-layout.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/desktop-config.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/task-summary.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/session-continue.mjs")]);
  await run(process.execPath, [resolve(repositoryRoot, "tests/e2e/surface-parity.mjs")]);
  console.log("Deterministic acceptance passed.");
}

async function runLiveSmoke() {
  console.log("== V1 acceptance: live cloud smoke ==");
  loadDotEnv();
  if (!process.env.OPENAI_API_KEY) {
    throw new Error("OPENAI_API_KEY missing for --live (set env or create repo-root .env)");
  }
  if (!process.env.XCODING_OPENAI_BASE_URL) {
    process.env.XCODING_OPENAI_BASE_URL = "https://ai.v58.dev/v1";
  }
  const cli = resolve(repositoryRoot, "apps/cli/dist/index.js");
  if (!existsSync(cli)) {
    await run(pnpmCommand, ["--filter", "@xcoding/cli", "build"]);
  }
  await run(process.execPath, [
    cli,
    "chat",
    "用一句话说明 monorepo 根仓库是做什么的",
    "--workspace",
    repositoryRoot,
  ]);
  console.log("Live cloud smoke passed.");
}

async function main() {
  loadDotEnv();
  const summary = [
    "V1 acceptance matrix",
    "- automated deterministic: tasks 1-10",
    "- live optional: task 1 smoke",
  ];
  console.log(summary.join("\n"));

  if (deterministicOnly || runLive) {
    await runDeterministic();
  }
  if (runLive) {
    await runLiveSmoke();
  }

  console.log("Acceptance harness finished.");
  console.log("See tests/acceptance/README.md for the full 10-task matrix.");
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
