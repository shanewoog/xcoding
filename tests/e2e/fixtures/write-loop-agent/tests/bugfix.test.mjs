import assert from "node:assert/strict";
import { subtract } from "../src/calc.mjs";

assert.equal(subtract(10, 3), 7);
assert.equal(subtract(2, 5), -3);
console.log("bugfix tests passed");
