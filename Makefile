CARGO ?= cargo
UV ?= uv
INSTALL ?= install
SED ?= sed
GTK_UPDATE_ICON_CACHE ?= gtk-update-icon-cache
UPDATE_DESKTOP_DATABASE ?= update-desktop-database
FC_CACHE ?= fc-cache

APP_ID := dev.craic.Craic
APP_NAME := Craic
BIN_NAME := craic
RUST_LOG ?= craic=debug

PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share
APPLICATIONSDIR ?= $(DATADIR)/applications
APPDATADIR ?= $(DATADIR)/$(BIN_NAME)
ICON_THEME_DIR ?= $(DATADIR)/icons/hicolor
ICONDIR ?= $(ICON_THEME_DIR)/scalable/apps
FONTDIR ?= $(DATADIR)/fonts/$(BIN_NAME)/JetBrainsMono

DESKTOP_TEMPLATE := data/$(APP_ID).desktop.in
APP_ICON := data/icons/hicolor/scalable/apps/$(APP_ID).svg
ASSET_FILES := $(wildcard src/assets/*.svg)
FONT_FILES := $(wildcard src/fonts/JetBrainsMono/*.ttf)
DOCS_DIR := docs
DOCS_SOURCE_DIR := $(DOCS_DIR)/source
DOCS_BUILD_DIR := $(DOCS_DIR)/build

.PHONY: dev run build release check test doc clean install uninstall

dev:
	RUST_LOG=$(RUST_LOG) $(CARGO) run

run: dev

build:
	$(CARGO) build

release:
	$(CARGO) build --release

check:
	$(CARGO) check

test:
	$(CARGO) test

doc:
	$(UV) run --project "$(DOCS_DIR)" sphinx-build -b html -j auto -W --keep-going "$(DOCS_SOURCE_DIR)" "$(DOCS_BUILD_DIR)"

clean:
	$(CARGO) clean

install: release
	$(INSTALL) -Dm755 target/release/$(BIN_NAME) "$(DESTDIR)$(BINDIR)/$(BIN_NAME)"
	$(INSTALL) -d "$(DESTDIR)$(APPLICATIONSDIR)"
	$(SED) -e "s|@APP_ID@|$(APP_ID)|g" \
		-e "s|@APP_NAME@|$(APP_NAME)|g" \
		-e "s|@BIN_PATH@|$(BINDIR)/$(BIN_NAME)|g" \
		"$(DESKTOP_TEMPLATE)" > "$(DESTDIR)$(APPLICATIONSDIR)/$(APP_ID).desktop"
	chmod 0644 "$(DESTDIR)$(APPLICATIONSDIR)/$(APP_ID).desktop"
	$(INSTALL) -Dm644 "$(APP_ICON)" "$(DESTDIR)$(ICONDIR)/$(APP_ID).svg"
	$(INSTALL) -d "$(DESTDIR)$(APPDATADIR)/assets"
	$(INSTALL) -m 0644 $(ASSET_FILES) "$(DESTDIR)$(APPDATADIR)/assets/"
	$(INSTALL) -d "$(DESTDIR)$(FONTDIR)"
	$(INSTALL) -m 0644 $(FONT_FILES) "$(DESTDIR)$(FONTDIR)/"
	@if command -v "$(GTK_UPDATE_ICON_CACHE)" >/dev/null 2>&1; then \
		"$(GTK_UPDATE_ICON_CACHE)" -q -t -f "$(DESTDIR)$(ICON_THEME_DIR)" || true; \
	fi
	@if command -v "$(UPDATE_DESKTOP_DATABASE)" >/dev/null 2>&1; then \
		"$(UPDATE_DESKTOP_DATABASE)" -q "$(DESTDIR)$(APPLICATIONSDIR)" || true; \
	fi
	@if command -v "$(FC_CACHE)" >/dev/null 2>&1; then \
		"$(FC_CACHE)" -q "$(DESTDIR)$(DATADIR)/fonts" || true; \
	fi
	@echo "Installed $(APP_NAME) to $(DESTDIR)$(BINDIR)/$(BIN_NAME)"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN_NAME)"
	rm -f "$(DESTDIR)$(APPLICATIONSDIR)/$(APP_ID).desktop"
	rm -f "$(DESTDIR)$(ICONDIR)/$(APP_ID).svg"
	rm -rf "$(DESTDIR)$(APPDATADIR)"
	rm -rf "$(DESTDIR)$(DATADIR)/fonts/$(BIN_NAME)"
	@if command -v "$(GTK_UPDATE_ICON_CACHE)" >/dev/null 2>&1; then \
		"$(GTK_UPDATE_ICON_CACHE)" -q -t -f "$(DESTDIR)$(ICON_THEME_DIR)" || true; \
	fi
	@if command -v "$(UPDATE_DESKTOP_DATABASE)" >/dev/null 2>&1; then \
		"$(UPDATE_DESKTOP_DATABASE)" -q "$(DESTDIR)$(APPLICATIONSDIR)" || true; \
	fi
	@if command -v "$(FC_CACHE)" >/dev/null 2>&1; then \
		"$(FC_CACHE)" -q "$(DESTDIR)$(DATADIR)/fonts" || true; \
	fi
