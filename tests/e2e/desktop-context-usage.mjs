import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { mock } from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";

const app = await readFile(new URL("../../apps/desktop/src/App.tsx", import.meta.url), "utf8");
const source = ts.createSourceFile("App.tsx", app, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const effects = [];
function visit(node) {
  if (ts.isCallExpression(node) && node.expression.getText(source) === "useEffect") {
    const dependencies = node.arguments[1];
    if (dependencies && ts.isArrayLiteralExpression(dependencies)
      && dependencies.elements.some((element) => element.getText(source) === "contextUsageOpen")) {
      effects.push(node);
    }
  }
  ts.forEachChild(node, visit);
}
visit(source);
assert.equal(effects.length, 1, "context usage must have an auto-close effect");
assert.deepEqual(effects[0].arguments[1].elements.map((element) => element.getText(source)),
  ["contextUsageOpen"], "streaming updates must not restart the countdown");
assert.ok(app.includes("onClick={() => setContextUsageOpen((open) => !open)}"),
  "clicking the info button must still toggle the popover manually");

mock.timers.enable({ apis: ["setTimeout"] });
try {
  const updates = [];
  const runEffect = (open) => runInNewContext(
    "(" + effects[0].arguments[0].getText(source) + ")()",
    {
      contextUsageOpen: open,
      setContextUsageOpen: (value) => updates.push(value),
      window: { setTimeout, clearTimeout },
    },
  );

  assert.equal(runEffect(false), undefined);
  mock.timers.tick(10_000);
  assert.deepEqual(updates, [], "a closed popover must not schedule state updates");

  let cleanup = runEffect(true);
  assert.equal(typeof cleanup, "function", "the timer must have effect cleanup");
  mock.timers.tick(9_999);
  assert.deepEqual(updates, [], "the popover must remain open before ten seconds");
  mock.timers.tick(1);
  assert.deepEqual(updates, [false], "the popover must close at ten seconds");
  cleanup();
  updates.length = 0;

  cleanup = runEffect(true);
  mock.timers.tick(4_000);
  cleanup();
  runEffect(false);
  mock.timers.tick(1_000);
  cleanup = runEffect(true);
  mock.timers.tick(5_000);
  assert.deepEqual(updates, [], "the old timer must not close a reopened popover");
  mock.timers.tick(4_999);
  assert.deepEqual(updates, [], "reopening must start a fresh ten seconds");
  mock.timers.tick(1);
  assert.deepEqual(updates, [false]);
  cleanup();
  updates.length = 0;

  cleanup = runEffect(true);
  cleanup();
  cleanup = runEffect(true);
  mock.timers.tick(10_000);
  assert.deepEqual(updates, [false], "effect replay must not leave duplicate timers");
  cleanup();
  updates.length = 0;

  cleanup = runEffect(true);
  mock.timers.tick(1_000);
  cleanup();
  mock.timers.tick(10_000);
  assert.deepEqual(updates, [], "unmounting must cancel the pending update");
} finally {
  mock.timers.reset();
}

console.log("Desktop context usage auto-close regression passed.");
