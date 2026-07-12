export type UserTextSegment =
  | { type: "text"; content: string }
  | { type: "system_reminder"; content: string };

const OPEN_TAG = "<system_reminder>";
const CLOSE_TAG = "</system_reminder>";

export function userTextForHeight(text: string): string {
  return parseUserText(text)
    .filter((segment) => segment.type === "text")
    .map((segment) => segment.content)
    .join("");
}

export function parseUserText(text: string): UserTextSegment[] {
  const segments: UserTextSegment[] = [];
  let offset = 0;

  while (offset < text.length) {
    const start = text.indexOf(OPEN_TAG, offset);
    if (start === -1) {
      segments.push({ type: "text", content: text.slice(offset) });
      break;
    }

    const contentStart = start + OPEN_TAG.length;
    const end = text.indexOf(CLOSE_TAG, contentStart);
    if (end === -1) {
      segments.push({ type: "text", content: text.slice(offset) });
      break;
    }

    if (start > offset) {
      const before = text.slice(offset, start);
      segments.push({
        type: "text",
        content: before.replace(/\s+$/, " "),
      });
    }

    const reminder = text.slice(contentStart, end).trim();
    if (reminder) {
      segments.push({ type: "system_reminder", content: reminder });
    }

    offset = end + CLOSE_TAG.length;
    const nextStart = text.indexOf(OPEN_TAG, offset);
    const boundary = nextStart === -1 ? text.length : nextStart;
    const after = text.slice(offset, boundary);
    if (after) {
      segments.push({
        type: "text",
        content: after.replace(/^\s+/, " "),
      });
      offset = boundary;
    }
  }

  return segments;
}
