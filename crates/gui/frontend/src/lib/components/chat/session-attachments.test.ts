import { describe, expect, test } from "vitest";
import { sessionAttachments } from "./session-attachments";
import type { BotMessage, Message } from "../../state.svelte";

let seq = 0;

function assistant(text: string): BotMessage {
  seq += 1;
  return {
    id: `a${seq}`,
    created_at: "2026-11-02T00:00:00Z",
    type: "assistant",
    content: [{ type: "text", text }],
  };
}

describe("sessionAttachments", () => {
  test("collects declared paths from assistant messages", () => {
    expect(
      sessionAttachments([
        assistant(
          "done\n<yomi_attachments>\nout.pdf\n data.csv \n</yomi_attachments>",
        ),
      ]),
    ).toEqual(["out.pdf", "data.csv"]);
  });

  test("dedupes by path, first-seen order, across messages", () => {
    expect(
      sessionAttachments([
        assistant("<yomi_attachments>\na.pdf\nb.pdf\n</yomi_attachments>"),
        assistant("<yomi_attachments>\nb.pdf\nc.pdf\n</yomi_attachments>"),
      ]),
    ).toEqual(["a.pdf", "b.pdf", "c.pdf"]);
  });

  test("merges multiple blocks in one message in document order", () => {
    expect(
      sessionAttachments([
        assistant(
          "<yomi_attachments>a.pdf</yomi_attachments>\n<yomi_attachments>b.pdf</yomi_attachments>",
        ),
      ]),
    ).toEqual(["a.pdf", "b.pdf"]);
  });

  test("ignores fenced examples and unterminated blocks", () => {
    expect(
      sessionAttachments([
        assistant(
          "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```\nreal\n<yomi_attachments>\nreal.pdf",
        ),
      ]),
    ).toEqual([]);
  });

  test("keeps a real block that follows a fenced example", () => {
    expect(
      sessionAttachments([
        assistant(
          "```\n<yomi_attachments>\nexample.pdf\n</yomi_attachments>\n```\nprose\n<yomi_attachments>\nreal.pdf\n</yomi_attachments>",
        ),
      ]),
    ).toEqual(["real.pdf"]);
  });

  test("ignores non-assistant messages", () => {
    const messages: Message[] = [
      {
        id: "u1",
        created_at: "2026-11-02T00:00:00Z",
        type: "user",
        content: [
          {
            type: "text",
            text: "<yomi_attachments>user.pdf</yomi_attachments>",
          },
        ],
      },
    ];
    expect(sessionAttachments(messages)).toEqual([]);
  });

  test("returns empty for assistant messages without declarations", () => {
    expect(sessionAttachments([assistant("just prose")])).toEqual([]);
  });
});
