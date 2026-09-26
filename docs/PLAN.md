# Lantern — Plan

A fast Linux photo browser with big, adjustable thumbnails. It is a file
manager that happens to be good at photos, not a photo organizer.

## Principles

- **Speed first.** Scrolling through thousands of photos must feel instant.
- **No index, no cache on disk.** Open a folder and see photos. Nothing is
  written anywhere except by the user's own actions.
- **Less is more.** Every feature below is the whole feature list.
- **Linux only.** GTK 4 + libadwaita, so it looks at home on GNOME and COSMIC.

## Features (v1)

1. Open a folder; browse its photos and subfolders in a grid.
2. Thumbnail size slider (and Ctrl+scroll / Ctrl +/-), from small to huge.
3. Skeleton placeholders while thumbnails decode; only visible cells load.
4. Sort by date taken (EXIF), falling back to file modified time; or by name.
5. Click to view full size; arrow keys to step through.
6. Actions on the selection:
   - **Delete** — move to the system trash (undoable), never a hard delete.
   - **Download** — copy to `~/Downloads`.
   - **Move / rename** — pick a destination folder and/or a new name.

Out of scope: tagging, albums, face detection, editing, metadata editing,
cloud anything, a library database.

## How it stays fast without a thumbnail cache

- **Virtualized grid** (`GtkGridView`): only the cells on screen exist, so a
  folder of 20,000 photos costs the same as one of 50.
- **Scaled decoding.** JPEGs are decoded directly at 1/2, 1/4, or 1/8 size via
  libjpeg-turbo, picking the smallest scale that still fills the cell. A 12 MP
  photo at 1/8 is ~500 px wide and decodes in a few milliseconds.
- **HEIC (iPhone).** Use the embedded preview image when it is big enough for
  the current cell size; otherwise decode the full image with libheif. This is
  the slow path and the main performance risk — measure early.
- **Work pool.** Decodes run on a thread pool sized to the CPU. Requests for
  cells that scroll off screen are canceled before they start.
- **In-memory LRU** of decoded textures for the session only, so scrolling back
  up is instant. Nothing is persisted.
- **EXIF orientation** applied at decode time.

## Architecture

```
src/
  main.rs        app entry point
  window.rs      main window, header bar, size slider, folder navigation
  grid.rs        GridView, list model of files, cell widgets, skeleton state
  decode.rs      scaled JPEG / HEIC / PNG decoding, orientation
  loader.rs      thread pool, request queue, cancellation, LRU
  viewer.rs      full-size view
  actions.rs     trash, copy to Downloads, move/rename
data/
  *.desktop, icon, metainfo
```

## Milestones

1. Window opens a folder and lists files in a grid with skeletons. (no decoding)
2. JPEG scaled decoding on the thread pool; size slider.
3. HEIC support; benchmark against a real iPhone folder.
4. Date-taken sorting.
5. Full-size viewer.
6. Delete / Download / Move-rename.
7. Polish: keyboard shortcuts, empty states, desktop file, icon, README
   screenshots, Flatpak.

## Build dependencies (Ubuntu / Pop!_OS)

```
sudo apt install libgtk-4-dev libadwaita-1-dev libheif-dev cmake nasm
```

libjpeg-turbo is built from source by the `turbojpeg` crate (hence cmake and
nasm) because the version Ubuntu 24.04 ships is too old for its API.

## Measurements

Set `LANTERN_DEBUG=1` to log every decode with its time in milliseconds.

- 2026-09-26, 12 MP JPEG at the 256 px bucket: ~16 ms median, 53 ms max,
  release build, generated test images.
- 2026-09-26, 1000-photo folder, 1200x800 window, 224 px cells: 161 cells
  built and decoded on open (GTK keeps about `columns * 30` cells alive), the
  rest only when scrolled to.
