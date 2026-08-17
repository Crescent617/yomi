import { describe, expect, test } from "vitest";
import { splitDetailsBlocks } from "./markdown-details";

describe("splitDetailsBlocks", () => {
  test("extracts block with summary", () => {
    const { text, blocks } = splitDetailsBlocks(
      "前文\n<details>\n<summary>展开细节</summary>\n内容 **加粗**\n</details>\n后文",
    );
    expect(blocks).toEqual([{ summary: "展开细节", body: "内容 **加粗**" }]);
    expect(text).toContain("%%YOMI-DETAILS-0%%");
    expect(text).toContain("前文");
    expect(text).toContain("后文");
    expect(text).not.toContain("</details>");
  });

  test("block without summary gets empty summary", () => {
    const { blocks } = splitDetailsBlocks("<details>\n只有正文\n</details>");
    expect(blocks).toEqual([{ summary: "", body: "只有正文" }]);
  });

  test("multiple blocks index in order", () => {
    const { text, blocks } = splitDetailsBlocks(
      "<details>\n<summary>一</summary>\n甲\n</details>\n\n<details>\n<summary>二</summary>\n乙\n</details>",
    );
    expect(blocks.map((b) => b.summary)).toEqual(["一", "二"]);
    expect(text).toContain("%%YOMI-DETAILS-0%%");
    expect(text).toContain("%%YOMI-DETAILS-1%%");
  });

  test("unterminated block stays as raw text", () => {
    const raw = "<details>\n<summary>标题</summary>\n还没闭合";
    const { text, blocks } = splitDetailsBlocks(raw);
    expect(blocks).toEqual([]);
    expect(text).toBe(raw);
  });

  test("summary with inline html keeps inner text", () => {
    const { blocks } = splitDetailsBlocks(
      "<details>\n<summary>带 <code>code</code> 的标题</summary>\nx\n</details>",
    );
    expect(blocks[0].summary).toBe("带 <code>code</code> 的标题");
  });
});

test("details inside fenced code stays as raw text", () => {
  const raw =
    "用法：\n```\n<details>\n<summary>示例</summary>\nx\n</details>\n```\n<details>\n<summary>真的</summary>\ny\n</details>";
  const { text, blocks } = splitDetailsBlocks(raw);
  expect(blocks).toEqual([{ summary: "真的", body: "y" }]);
  expect(text).toContain("<summary>示例</summary>");
  expect(text).toContain("%%YOMI-DETAILS-0%%");
});
