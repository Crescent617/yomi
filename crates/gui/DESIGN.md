# GUI Design Guide

## Essence

Yomi is a focused agent workspace: calm, dense, and direct. The interface should make the agent's current state obvious without competing with the work itself.

## Principles

- **Workspace over dashboard.** Prefer full-size panes, dividers, and toolbars. Avoid nested cards, decorative containers, and excessive padding.
- **Show state where it belongs.** Stream progress stays inline with messages; input configuration stays near ChatInput; app-wide health belongs in StatusBar.
- **One clear hierarchy.** Use typography, spacing, and subtle surfaces before adding borders or color. Keep primary content dominant and metadata quiet.
- **Semantic color only.** Use theme tokens such as `primary`, `secondary`, `warning`, `error`, `success`, `info`, `subtle`, and `code-bg`. Never hardcode Tailwind palette colors or use `dark:` variants.
- **Color communicates meaning.** Most UI is neutral. Reserve color for selection, status, risk, feedback, and the current primary action.
- **Prefer soft actions.** Header actions generally use a light semantic background plus a subtle border. Use solid buttons for decisive dialog submission only.
- **Progressive disclosure.** Show summaries first; expand raw arguments, advanced settings, previews, and diagnostics on demand.
- **Compact, not cramped.** Favor 28–36 px controls, restrained radii, short labels, and consistent gaps. Preserve comfortable reading space for messages and code.
- **Use familiar interaction patterns.** Master–detail panes, segmented controls, inline status, toolbars, radio rows, and checkbox rows are preferred over novel menus.
- **Feedback stays local.** Show loading, success, validation, and failure at the control or content that caused it. Use global notifications only when no local surface exists.
- **Motion is functional.** Keep transitions short and subtle; use them to explain appearance, state changes, or progress. Respect `prefers-reduced-motion`.
- **Accessible by default.** Every icon-only control needs a label and tooltip. Support keyboard focus, disabled/loading states, touch devices, and sufficient semantic contrast.

## Visual Language

- Typography: Space Grotesk for UI prose and labels; IBM Plex Mono for code, paths, commands, IDs, tool names, and data readouts.
- Surfaces: `background` for work areas, subtle semantic tints for selected or pending states, `code-bg` for machine-readable content.
- Borders: use pane dividers and light outlines; avoid double borders and card-inside-card layouts.
- Radius: modest and consistent. Panels and dialogs may be rounded; editors and full-size workspaces should usually meet their pane edges.
- Icons: use Lucide consistently. Do not mix emoji with interface icons.

## Signature Elements

Yomi's identity lives in a small set of repeatable elements. Use them
consistently; do not invent per-component variations.

- **Kakishibu theme.** The default `yomi-ink` palette: warm washi-paper light / sumi-ink dark neutrals with a persimmon (kakishibu) `primary`; its sibling `yomi-ai` ("Aizome") swaps the accent for true indigo. Other builtin themes remain selectable; the static `:root`/`.dark` boot palettes in `app.css` mirror yomi-ink and must stay in sync with `palettes.ts`.
- **Micro-labels.** Metadata, section headers, and readouts use the `micro-label` utility (10 px uppercase mono, wide tracking). Examples: StatusBar segments, popover section titles.
- **Stream shimmer.** The streaming status line pairs a mono gradient-shimmer verb (Thinking/Writing/tool verb) with a mono target, quiet mono telemetry (elapsed, estimated tokens), and a single 1 px theme-color scan line sweeping its underside as the only other motion. Keep both slow and subtle. Honors `prefers-reduced-motion`.
- **Pet mood chip.** The StatusBar shows the aggregate agent mood as a mini pet sprite plus a mood micro-label (idle/working/curious/alert), derived in `status-activity.ts` from the same priority ladder as the Rust `PetMood`.
- **Paper grain.** A faint static grain overlays the workspace (never the pet window, never interactive). It stays near-invisible; if it is noticeable in screenshots, it is too strong.

## Desktop Pet

The desktop Pet uses Codex Pets spritesheets; the old procedural status lamps no longer apply. Keep the Pet visually unobtrusive, use animation to communicate aggregate agent state, and do not make animation the only source of critical information. Package details are documented in [`docs/DESKTOP_PETS.md`](../../docs/DESKTOP_PETS.md).

## Decision Test

Before adding UI, ask:

1. Is this information shown at the correct scope?
2. Can spacing or typography express the hierarchy without another card?
3. Is color carrying meaning rather than decoration?
4. Is the primary action clear without making every action prominent?
5. Can the user understand the current state and next step at a glance?
