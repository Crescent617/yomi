import { describe, expect, test } from "vitest";
import { stripEndTurnMarker } from "./end-turn-marker";

describe("stripEndTurnMarker", () => {
  test("strips a trailing marker", () => {
    expect(stripEndTurnMarker("记一笔 __YOMI_END_TURN__")).toBe("记一笔");
  });

  test("tolerates trailing whitespace", () => {
    expect(stripEndTurnMarker("done __YOMI_END_TURN__\n  \n")).toBe("done");
  });

  test("marker-only text becomes empty", () => {
    expect(stripEndTurnMarker("__YOMI_END_TURN__")).toBe("");
  });

  test("mid-text marker is left in place", () => {
    const text = "__YOMI_END_TURN__ 后面还有正文";
    expect(stripEndTurnMarker(text)).toBe(text);
  });

  test("no marker: text returned unchanged", () => {
    const text = "普通收尾  \n";
    expect(stripEndTurnMarker(text)).toBe(text);
  });

  test("partial marker is left in place", () => {
    const text = "收尾 __YOMI_END_TU";
    expect(stripEndTurnMarker(text)).toBe(text);
  });
});
