# Three voices mark

`generate.mjs` makes every frame of the Three voices mark as a standalone SVG file.

## Run

From the repo root:

    node design/mark/generate.mjs

The script needs only Node. It has no dependencies.

The output is deterministic. The filter id is fixed, so two runs give byte-identical files.

## Output

Frames go to `crates/consort-desktop/assets/mark/`.

| Folder | Palette | Edges | Size |
| --- | --- | --- | --- |
| `dark/` | dark, blend `screen` | displaced by a filter | 44 px, viewBox `10 11 80 80` |
| `light/` | light, blend `multiply` | displaced by a filter | 44 px, viewBox `10 11 80 80` |
| `dark-sharp/` | dark | no filter | 20 px |
| `light-sharp/` | light | no filter | 20 px |

Sharp idle and done frames use viewBox `20 21 60 60`. Sharp working and needs frames use `10 11 80 80`.

Each folder holds 38 frames:

| State | Files | Notes |
| --- | --- | --- |
| idle | `idle-00` | Still. |
| working | `working-00` to `working-11` | Loops after 12 frames. |
| needs | `needs-00` to `needs-23` | Loops after 24 frames. Gold beats every 4 frames. |
| done | `done-00` | Still. |

In total there are 152 frames.

## Playback

Play moving states at 8 fps. Play idle and done as one still frame.

## Logo files

The script also writes `design/logo/mark-dark.svg`, `mark-light.svg`, `app-icon-dark.svg` and `app-icon-light.svg`. The app icons are 1024 px rounded tiles on the Spruce card colour.
