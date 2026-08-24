import { describe, it, expect } from "vitest";
import { fitTableColumnWidths, TABLE_MIN_COL_W } from "./share-table";

function sum(widths: number[]): number {
  return widths.reduce((a, b) => a + b, 0);
}

describe("fitTableColumnWidths", () => {
  it("keeps natural widths when everything fits", () => {
    expect(fitTableColumnWidths([100, 200, 150], 1000)).toEqual([
      100, 200, 150,
    ]);
  });

  it("returns a copy rather than mutating the input", () => {
    const natural = [100, 200];
    const widths = fitTableColumnWidths(natural, 100);
    expect(widths).not.toBe(natural);
    expect(natural).toEqual([100, 200]);
  });

  it("shrinks proportionally instead of punishing the widest column", () => {
    // Regression: a content-heavy last column must stay the widest, not get
    // squeezed to the floor while thin columns keep their full width.
    const widths = fitTableColumnWidths([150, 200, 180, 950], 552);
    expect(widths[3]).toBeGreaterThan(widths[0]);
    expect(widths[3]).toBeGreaterThan(widths[1]);
    expect(widths[3]).toBeGreaterThan(widths[2]);
    expect(widths[3]).toBeGreaterThan(300);
    expect(sum(widths)).toBeCloseTo(552, 5);
  });

  it("hands clamped columns' deficit back to the rest", () => {
    const widths = fitTableColumnWidths([60, 60, 1000], 300);
    expect(widths[0]).toBe(TABLE_MIN_COL_W);
    expect(widths[1]).toBe(TABLE_MIN_COL_W);
    expect(widths[2]).toBeCloseTo(300 - TABLE_MIN_COL_W * 2, 5);
    expect(sum(widths)).toBeCloseTo(300, 5);
  });

  it("keeps columns already at/below the floor at their natural width", () => {
    const widths = fitTableColumnWidths([30, 500], 100);
    expect(widths[0]).toBe(30);
    expect(widths[1]).toBeCloseTo(70, 5);
  });

  it("never shrinks a column below the floor, even when it must overflow", () => {
    const widths = fitTableColumnWidths([1000, 1000, 1000], 100);
    expect(widths).toEqual([TABLE_MIN_COL_W, TABLE_MIN_COL_W, TABLE_MIN_COL_W]);
  });

  it("floors every column when the budget is zero or negative", () => {
    expect(fitTableColumnWidths([300, 100], 0)).toEqual([
      TABLE_MIN_COL_W,
      TABLE_MIN_COL_W,
    ]);
  });

  it("handles an empty table", () => {
    expect(fitTableColumnWidths([], 100)).toEqual([]);
  });
});
