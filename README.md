# Craic

A sleek GNOME Vibe IDE & Agentic Development Environment built with GTK 4 and libadwaita.

<img width="1207" height="764" alt="image" src="https://github.com/user-attachments/assets/511380e1-a28e-42b3-b03c-7896885072ce" />


Craic lets you develop code alongside autonomous AI agents (Codex, Antigravity, OpenCode, Ollama) with integrated Git workflow, an embedded terminal, and a syntax-highlighted code editor.

---

## Requirements

Install Rust and system dependencies:

### Fedora
```sh
sudo dnf install rust cargo gtk4-devel webkitgtk6.0-devel libadwaita-devel vte291-gtk4-devel poppler-glib-devel gobject-introspection-devel pkgconf-pkg-config
```

### Ubuntu / Debian
```sh
sudo apt install rustc cargo libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev libpoppler-glib-dev gobject-introspection pkg-config
```

---

## Configuration

Settings are stored in `~/.craic/config.toml`:

* **`workspace_roots`**: Directories whose immediate children are workspaces. Entries may be strings for local paths or tables with `path` and optional `provider` such as `ssh:remote.host`.
* **`workspaces`**: Explicit individual workspaces. Entries support the same optional `provider` field.
* **`commit_message_provider`**: Agent to use for commit messages (`codex`, `agy`, `opencode`, `ollama`).
* **`ollama.base_url`**: Endpoint for local Ollama server.
* **`font_size`**: Editor, terminal, and diff view sizes:

Example SSH workspace:
```toml
[[workspaces]]
path = "~/workspaces/project"
provider = "ssh:remote.host"
```

Example SSH workspace root:
```toml
[[workspace_roots]]
path = "~/workspaces"
provider = "ssh:remote.host"
```

---

## Usage

### Run
```sh
cargo run
```

### Install
Installs build artifacts, desktop launcher, and icons under `~/.local`:
```sh
make install
```
