import type { SessionState } from "../../state.svelte";

export interface SessionBreadcrumbItem {
  id: string;
  label: string;
  isSubagent: boolean;
}

export function buildSessionBreadcrumb(
  session: SessionState,
  resolveSession: (id: string) => SessionState | undefined,
): SessionBreadcrumbItem[] {
  const items: SessionBreadcrumbItem[] = [];
  let current: SessionState | undefined = session;
  const seen = new Set<string>();

  while (current && !seen.has(current.id)) {
    seen.add(current.id);
    items.push({
      id: current.id,
      label: current.alias || current.id.slice(-8),
      isSubagent: Boolean(current.parent_session_id),
    });

    if (!current.parent_session_id) break;
    const parentId = current.parent_session_id;
    current = resolveSession(parentId);
    if (!current) {
      items.push({ id: parentId, label: "…", isSubagent: false });
      break;
    }
  }

  return items.reverse();
}
