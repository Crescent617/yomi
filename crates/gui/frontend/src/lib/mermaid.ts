import { mermaidTheme } from "./mermaidTheme";

const MAX_SOURCE_LENGTH = 50_000;
let renderId = 0;
let renderQueue: Promise<unknown> = Promise.resolve();

export interface MermaidRenderResult {
  svg: string;
  bindFunctions?: (element: Element) => void;
}

/**
 * Render one diagram at a time because Mermaid configuration is global.
 * The module is loaded only when a completed Markdown message contains a
 * mermaid fence, keeping it out of the normal chat bundle.
 */
export function renderMermaid(source: string): Promise<MermaidRenderResult> {
  const task = renderQueue.then(() => renderMermaidInternal(source));
  renderQueue = task.catch(() => undefined);
  return task;
}

async function renderMermaidInternal(
  source: string,
): Promise<MermaidRenderResult> {
  if (!source.trim()) throw new Error("The diagram is empty.");
  if (source.length > MAX_SOURCE_LENGTH) {
    throw new Error("The diagram is too large to render.");
  }

  const { default: mermaid } = await import("mermaid");
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    suppressErrorRendering: true,
    theme: "base",
    htmlLabels: false,
    maxTextSize: MAX_SOURCE_LENGTH,
    themeVariables: mermaidTheme(),
  });

  const valid = await mermaid.parse(source, { suppressErrors: true });
  if (!valid) throw new Error("The Mermaid syntax is invalid.");

  renderId += 1;
  const result = await mermaid.render(`yomi-mermaid-${renderId}`, source);
  return {
    svg: sanitizeSvg(result.svg),
    bindFunctions: result.bindFunctions,
  };
}

/** Defense in depth on top of Mermaid's strict security mode. */
function sanitizeSvg(svg: string): string {
  const document = new DOMParser().parseFromString(svg, "image/svg+xml");
  document.querySelectorAll("script, foreignObject").forEach((node) => node.remove());
  for (const element of document.querySelectorAll("*")) {
    for (const attribute of [...element.attributes]) {
      if (
        attribute.name.toLowerCase().startsWith("on") ||
        /^(?:javascript|data):/i.test(attribute.value.trim())
      ) {
        element.removeAttribute(attribute.name);
      }
    }
  }
  return new XMLSerializer().serializeToString(document.documentElement);
}
