import { describe, expect, test } from "vitest";
import { deleteToLineStart } from "./delete-to-line-start";

describe("deleteToLineStart", () => {
  test("deletes from line start to caret on the first line", () => {
    expect(deleteToLineStart('foo = "bar"', 7, 7)).toEqual({
      start: 0,
      end: 7,
      cursor: 0,
    });
  });

  test("deletes to the previous newline on later lines", () => {
    const value = '[server]\nport = 8080\nhost = "x"';
    // caret after `8080` on the second line
    const caret = value.indexOf("8080") + 4;
    expect(deleteToLineStart(value, caret, caret)).toEqual({
      start: 9,
      end: caret,
      cursor: 9,
    });
  });

  test("is a no-op when the caret is already at line start", () => {
    const value = "abc\ndef";
    expect(deleteToLineStart(value, 4, 4)).toEqual({
      start: 4,
      end: 4,
      cursor: 4,
    });
  });

  test("is a no-op at offset 0", () => {
    expect(deleteToLineStart("abc", 0, 0)).toEqual({
      start: 0,
      end: 0,
      cursor: 0,
    });
  });

  test("removes the selection when one is active", () => {
    const value = "abcdef";
    expect(deleteToLineStart(value, 2, 5)).toEqual({
      start: 2,
      end: 5,
      cursor: 2,
    });
  });

  test("does not split surrogate pairs around emoji", () => {
    const value = 'greeting = "hi 😀 there"';
    // caret after the emoji (UTF-16 index past the surrogate pair)
    const caret = value.indexOf("😀") + 2;
    expect(deleteToLineStart(value, caret, caret)).toEqual({
      start: 0,
      end: caret,
      cursor: 0,
    });
  });
});
