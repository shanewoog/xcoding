import assert from "node:assert/strict";
import test from "node:test";
import { selectPlan } from "./verify-affected.mjs";

test("desktop source selects only desktop checks and contracts", () => {
  const plan = selectPlan(["apps/desktop/src/panels.tsx"]);
  assert.equal(plan.full, false);
  assert.deepEqual(plan.checks.map((check) => check.label), ["desktop TypeScript check"]);
  assert.deepEqual(plan.e2e, [
    "desktop-activity.mjs",
    "desktop-config.mjs",
    "desktop-gitnexus.mjs",
    "desktop-layout.mjs",
    "desktop-message-links.mjs",
    "desktop-review.mjs",
    "surface-parity.mjs",
    "task-summary.mjs",
  ]);
  assert.equal(plan.e2eBuild, false);
});

test("a server crate selects its package tests and built e2e", () => {
  const plan = selectPlan(["crates/xcoding-server/src/lib.rs"]);
  assert.equal(plan.full, false);
  assert.deepEqual(plan.checks.map((check) => check.label), ["xcoding-server Rust tests"]);
  assert.equal(plan.e2e.length, 0);
  assert.equal(plan.e2eBuild, true);
});

test("runtime crate changes run crate tests and rebuilt e2e", () => {
  const plan = selectPlan(["crates/xcoding-core/src/lib.rs"]);
  assert.equal(plan.full, false);
  assert.deepEqual(plan.checks.map((check) => check.args), [["test", "-p", "xcoding-core"]]);
  assert.equal(plan.e2eBuild, true);
});

test("temporary files do not broaden the affected scope", () => {
  const plan = selectPlan(["_tmp_debug.py", "tmp-fix.js", "docs/notes.md"]);
  assert.deepEqual(plan.files, ["docs/notes.md"]);
  assert.equal(plan.full, false);
  assert.equal(plan.checks.length, 0);
  assert.equal(plan.e2e.length, 0);
});

test("unknown source changes conservatively fall back to full verification", () => {
  const plan = selectPlan(["tools/custom-build.mjs"]);
  assert.equal(plan.full, true);
  assert.deepEqual(plan.checks.map((check) => check.label), ["full TypeScript check"]);
  assert.equal(plan.e2eBuild, true);
});

test("deleted source files are treated as affected changes", () => {
  const plan = selectPlan(["apps/desktop/src/removed-panel.tsx"]);
  assert.equal(plan.full, false);
  assert.deepEqual(plan.checks.map((check) => check.label), ["desktop TypeScript check"]);
  assert.equal(plan.e2e.length, 8);
});

test("explicit test selection remains available", () => {
  const plan = selectPlan(["tests/e2e/doctor.mjs"]);
  assert.deepEqual(plan.e2e, ["doctor.mjs"]);
  assert.equal(plan.e2eBuild, false);
});
