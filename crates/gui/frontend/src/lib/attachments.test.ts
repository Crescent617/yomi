import { describe, expect, it } from "vitest";
import { parseAttachments } from "./attachments";

// Mirrors `kernel::utils::attachments::tests` — keep in sync.

describe("parseAttachments", () => {
  it("returns text unchanged when there is no block", () => {
    const text = "  hello <yomi_attachments> world\n";
    const { cleaned, paths } = parseAttachments(text);
    expect(cleaned).toBe(text);
    expect(paths).toEqual([]);
  });

  it("strips a trailing block", () => {
    const { cleaned, paths } = parseAttachments(
      "report done\n\n<yomi_attachments>\nout.pdf\n data.csv \n</yomi_attachments>\n",
    );
    expect(cleaned).toBe("report done");
    expect(paths).toEqual(["out.pdf", "data.csv"]);
  });

  it("leaves empty text for a block-only message", () => {
    const { cleaned, paths } = parseAttachments(
      "<yomi_attachments>\na.pdf\n</yomi_attachments>",
    );
    expect(cleaned).toBe("");
    expect(paths).toEqual(["a.pdf"]);
  });

  it("recognizes a mid-text block (only fence parity matters)", () => {
    const { cleaned, paths } = parseAttachments(
      "before <yomi_attachments>a.pdf</yomi_attachments> after",
    );
    expect(cleaned).toBe("before  after");
    expect(paths).toEqual(["a.pdf"]);
  });

  it("keeps text after the declaration", () => {
    const { cleaned, paths } = parseAttachments(
      "done\n<yomi_attachments>\nout.pdf\n</yomi_attachments>\n附件如上",
    );
    expect(cleaned).toBe("done\n\n附件如上");
    expect(paths).toEqual(["out.pdf"]);
  });

  it("strips a declaration followed by a balanced fenced block", () => {
    const { cleaned, paths } = parseAttachments(
      "<yomi_attachments>\nout.pdf\n</yomi_attachments>\n```\ncode\n```",
    );
    expect(cleaned).toBe("```\ncode\n```");
    expect(paths).toEqual(["out.pdf"]);
  });

  it("merges multiple blocks in order", () => {
    const { cleaned, paths } = parseAttachments(
      "<yomi_attachments>a.pdf</yomi_attachments>\n<yomi_attachments>b.pdf</yomi_attachments>\n",
    );
    expect(cleaned).toBe("");
    expect(paths).toEqual(["a.pdf", "b.pdf"]);
  });

  it("surfaces a fenced example as typed", () => {
    const text =
      "use this syntax:\n```\n<yomi_attachments>\nout.pdf\n</yomi_attachments>\n```";
    const { cleaned, paths } = parseAttachments(text);
    expect(cleaned).toBe(text);
    expect(paths).toEqual([]);
  });

  it("keeps the fenced example but collects the real declaration", () => {
    const text =
      "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```\n<yomi_attachments>\nreal.pdf\n</yomi_attachments>";
    const { cleaned, paths } = parseAttachments(text);
    expect(cleaned).toBe(
      "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```",
    );
    expect(paths).toEqual(["real.pdf"]);
  });

  it("leaves an unterminated block untouched", () => {
    const text = "done\n<yomi_attachments>\na.pdf";
    const { cleaned, paths } = parseAttachments(text);
    expect(cleaned).toBe(text);
    expect(paths).toEqual([]);
  });

  it("strips an empty block without paths", () => {
    const { cleaned, paths } = parseAttachments(
      "done\n<yomi_attachments>\n</yomi_attachments>",
    );
    expect(cleaned).toBe("done");
    expect(paths).toEqual([]);
  });
});
