# Lantern

A fast Linux photo browser with big, adjustable thumbnails.

Open a folder and scroll through thousands of photos, including iPhone HEIC
files, with no indexing and nothing cached to disk. Delete (to trash), copy to
Downloads, or move and rename, and that's it.

## Using it

Open a folder (or run `lantern ~/Pictures`). Subfolders come first, then
photos sorted by date taken. Double-click a photo to see it full size and use
the arrow keys to step through the folder.

| Keys | What |
| --- | --- |
| Ctrl+O | Open a folder |
| Alt+Up | Parent folder |
| Ctrl+scroll, Ctrl+plus / Ctrl+minus, Ctrl+0 | Thumbnail size |
| Delete | Move selected photos to the trash |
| Ctrl+D | Copy selected photos to Downloads |
| F2 | Move or rename selected photos |
| Escape | Back to the grid |

## Install

On Ubuntu 24.04 or Pop!_OS 24.04, download the `.deb` from the
[latest release](https://github.com/loganbecket/lantern/releases/latest)
and open it, or:

```
sudo apt install ./lantern_*.deb
```

## Build from source

```
sudo apt install libgtk-4-dev libadwaita-1-dev libheif-dev cmake nasm
make install
```

This installs Lantern for the current user only, so it needs no root, and adds
it to your application menu. `make uninstall` removes it. `make deb` builds
the package (needs `cargo install cargo-deb`).

## License

MIT
