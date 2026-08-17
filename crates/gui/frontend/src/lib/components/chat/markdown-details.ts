// `<details>/<summary>` 折叠块提取：markdown 预切分。
//
// streaming-markdown 不支持原始 HTML（< 一律转义为文本），所以折叠块在
// 进入解析器之前被切出来：块的位置换成一行占位标记（%%YOMI-DETAILS-N%%），
// 渲染后由 TextBlock 在标记处挂载 DetailsBlock 组件。
//
// 只识别行首锚定的 <details>（可带缩进）；未闭合的块原样保留为文本
// （流式中途的自然状态），闭合后下一次渲染即成块。

export interface DetailsSegment {
  kind: "details";
  summary: string;
  body: string;
}

export interface MarkdownSegment {
  kind: "md";
  text: string;
}

export type Segment = MarkdownSegment | DetailsSegment;

export interface SplitDetails {
  /** 折叠块替换为占位标记后的文本 */
  text: string;
  /** 按占位序号索引的折叠块 */
  blocks: { summary: string; body: string }[];
}

const OPEN_RE = /^[ \t]*<details>[ \t]*$/;
const SUMMARY_RE = /^[ \t]*<summary>([\s\S]*?)<\/summary>[ \t]*$/;
const CLOSE_RE = /^[ \t]*<\/details>[ \t]*$/;

export const DETAILS_MARKER_PREFIX = "%%YOMI-DETAILS-";

export function splitDetailsBlocks(content: string): SplitDetails {
  const lines = content.split("\n");
  const blocks: { summary: string; body: string }[] = [];
  const out: string[] = [];

  // 代码围栏内的 <details> 是示例而非声明——与 mention 的 fence 规则一致
  let fenced = false;
  let i = 0;
  while (i < lines.length) {
    if (lines[i].trimStart().startsWith("```")) {
      fenced = !fenced;
      out.push(lines[i]);
      i++;
      continue;
    }
    if (fenced || !OPEN_RE.test(lines[i])) {
      out.push(lines[i]);
      i++;
      continue;
    }
    // 找配对闭合行；找不到就放弃整块（按原文输出）
    let close = -1;
    for (let j = i + 1; j < lines.length; j++) {
      if (CLOSE_RE.test(lines[j])) {
        close = j;
        break;
      }
    }
    if (close === -1) {
      out.push(lines[i]);
      i++;
      continue;
    }
    // 首行 summary 可选；没有 summary 时正文从 open+1 开始
    let summary = "";
    let bodyStart = i + 1;
    const m = SUMMARY_RE.exec(lines[bodyStart] ?? "");
    if (m) {
      summary = m[1].trim();
      bodyStart++;
    }
    const body = lines.slice(bodyStart, close).join("\n").trim();
    blocks.push({ summary, body });
    // 占位行必须独立成段，smd 才会渲染成独立 <p>
    out.push("", `${DETAILS_MARKER_PREFIX}${blocks.length - 1}%%`, "");
    // 被切走的块内若含围栏标记，围栏状态按奇偶延续
    const fenceLines = lines
      .slice(i, close + 1)
      .filter((l) => l.trimStart().startsWith("```")).length;
    if (fenceLines % 2 === 1) fenced = !fenced;
    i = close + 1;
  }

  return { text: out.join("\n"), blocks };
}
