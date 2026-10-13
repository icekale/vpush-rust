import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { escapeHtml } from "../static-assets/core/html.js";
import { createImaView } from "../static-assets/views/ima.js";

function readerExtractionHtml(item) {
  const view = createImaView({ escapeHtml, state: {} });
  return view.imaReaderExtractionHtml(item);
}

test("AI 解读 card renders thesis, rating, target price, and ticker links", () => {
  const html = readerExtractionHtml({
    extraction: {
      status: "ok",
      rating: "增持",
      target_price: "25.00元",
      thesis: "AI 算力驱动需求高增。",
      tickers: [
        { code: "002428", name: "云南锗业", stance: "推荐" },
        { code: "A&B", name: "名称<险>", stance: "" },
      ],
    },
  });
  assert.match(html, /<summary><span>AI 解读<\/span><\/summary>/);
  assert.match(html, /<p>AI 算力驱动需求高增。<\/p>/);
  assert.match(html, /ima-ex-badge ima-ex-rating">增持</);
  assert.match(html, /ima-ex-badge">25\.00元</);
  assert.match(html, /href="\/ticker\/002428"/);
  assert.match(html, />云南锗业</);
  assert.match(html, /href="\/ticker\/A%26B"/);
  assert.match(html, />名称&lt;险&gt;</);
  assert.doesNotMatch(html, /<script/);
});

test("AI 解读 is omitted unless status is ok and a field is present", () => {
  assert.equal(readerExtractionHtml({}), "");
  assert.equal(readerExtractionHtml({ extraction: { status: "failed", thesis: "失败也有字" } }), "");
  assert.equal(readerExtractionHtml({
    extraction: { status: "ok", rating: "", target_price: "", thesis: "  ", tickers: [] },
  }), "");
  const thesisOnly = readerExtractionHtml({
    extraction: { status: "ok", thesis: "只有一句逻辑。" },
  });
  assert.match(thesisOnly, /AI 解读/);
  assert.match(thesisOnly, /只有一句逻辑。/);
  assert.doesNotMatch(thesisOnly, /ima-ex-badge/);
});

test("reader places AI 解读 before the IMA abstract", () => {
  const source = readFileSync(new URL("../static-assets/views/ima.js", import.meta.url), "utf8");
  const info = source.slice(source.indexOf('<section class="ima-reader-info">'));
  const extractionAt = info.indexOf("${extractionHtml}");
  const abstractAt = info.indexOf("${abstractHtml}");
  assert.ok(extractionAt > 0 && abstractAt > extractionAt);
  assert.match(source, /isFeishuTimeline \? "" : imaReaderExtractionHtml\(item\)/);
  assert.match(source, /imaUsableAbstract\(item\.abstract_zh\) \|\| imaUsableAbstract\(item\.abstract\)/);
});
