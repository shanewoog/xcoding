import assert from "node:assert/strict";
import { multiply } from "../src/calc.mjs";

assert.equal(multiply(3, 4), 12);
assert.equal(multiply(0, 9), 0);
console.log("feature tests passed");
