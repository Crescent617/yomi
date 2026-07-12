import { describe, expect, test } from "vitest";
import { parseUserText, userTextForHeight } from "./user-text";

describe("system reminder parser", () => {
  test("preserves text order around reminders", () => {
    expect(
      parseUserText(
        "before <system_reminder>keep this in mind</system_reminder> after",
      ),
    ).toEqual([
      { type: "text", content: "before " },
      { type: "system_reminder", content: "keep this in mind" },
      { type: "text", content: " after" },
    ]);
  });

  test("renders reminders inline across surrounding newlines", () => {
    expect(
      parseUserText(
        "before\n<system_reminder>remember</system_reminder>\nafter",
      ),
    ).toEqual([
      { type: "text", content: "before " },
      { type: "system_reminder", content: "remember" },
      { type: "text", content: " after" },
    ]);
  });

  test("excludes reminders from measured text", () => {
    expect(
      userTextForHeight(
        "short<system_reminder>very\nlong\nreminder</system_reminder>text",
      ),
    ).toBe("shorttext");
  });

  test("keeps an unmatched tag as text", () => {
    const text = "before <system_reminder>unfinished";
    expect(parseUserText(text)).toEqual([{ type: "text", content: text }]);
  });

  test("omits empty reminders", () => {
    expect(
      parseUserText("before<system_reminder> </system_reminder>after"),
    ).toEqual([
      { type: "text", content: "before" },
      { type: "text", content: "after" },
    ]);
  });
});
