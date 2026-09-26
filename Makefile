# Build and install Lantern for the current user (no root needed).
#
#   make install          -> ~/.local
#   make PREFIX=/usr install  (as root, for all users)

PREFIX ?= $(HOME)/.local
APP_ID  = io.github.loganbecket.Lantern
BIN     = $(PREFIX)/bin
APPS    = $(PREFIX)/share/applications
ICONS   = $(PREFIX)/share/icons/hicolor
META    = $(PREFIX)/share/metainfo
SIZES   = 16 32 48 64 128 256 512

.PHONY: build install uninstall deb clean

build:
	cargo build --release

# Needs `cargo install cargo-deb`. Output lands in target/debian/.
deb:
	cargo deb

install: build
	install -Dm755 target/release/lantern $(BIN)/lantern
	install -Dm644 data/$(APP_ID).desktop $(APPS)/$(APP_ID).desktop
	for s in $(SIZES); do \
	  install -Dm644 data/icons/hicolor/$${s}x$${s}/apps/$(APP_ID).png $(ICONS)/$${s}x$${s}/apps/$(APP_ID).png; \
	done
	install -Dm644 data/icons/hicolor/scalable/apps/$(APP_ID).svg $(ICONS)/scalable/apps/$(APP_ID).svg
	install -Dm644 data/$(APP_ID).metainfo.xml $(META)/$(APP_ID).metainfo.xml
	-update-desktop-database $(APPS) 2>/dev/null
	-gtk-update-icon-cache -q -t $(ICONS) 2>/dev/null

uninstall:
	rm -f $(BIN)/lantern $(APPS)/$(APP_ID).desktop $(META)/$(APP_ID).metainfo.xml
	for s in $(SIZES); do rm -f $(ICONS)/$${s}x$${s}/apps/$(APP_ID).png; done
	rm -f $(ICONS)/scalable/apps/$(APP_ID).svg
	-update-desktop-database $(APPS) 2>/dev/null
	-gtk-update-icon-cache -q -t $(ICONS) 2>/dev/null

clean:
	cargo clean
