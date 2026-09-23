import { stripControlPua } from "../../utils";

/** Hard cap on a single quote's length — quoting a whole long message
 * should not explode the outgoing prompt. */
export const MAX_QUOTE_CHARS = 2000;

/**
 * Normalize raw selected text into a quote body: strip control/PUA
 * characters (GUI paste path has a PUA precedent), normalize line endings,
 * collapse blank-line runs, trim, and truncate by code point (never slice
 * surrogate pairs).
 */
export function formatQuoteText(
  raw: string,
  maxChars: number = MAX_QUOTE_CHARS,
): string {
  let text = stripControlPua(raw).replace(/\r\n?/g, "\n");
  text = text.replace(/\n{3,}/g, "\n\n").trim();
  if (!text) return "";
  // UTF-16 length is an upper bound for the code-point count — only pay
  // for the spread when truncation is actually possible.
  if (text.length > maxChars) {
    const points = [...text];
    if (points.length > maxChars) {
      text = points.slice(0, maxChars).join("").replace(/\s+$/, "") + "…";
    }
  }
  return text;
}

/** Render pending quotes as a markdown blockquote prefix ("" when none). */
export function quotePrefix(quotes: string[]): string {
  return quotes
    .map((q) => q.trim())
    .filter((q) => q.length > 0)
    .map((q) => q.split("\n").join("\n> "))
    .map((q) => "> " + q)
    .join("\n\n");
}

/** Combine quote prefix with the user's text for the outgoing message. */
export function composeOutgoingText(
  quotes: string[],
  baseText: string,
): string {
  const prefix = quotePrefix(quotes);
  if (!prefix) return baseText;
  return baseText ? prefix + "\n\n" + baseText : prefix;
}

function messageIdOf(node: Node | null): string | null {
  // Text nodes have no attributes — start from the containing element.
  let el: Element | null =
    node && node.nodeType === 3 ? node.parentElement : (node as Element | null);
  while (el) {
    const id = el.getAttribute?.("data-message-id");
    if (id) return id;
    el = el.parentElement;
  }
  return null;
}

/**
 * The message a selection belongs to, or null when it does not resolve to
 * exactly one message container (outside the transcript, or spanning
 * multiple messages).
 */
export function quoteSourceMessageId(selection: Selection): string | null {
  const anchor = messageIdOf(selection.anchorNode);
  const focus = messageIdOf(selection.focusNode);
  if (!anchor || !focus || anchor !== focus) return null;
  return anchor;
}
