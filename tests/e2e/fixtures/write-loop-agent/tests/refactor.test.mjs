import assert from "node:assert/strict";
import { add } from "../src/calc.mjs";

assert.equal(add(2, 3), 5);
assert.equal(add(-1, 1), 0);
assert.equal(add(0, 0), 0);
console.log("refactor tests passed");
