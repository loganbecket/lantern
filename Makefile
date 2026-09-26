# Build and install Lantern for the current user (no root needed).
#
#   make install          -> ~/.local
#   make PREFIX=/usr install  (as root, for all users)

PREFIX ?= $(HOME)/.local
APP_ID  = io.github.loganbecket.Lantern
BIN     = $(PREFIX)/bin
APPS    = $(PREFIX)/share/applications
ICONS   = $(PREFIX)/share/icons/hicolor/scalable/apps
META    = $(PREFIX)/share/metainfo

.PHONY: build install uninstall deb clean

build:
	cargo build --release

# Needs `cargo install cargo-deb`. Output lands in target/debian/.
deb:
	cargo deb

install: build
	install -Dm755 target/release/lantern $(BIN)/lantern
	install -Dm644 data/$(APP_ID).desktop $(APPS)/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).svg $(ICONS)/$(APP_ID).svg
	install -Dm644 data/$(APP_ID).metainfo.xml $(META)/$(APP_ID).metainfo.xml
	-update-desktop-database $(APPS) 2>/dev/null
	-gtk-update-icon-cache -q -t $(PREFIX)/share/icons/hicolor 2>/dev/null

uninstall:
	rm -f $(BIN)/lantern $(APPS)/$(APP_ID).desktop $(ICONS)/$(APP_ID).svg $(META)/$(APP_ID).metainfo.xml
	-update-desktop-database $(APPS) 2>/dev/null

clean:
	cargo clean
