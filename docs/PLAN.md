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
- **Work pool.** Decodes run on a thread pool sized to the CPU, always taking
  the photo nearest the current scroll position next. Requests for cells that
  scroll off screen are canceled before they start.
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

Set `LANTERN_DEBUG=1` to log every decode with its time in milliseconds. It
also makes each launch a separate instance, so a test run does not open a
window in an already running Lantern.

- 2026-09-26, iPhone HEIC, 40 real files on local disk, 256 px bucket
  (embedded thumbnail): 23 ms median. 640 px bucket (full HEVC decode, 7
  threads busy): 1.3 s median, 73 ms min. Full decode is the slow path;
  a follow-up could show the embedded thumbnail first and sharpen later.
- 2026-09-26, same code on a CIFS network share (~12 MB/s): reads dominate,
  ~4 photos/s regardless of format. Nothing to fix in the decoder; the
  queue now favors what is on screen so the first rows fill first.
- 2026-09-26, the share is Wi-Fi at ~8-10 MB/s total, whatever the
  parallelism; that caps sharp thumbnails at ~3-4 photos/s there. JPEG
  previews from the EXIF thumbnail (first 64 KB, one read, fadvise) cost
  ~35 ms sequential and ~120 ms with 7 in flight, so a screenful is
  recognizable in well under a second and the ~160 buffered cells in a few
  seconds; sharp versions follow. iPhone HEIC has no cheap preview: its
  thumbnail sits at the end of the file and libheif 1.17 reads the whole
  file through the reader API, so HEIC goes straight to the full path.
  Reads of file prefixes must be one large read: `read_to_end`-style
  growth costs ~100 ms per file over SMB.
- 2026-09-26, date taken. Reading the first 64 KB of a file finds the EXIF
  date for 39 of 40 real iPhone HEICs; 1000 local files take ~40 ms. On the
  share every file open costs ~100 ms and opens serialize, so the same pass
  is ~90 s and competes with thumbnails. The pass now times its first reads
  and backs off on slow folders, leaving modified-time order. On this share
  modified time is within a day of the EXIF date for about 70% of files.

- 2026-09-26, 12 MP JPEG at the 256 px bucket: ~16 ms median, 53 ms max,
  release build, generated test images.
- 2026-09-26, 1000-photo folder, 1200x800 window, 224 px cells: 161 cells
  built and decoded on open (GTK keeps about `columns * 30` cells alive), the
  rest only when scrolled to.
