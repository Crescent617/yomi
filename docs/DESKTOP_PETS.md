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

Yomi renders status-driven animations for both versions and uses the V2 look rows while the pet is idle. Status and interaction animations take priority over pointer look direction. Pointer look is disabled when the operating system requests reduced motion or when global cursor coordinates are unavailable (including Wayland environments where the window API reports no global cursor position). Message bubbles and pet-to-session navigation are intentionally deferred.
