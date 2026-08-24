import { expect, test, type Page } from "@playwright/test";
import { writeFile, mkdir } from "node:fs/promises";

/**
 * E2E for the share card renderer: drives the real canvas pipeline
 * (markdown parse → layout → draw → PNG blob) in the browser via the /e2e
 * harness and verifies the rendered pixels.
 *
 * Regression probe: a content-heavy last table column must keep its
 * proportional share of the width. The old layout squeezed the widest
 * column to the minimum, leaving the right side of the table empty; the
 * probe measures ink coverage in the right-hand strip of the card.
 */

// Card geometry mirrors share-card.ts (unscaled px, SCALE = 2).
const SCALE = 2;
const CARD_WIDTH = 720;
const OUTER_PAD = 32;
const CARD_PAD = 48;
const CONTENT_X = OUTER_PAD + CARD_PAD; // 80
const TEXT_WIDTH = CARD_WIDTH - CARD_PAD * 2; // 624

const MARKDOWN = `## 青光眼筛查指标

| 检查项目（含英文缩写） | 判读口径与测量方式 | 参考阈值与判读标准 | 说明 |
|---|---|---|---|
| 眼底彩色照相 fundus | 垂直杯盘比 VCDR 判读 | ≥0.7 或双眼差 ≥0.2 | 青光眼视神经病变的核心结构指标，需结合盘沿切迹、神经纤维层缺损与环行血管裸露综合判断，单次异常需复查确认 |
| 视盘 OCT 扫描 | 盘沿面积与 RNFL 厚度分析 | 低于同龄正常值第 5 百分位 | 主要用于排除生理性大杯，单次测量异常需结合眼底照相复核，不能单独作为诊断依据 |
| 眼压测量 | Goldmann 压平眼压计测量 | >21 mmHg（需角膜厚度校正） | 单次升高不足以诊断，需结合角膜厚度与昼夜波动曲线，高眼压症与青光眼性损伤需区分随访策略 |
`;

interface CardStats {
  pngWidth: number;
  pngHeight: number;
  /** Ink pixel ratio in the right 40% of the text area. */
  rightInk: number;
  /** Ink pixel ratio in the left 40% of the text area. */
  leftInk: number;
  /** Base64 PNG for artifact inspection. */
  base64: string;
}

async function renderCard(page: Page): Promise<CardStats> {
  return page.evaluate(
    async ({ markdown, cardWidth, scale, contentX, textWidth }) => {
      const { renderShareCard } = window.__e2e.shareCard;
      const blob = await renderShareCard({
        content: markdown,
        date: new Date("2026-08-24T13:00:00"),
        width: cardWidth,
      });

      const bitmap = await createImageBitmap(blob);
      const canvas = document.createElement("canvas");
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("no 2d context");
      ctx.drawImage(bitmap, 0, 0);

      // Card background sampled just inside the card's top-left corner.
      const bg = ctx.getImageData(
        (32 + 4) * scale,
        (32 + 4) * scale,
        1,
        1,
      ).data;
      const isInk = (d: Uint8ClampedArray, i: number) =>
        Math.abs(d[i] - bg[0]) +
          Math.abs(d[i + 1] - bg[1]) +
          Math.abs(d[i + 2] - bg[2]) >
        60;

      // Ink ratio within a horizontal slice of the text area, scanned over
      // the full card height (background outside the body is uniform).
      const inkRatio = (x0rel: number, x1rel: number): number => {
        const x0 = Math.round((contentX + x0rel * textWidth) * scale);
        const x1 = Math.round((contentX + x1rel * textWidth) * scale);
        const y0 = Math.round((32 + 4) * scale);
        const y1 = canvas.height - y0;
        const d = ctx.getImageData(x0, y0, x1 - x0, y1 - y0).data;
        let ink = 0;
        const total = (x1 - x0) * (y1 - y0);
        for (let i = 0; i < d.length; i += 4) {
          if (isInk(d, i)) ink++;
        }
        return ink / total;
      };

      const buf = new Uint8Array(await blob.arrayBuffer());
      let binary = "";
      for (let i = 0; i < buf.length; i += 8192) {
        binary += String.fromCharCode(...buf.subarray(i, i + 8192));
      }
      return {
        pngWidth: bitmap.width,
        pngHeight: bitmap.height,
        rightInk: inkRatio(0.6, 1.0),
        leftInk: inkRatio(0.0, 0.4),
        base64: btoa(binary),
      };
    },
    {
      markdown: MARKDOWN,
      cardWidth: CARD_WIDTH,
      scale: SCALE,
      contentX: CONTENT_X,
      textWidth: TEXT_WIDTH,
    },
  );
}

test("share card renders a table with proportional column widths", async ({
  page,
}) => {
  await page.goto("/e2e");
  await page.waitForFunction(() => window.__e2e);
  const stats = await renderCard(page);

  await mkdir("test-results", { recursive: true });
  await writeFile(
    "test-results/share-card-table.png",
    Buffer.from(stats.base64, "base64"),
  );

  // Canvas size: (card width + outer pads) * scale.
  expect(stats.pngWidth).toBe((CARD_WIDTH + OUTER_PAD * 2) * SCALE);

  // Layout discriminator: squeezing the content-heavy last column to the
  // floor (old layout) wraps its text into ~15 lines per row and makes the
  // card ~2.3x taller (observed 3188px vs 1368px at this content); the
  // proportional layout stays compact.
  expect(stats.pngHeight).toBeLessThan(2000);

  // The content-heavy last column must fill the right side of the table:
  // with the old layout this strip held only two 1px separator lines and
  // the footer date (< 1% ink); proportional layout fills it with wrapped
  // cell text. Left columns carry real content too, so both sides have ink.
  expect(stats.rightInk).toBeGreaterThan(0.02);
  expect(stats.leftInk).toBeGreaterThan(0.02);
});
