# Desktop Pet packs

Yomi supports local packages that follow the Codex Pets V1 and V2 atlas contracts. Put each pet in its own directory:

```text
~/.yomi/pets/<pet-id>/
├── pet.json
└── spritesheet.webp
```

Select and enable the pet from **Settings → Application → Desktop pet**. Invalid packages are ignored. The pet window size can be scaled from 50% to 200% in the same section.

## `pet.json`

The manifest uses the Codex Pets field names without conversion:

```json
{
  "id": "tiny-dino",
  "displayName": "Tiny Dino",
  "description": "A tiny coding dinosaur.",
  "spritesheetPath": "spritesheet.webp",
  "kind": "creature"
}
```

- `id` must match the package directory name.
- `displayName`, `description`, and `spritesheetPath` are required.
- `kind` is optional.
- `spriteVersionNumber` may be omitted or set to `1` for V1; it must be set to `2` for V2.
- `spritesheetPath` must be a relative path contained within the package directory.
- Packages are data-only; Yomi does not load scripts from them.

## Codex Pets spritesheets

Both versions use an 8-column grid with `192 × 208` cells. Rows 0–8 follow the same standard animation order:

| Row | Animation       | Frames |
| --: | --------------- | -----: |
|   0 | `idle`          |      6 |
|   1 | `running-right` |      8 |
|   2 | `running-left`  |      8 |
|   3 | `waving`        |      4 |
|   4 | `jumping`       |      5 |
|   5 | `failed`        |      8 |
|   6 | `waiting`       |      6 |
|   7 | `running`       |      6 |
|   8 | `review`        |      6 |

- V1 uses an 8 × 9 (`1536 × 1872`) WebP atlas.
- V2 uses an 8 × 11 (`1536 × 2288`) WebP atlas. Rows 9–10 contain 16 clockwise look directions in 22.5° steps, starting at up (000°).

Yomi renders status-driven animations for both versions and uses the V2 look rows while the pet is idle. Status and interaction animations take priority over pointer look direction. Pointer look is disabled when the operating system requests reduced motion or when global cursor coordinates are unavailable (including Wayland environments where the window API reports no global cursor position). Message bubbles are intentionally deferred.

## Companion panel

The pet window doubles as a floating conversation entry point. The feature ships without a settings entry for now — `companion.enabled` defaults to false and the preference is not exposed in the UI. Once enabled (Settings → Application → Desktop pet → Conversation panel), the global hotkey (default `CmdOrControl+Shift+Y`, configurable in the same section) toggles the panel from anywhere. The hotkey is registered only while companion mode is enabled.

- Click the pet to open or close the panel; `Escape` closes it. While open, the window grows from the sprite anchor (panel above the sprite, flipping below when there is no room above, 8 px margins to screen edges) and the sprite keeps its exact screen position (rendered at `sprite_x`, smaller at 0.7× scale); closing restores the collapsed frame. Dragging is disabled while the panel is open.
- Panel height adapts to content: the frontend measures the natural height (header + messages + bars + input), reports it, and the window is reframed around the open-time sprite anchor (placement kept), clamped to [240, 560] and to the work area. The last height is remembered across opens.
- Session binding: a pinned session wins, then a one-shot override (set when opening from a pending permission/ask indicator, cleared on close), then the main window's active session, delivered via `PetSnapshot.active_session_id`. Pinning is runtime-only and does not persist across restarts.
- The panel reuses the main window's message fold pipeline in the pet window's own JS context (same modules, fresh store), so streaming, `PermissionBar`, and `AskUserBar` work unchanged. Sending uses `send_message` like the main chat input.
- Session event pumps are reference-counted by window label: the companion can hold a subscription on a session the main window is not viewing, and the pump stops only when the last subscriber leaves. `active_session` is written only by the `main` window.
- The pet window's kernel notification listener runs in passive mode: it updates local session phase/title but skips completion records, unread badges, and desktop notifications (those stay owned by the main window).
- "Open in main" moves the bound session to the main window (shows, unminimizes, focuses it, then emits the existing `app:open_session` event) and closes the panel.
