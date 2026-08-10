/** Pure helpers for the Agents template panel (kept component-free for tests). */

/** Must stay in sync with `kernel::agent_tmpl::validate_name`. */
export const NAME_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;

/** Placeholder body for a new template (matches builtin ROLE.md style). */
export const NEW_TEMPLATE_STUB =
  "You are a specialist. Describe the role's responsibilities, principles, and output expectations here.\n";

export interface NameCheck {
  error: string;
  override: string;
}

/** Validate a new template name against the kebab-case rule and existing templates. */
export function checkTemplateName(
  name: string,
  existing: { name: string; source: string }[],
  scope: string,
): NameCheck {
  if (!NAME_RE.test(name)) {
    return { error: "Use kebab-case: ^[a-z0-9][a-z0-9-]{0,63}$", override: "" };
  }
  const hit = existing.find((t) => t.name === name);
  if (hit && hit.source === scope) {
    return { error: `"${name}" already exists in ${scope}`, override: "" };
  }
  return {
    error: "",
    override: hit ? `Will override the ${hit.source} template` : "",
  };
}

/** Whether the create form holds user input worth guarding against loss. */
export function createDraftDirty(name: string, draft: string): boolean {
  return name !== "" || draft !== NEW_TEMPLATE_STUB;
}
