import test from "node:test";
import assert from "node:assert/strict";

import { beijingDay } from "../static-assets/views/news.js";

test("periodical dates are shown in Beijing time", () => {
  assert.equal(beijingDay("2026-09-25T17:30:00+00:00"), "2026-09-26");
  assert.equal(beijingDay("2026-12-31T16:00:00+00:00"), "2027-01-01");
  assert.equal(beijingDay("2026-12-31T15:59:59+00:00"), "2026-12-31");
  assert.equal(beijingDay("2026-09-26T12:30:00Z"), "2026-09-26");
  assert.equal(beijingDay("2026-09-26 07:00"), "2026-09-26");
  assert.equal(beijingDay("2026-09-26"), "2026-09-26");
  assert.equal(beijingDay(""), "");
  assert.equal(beijingDay(null), "");
  assert.equal(beijingDay("第 12 期"), "第 12 期");
});
