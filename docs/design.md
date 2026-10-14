# musicbox design

Visual concept for the app. It follows rekordbox's browser layout and tone, so DJs moving between the two find things in the same places. It uses no rekordbox assets, skins, icons or fonts. Code: `apps/musicbox/src/theme.rs` (tokens and egui style), `icons.rs`, `deck.rs`, `tree.rs`, `browser.rs`, `cue_panel.rs`, `waveform.rs`.

Review renders: `MB_UI_SHOTS=<dir> cargo test -p musicbox --test ui_design`. This saves eight frames: empty deck, collection, playlist, editing, context menu, dialog, device, and the 960 × 600 minimum window.

## Principles
1. **The music is the brightest thing on screen.** Waveforms, artwork, key colours and hot-cue pads carry colour. Chrome stays grey.
2. **One accent.** Blue (`ACCENT`) marks the selection, the editing state, focus and primary buttons, and nothing else.
3. **A 4 px grid.**
   - Rows are 24 px.
   - Bars are 32 px (top bar, which is also the window's title bar), 44 px (browser toolbar), 28 px (table header) and 26 px (status).
   - Panel gutters are 14 px.
4. **Dark seams between panels**, not light borders. Surfaces step up in lightness from the wells (waveforms, fields) to the panels, the deck and the raised controls.
5. **Numbers align right.** BPM, time, bitrate and track number sit on the right edge of their column, like a player's list.

## Layout
```
┌ top bar 32 = title bar ─ ● ● ●  mark ──────────────────── [lock] Read-only ┐
├ deck ─────────────────────────────────────────────────────────────────────┤
│  zoomed waveform 64 (beat grid, cues, centre playhead)                     │
│  [art 76] title / artist · album                      BPM   KEY   TIME     │
│           (▶) overview waveform 32 ─────────────────────────── RGB|3-Band  │
│  CUE   Memory Cue │ A B C D E F G H │ cue list ▾  colour  comment  …       │
│  GRID  ◀ Grid  Grid ▶  Set Downbeat │ BPM  Set BPM  BPM ×2  BPM ÷2  info   │
├ tree 240 ──────────┬ toolbar 44: list name · count ─────────── (search) ───┤
│ ◎ Collection   169 │ header 28: # · Title · Artist · … (sorted = white ▲)  │
│ ▾ ≡ Playlists      │ rows 24, striped; colour bar · # · art · title …      │
│     ≡ list      21 │                                                       │
│ DEVICES            │                                                       │
│ ▾ ▭ STICK          │                                                       │
├────────────────────┴───────────────────────────────────────────────────────┤
│ status 26: ● last message                         job progress ▬▬▬  Cancel │
└────────────────────────────────────────────────────────────────────────────┘
```
- **Title bar.** The window's title bar is transparent and the content runs under it (`with_fullsize_content_view`), so the top bar is the title bar.
  - The bar is 32 pt, the height macOS 27 gives the title bar. Its content is centred 16 pt from the top, on the traffic lights' line.
  - The first 80 pt are left to the traffic lights (they end at 68 pt), except in full screen.
  - Dragging the bar's empty part moves the window, and a double click zooms it.
- **Menu bar** (`menu_bar.rs`, the `muda` crate). File, Edit and Window live in the macOS menu bar, not in the window:
  - musicbox: About, Services, Hide, Hide Others, Show All, Quit (⌘Q, sent as a normal close request so the library connection shuts down).
  - File: Open rekordbox xml… (⌘O), Close rekordbox xml, Export Collection in xml format… (⌘E), Close Window.
  - Edit: Undo, Redo (no key equivalents: ⌘Z stays with egui, so text fields undo their own typing).
  - Window: Minimize, Zoom, Enter Full Screen, Bring All to Front.
  - Items that do not apply are greyed out. The headless tests send the same commands through `App::menu_command`.
- **App icon** (`app_icon.rs`). The musicbox mark: a blue rounded tile with three dark level bars, on the macOS icon grid (an 824/1024 body with a soft shadow). It is drawn in code, used as the window and Dock icon, and packed into `musicbox.app` by `tools/bundle/bundle.sh` (example `app_icon`, then `iconutil`).
- **Deck height** depends on what it shows, so the browser never jumps while one kind of track stays selected:
  - 170 px: no track, or a device or xml track (waveforms and info).
  - 238 px: a collection track (adds the two control rows).
- **Empty deck.** With no track it keeps its shape: empty wells, a placeholder tile, and dashes in the readouts.
- **Zoomed waveform.** It draws the 3-band scroll wave (PWV7, `.2EX`) in the RGB style (low, mid and high as red, green and blue, height the loudest band) or in the 3-band style.
  - PWV5 and PWV3 keep only transient peaks: a kick is 3–4 columns, then zeros. Drawn as they are, they leave gaps between beats, so PWV5 is used only when a track has no PWV7.
  - The grid line under the controls shows the tempo at the cursor, since dynamic grids change tempo.
- **Position.**
  - The zoomed strip and the pads move the cursor without playing.
  - The overview seeks and plays (as before).
  - The part already played is dimmed on the overview.

## Tokens
| Token | Value | Use |
|---|---|---|
| `SEAM` | `#0A0A0A` | lines between panels, menu separators |
| `BG_WELL` | `#0F0F0F` | waveform wells, text fields, search |
| `BG_PANEL` | `#171717` | tree, table |
| `BG_STRIPE` | `#1B1B1B` | odd table rows |
| `BG_DECK` | `#1E1E1E` | top bar, deck, toolbar, status bar |
| `BG_BAR` | `#212121` | table header |
| `BG_RAISED` / `BG_HOVER` / `BG_PRESSED` | `#2A2A2A` / `#343434` / `#3D3D3D` | buttons and pads at rest / hovered / pressed |
| `LINE` | `#2E2E2E` | separators inside a surface |
| `TEXT_STRONG` / `TEXT` / `TEXT_DIM` / `TEXT_MUTED` | `#F2F2F2` / `#D6D6D6` / `#9C9C9C` / `#6C6C6C` | titles and selection / body / secondary columns / captions and counts |
| `ACCENT` / `ACCENT_HOVER` | `#1473EB` / `#2A85F7` | selection edge, editing, focus, primary buttons |
| `SELECTION` | `#163C66` | selected rows (table and tree) |
| `ROW_HOVER` | `#222629` | hovered rows |
| `WARN` / `DANGER` | `#FFB937` / `#F0524F` | device warnings / errors |

Windows and menus use `#242424` with a `#383838` edge, 10 px (window) or 8 px (menu) radius, and a soft drop shadow. Controls use a 4 px radius. The pill-shaped controls (search, editing switch, waveform style) use the full height as their radius.

## Type
- **Font.** The macOS system font (SF Pro, `/System/Library/Fonts/SFNS.ttf`) is read at run time, never bundled.
  - Weights: regular 410, medium 510 and semibold 620.
  - Optical size: 17, the text cut.
  - Hiragino Sans is the CJK fallback, and egui's own fonts remain behind both for symbols.
  - SF Mono is used for monospace.
  - Without these files (other systems), egui's defaults are used.
- **Sizes:**

  | Size | Use |
  |---|---|
  | 13 | body |
  | 11.5 (medium) | table header |
  | 11 | small |
  | 10, upper case, +0.8 tracking (semibold) | captions such as `DEVICES`, `BPM`, `CUE` |
  | 15 (semibold) | list title |
  | 16 (semibold) | deck title |
  | 19 (semibold) | readouts |

## Components
- **Tree rows.** Full-width rows with:
  - a chevron (for rows that open and close);
  - a 14 px icon;
  - the name, truncated;
  - a right-aligned track count.

  A selected row gets the selection fill and a 2 px accent edge. The device row shows a lock icon on the right when the volume is protected.
- **Track table.**
  - The first column is a colour bar for the track colour slot, followed by the number and an 18 px artwork thumbnail.
  - Key text is coloured by its place on the circle of fifths (`theme::key_color`; minor and major of one place share a hue).
  - The title column takes the width the others leave.
  - Column edges show only on hover (the resize handle).
  - Switching lists starts at the top. Changing search or sort keeps the focused track in view.
- **Hot-cue pads** (28 × 26).
  - A set pad is filled with the cue's colour and shows a dark letter.
  - An empty pad is a raised tile with a muted letter.
  - The selected cue's pad gets a white ring.
- **Editing switch.**
  - Read-only: a grey pill with a closed lock.
  - Editing: a selection-blue pill with an accent edge and an open lock.
- **Dialogs.** OK (accent, filled) and Cancel sit at the bottom right. Text fields have 8 px side padding.
- **Icons** (`icons.rs`). Drawn with the painter on a 16 px grid with 1.4 px strokes:
  - library and lists: record (collection), list, folder, funnel (intelligent list), clock (history), tag;
  - sources: USB stick (device), document (xml);
  - controls: play, pause, search, lock and unlock, chevrons, close;
  - the app mark.

## Performance
`apps/musicbox/tests/perf_lists.rs` (run with `--release --ignored --nocapture`) times frames on 50 000 tracks with 400 playlists and a 20 000-track list:

| Action | Release |
|---|---|
| Idle frame | 0.5 ms |
| Scrolling | 1.3 ms |
| Typing a search, per key | 2.6 ms |
| Sorting | 4–9 ms |
| Opening the 20 000-track list | 2.4 ms |

Each lookup is built once per library snapshot and reused (`browser::Lookups`): the content-ID index, the lower-case search text, the sort keys of each text column, and whether each artwork file exists. A debug build is 10–40× slower (scrolling 35–73 ms per frame), so `start.sh` runs a release build. The check for a running rekordbox reads the process list on a worker thread.

## Accessibility labels the UI tests rely on
The custom widgets report the same names the egui widgets had, so `egui_kittest` finds them by label. Do not rename them without updating `apps/musicbox/tests/`:
- the editing switch: `Read-only`, `Editing`, `Editing (starting…)`;
- tree rows: the list or device name; `Playlists`, `Histories`, `My Tag`, `Collection`, `All Tracks`;
- the transport button: `Play` / `Pause`;
- the hot-cue pads: `A`–`H`;
- the clear-search button: `Clear search`.
