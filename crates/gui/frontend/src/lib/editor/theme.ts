import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

/**
 * CodeMirror chrome colors driven entirely by the app's semantic CSS
 * variables (see app.css `@theme`). `var()` references resolve at paint
 * time, so the editor follows light/dark and custom palettes live, with
 * no reconfiguration or `dark:` branching.
 */
export const appTheme = EditorView.theme({
  "&": {
    backgroundColor: "var(--color-background)",
    color: "var(--color-foreground)",
    fontSize: "0.875rem",
    height: "100%",
  },
  ".cm-content": {
    fontFamily: "var(--font-mono)",
    lineHeight: "1.5rem",
    caretColor: "var(--color-foreground)",
    padding: "1rem 0",
  },
  ".cm-line": {
    padding: "0 1rem",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "var(--color-foreground)",
  },

  /* Selection (drawSelection layer + native ::selection fallback). */
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, &.cm-focused ::selection":
    {
      backgroundColor: "hsl(var(--primary) / 0.2)",
    },
  ".cm-selectionMatch": {
    backgroundColor: "hsl(var(--primary) / 0.12)",
  },

  /* Active line — matches the old hand-rolled editor's bg-primary/10. */
  ".cm-activeLine": {
    backgroundColor: "hsl(var(--primary) / 0.1)",
  },

  /* Gutter — old editor: bg-muted/30, border-r border-border, mono muted. */
  ".cm-gutters": {
    backgroundColor: "hsl(var(--muted) / 0.3)",
    color: "var(--color-muted-foreground)",
    fontFamily: "var(--font-mono)",
    fontSize: "0.75rem",
    border: "none",
    borderRight: "1px solid var(--color-border)",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "hsl(var(--primary) / 0.1)",
    color: "var(--color-primary)",
  },
  ".cm-foldGutter": {
    color: "var(--color-muted-foreground)",
  },

  /* Search & replace panel and other panels. */
  ".cm-panels": {
    backgroundColor: "var(--color-card)",
    color: "var(--color-card-foreground)",
  },
  ".cm-panels.cm-panels-top": {
    borderBottom: "1px solid var(--color-border)",
  },
  ".cm-panels.cm-panels-bottom": {
    borderTop: "1px solid var(--color-border)",
  },
  ".cm-panel.cm-panel-search": {
    fontFamily: "var(--font-mono)",
    fontSize: "0.75rem",
  },
  ".cm-textfield": {
    backgroundColor: "var(--color-background)",
    color: "var(--color-foreground)",
    border: "1px solid var(--color-input)",
    borderRadius: "0.375rem",
  },
  ".cm-button": {
    backgroundColor: "var(--color-secondary)",
    backgroundImage: "none",
    color: "var(--color-secondary-foreground)",
    border: "1px solid var(--color-border)",
    borderRadius: "0.375rem",
  },
  ".cm-searchMatch": {
    backgroundColor: "hsl(var(--warning) / 0.3)",
  },
  ".cm-searchMatch-selected": {
    backgroundColor: "hsl(var(--warning) / 0.5)",
  },

  /* Bracket matching. */
  ".cm-matchingBracket": {
    backgroundColor: "hsl(var(--success) / 0.25)",
    outline: "none",
  },
  ".cm-nonmatchingBracket": {
    backgroundColor: "hsl(var(--error) / 0.25)",
  },

  /* Autocomplete / tooltips. */
  ".cm-tooltip": {
    backgroundColor: "var(--color-popover)",
    color: "var(--color-popover-foreground)",
    border: "1px solid var(--color-border)",
    borderRadius: "0.375rem",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    backgroundColor: "var(--color-primary)",
    color: "var(--color-primary-foreground)",
  },
});

/**
 * Syntax colors mapped onto the app's semantic palette — same roles as the
 * old hand-rolled TOML highlighter (key→primary, string→success,
 * bool→warning, number/section→info, comment→muted).
 */
const appHighlightStyle = HighlightStyle.define([
  { tag: [t.comment, t.meta], color: "var(--color-muted-foreground)" },
  {
    tag: [
      t.keyword,
      t.modifier,
      t.controlKeyword,
      t.moduleKeyword,
      t.operatorKeyword,
      t.definitionKeyword,
    ],
    color: "var(--color-primary)",
  },
  {
    tag: [t.string, t.special(t.string), t.regexp, t.character],
    color: "var(--color-success)",
  },
  { tag: [t.number, t.integer, t.float], color: "var(--color-info)" },
  { tag: [t.bool, t.null], color: "var(--color-warning)" },
  { tag: t.atom, color: "var(--color-info)" },
  {
    tag: [t.propertyName, t.attributeName, t.labelName],
    color: "var(--color-primary)",
  },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName],
    color: "var(--color-info)",
  },
  {
    tag: [t.typeName, t.className, t.namespace],
    color: "var(--color-warning)",
  },
  { tag: t.tagName, color: "var(--color-destructive)" },
  {
    tag: [t.operator, t.punctuation, t.separator],
    color: "var(--color-muted-foreground)",
  },
  { tag: t.escape, color: "var(--color-destructive)" },
  {
    tag: [t.link, t.url],
    color: "var(--color-info)",
    textDecoration: "underline",
  },
  { tag: t.heading, color: "var(--color-primary)", fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: t.inserted, color: "var(--color-success)" },
  { tag: t.deleted, color: "var(--color-destructive)" },
  { tag: t.changed, color: "var(--color-warning)" },
  { tag: t.invalid, color: "var(--color-error)" },
]);

export const appSyntaxHighlighting = syntaxHighlighting(appHighlightStyle);
