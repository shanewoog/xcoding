import { spawn } from "node:child_process";
import { readdir, mkdir, mkdtemp, rm } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const e2eDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(e2eDirectory, "../..");
const runnerName = basename(fileURLToPath(import.meta.url));
const originalUserHome = process.env.USERPROFILE || process.env.HOME || homedir();
const defaultWorkers = Math.min(4, Math.max(1, Number(process.env.XCODING_E2E_WORKERS) || 4));

function parseArguments(args) {
  const options = {
    build: true,
    list: false,
    requireSelection: false,
    selectors: [],
    workers: defaultWorkers,
  };

  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--no-build") {
      options.build = false;
    } else if (argument === "--list") {
      options.list = true;
    } else if (argument === "--require-selection") {
      options.requireSelection = true;
    } else if (argument === "--workers") {
      const value = Number(args[index + 1]);
      if (!Number.isInteger(value) || value < 1) {
        throw new Error("--workers requires a positive integer");
      }
      options.workers = value;
      index += 1;
    } else if (argument.startsWith("--")) {
      throw new Error(`unknown option: ${argument}`);
    } else {
      options.selectors.push(argument);
    }
  }

  if (options.requireSelection && options.selectors.length === 0) {
    throw new Error("provide one or more e2e file names or patterns after --");
  }
  return options;
}

function selectorPattern(selector) {
  const name = basename(selector).replace(/[.+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^${name.replaceAll("*", ".*")}$`, "i");
}

async function discoverTests(selectors) {
  const entries = await readdir(e2eDirectory, { withFileTypes: true });
  const allTests = entries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".mjs") && entry.name !== runnerName)
    .map((entry) => entry.name)
    .sort();

  if (selectors.length === 0) return allTests;

  const patterns = selectors.map(selectorPattern);
  const selected = allTests.filter((name) => patterns.some((pattern) => pattern.test(name)));
  const unmatched = selectors.filter((selector) => {
    const pattern = selectorPattern(selector);
    return !allTests.some((name) => pattern.test(name));
  });
  if (unmatched.length > 0) {
    throw new Error(`no e2e tests matched: ${unmatched.join(", ")}`);
  }
  return selected;
}

function runProcess(command, args, options = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, {
      cwd: repositoryRoot,
      env: process.env,
      stdio: "inherit",
      windowsHide: true,
      ...options,
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} ${args.join(" ")} exited with ${code}`));
    });
  });
}

async function buildPrerequisites() {
  const pnpmCommand = process.platform === "win32" ? process.env.ComSpec || "cmd.exe" : "pnpm";
  const pnpmArgs = process.platform === "win32"
    ? ["/d", "/s", "/c", "pnpm", "--filter", "@xcoding/cli...", "build"]
    : ["--filter", "@xcoding/cli...", "build"];
  console.log("Building e2e prerequisites...");
  await Promise.all([
    runProcess("cargo", ["build", "-p", "xcoding-server"]),
    runProcess(pnpmCommand, pnpmArgs),
  ]);
}

function runTest(name, home) {
  return new Promise((resolvePromise) => {
    const startedAt = performance.now();
    const child = spawn(process.execPath, [join(e2eDirectory, name)], {
      cwd: repositoryRoot,
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        CARGO_HOME: process.env.CARGO_HOME || join(originalUserHome, ".cargo"),
        RUSTUP_HOME: process.env.RUSTUP_HOME || join(originalUserHome, ".rustup"),
      },
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      resolvePromise({ name, passed: false, seconds: (performance.now() - startedAt) / 1000, stdout, stderr, error });
    });
    child.on("close", (code) => {
      resolvePromise({ name, passed: code === 0, seconds: (performance.now() - startedAt) / 1000, stdout, stderr, code });
    });
  });
}

async function runTests(tests, workers, runRoot) {
  const results = new Array(tests.length);
  let nextIndex = 0;

  async function worker() {
    while (nextIndex < tests.length) {
      const index = nextIndex;
      nextIndex += 1;
      const name = tests[index];
      const home = join(runRoot, `${String(index + 1).padStart(2, "0")}-${name.slice(0, -4)}`);
      await mkdir(home, { recursive: true });
      const result = await runTest(name, home);
      results[index] = result;
      console.log(`${result.passed ? "PASS" : "FAIL"} ${name} (${result.seconds.toFixed(2)}s)`);
    }
  }

  await Promise.all(Array.from({ length: Math.min(workers, tests.length) }, () => worker()));
  return results;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  const tests = await discoverTests(options.selectors);
  if (options.list) {
    console.log(tests.join("\n"));
    return;
  }
  if (tests.length === 0) throw new Error("no e2e tests discovered");

  if (options.build) await buildPrerequisites();

  const runRoot = await mkdtemp(join(tmpdir(), "xcoding-e2e-run-"));
  const startedAt = performance.now();
  try {
    console.log(`Running ${tests.length} e2e tests with ${options.workers} worker(s)...`);
    const results = await runTests(tests, options.workers, runRoot);
    const failures = results.filter((result) => !result.passed);
    for (const failure of failures) {
      console.error(`\n--- ${failure.name} ---`);
      if (failure.stdout.trim()) console.error(failure.stdout.trimEnd());
      if (failure.stderr.trim()) console.error(failure.stderr.trimEnd());
      if (failure.error) console.error(failure.error);
      else console.error(`exited with ${failure.code}`);
    }
    console.log(
      `\nE2E summary: ${results.length - failures.length} passed, ${failures.length} failed in ${((performance.now() - startedAt) / 1000).toFixed(2)}s.`,
    );
    if (failures.length > 0) process.exitCode = 1;
  } finally {
    await rm(runRoot, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
