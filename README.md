# Craic

A development environment made by me for me :D

Any usage outside of my computer is out of scope and unsupported x3

<img width="1207" height="764" alt="image" src="https://github.com/user-attachments/assets/511380e1-a28e-42b3-b03c-7896885072ce" />

---

## Features

- Git integration with auto generated commit messages.
- Docker integration.
- SSH Remote connection.
- Vibe coding integration with Codex, AGY, OpenCode, and Ollama.
- Document preview for Markdown, SVG, PDF, images, audio, and video.
- Syntax highlighting, spellchecking, and Markdown linting.
- Color-coding for workspaces and hosts.

---

## Requirements

Install Rust and system dependencies:

### Fedora

```sh
sudo dnf install rust cargo gtk4-devel webkitgtk6.0-devel gtksourceview5-devel libadwaita-devel vte291-gtk4-devel poppler-glib-devel gobject-introspection-devel pkgconf-pkg-config
```

### Ubuntu / Debian

```sh
sudo apt install rustc cargo libgtk-4-dev libgtksourceview-5-dev libadwaita-1-dev libvte-2.91-gtk4-dev libpoppler-glib-dev gobject-introspection pkg-config
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

### Uninstall

```sh
make uninstall
```

---

## Configuration

Settings are stored in `~/.craic/config.toml`:

```toml
# AI Assist Providers
commit_message_provider = "opencode"
commit_message_model = "deepseek-coder"

# Local Ollama URL (optional)
ollama.base_url = "http://localhost:11434"

# Custom Smart Feature Configurations
[smart_feature.codex]
provider = "opencode"
model = "deepseek-coder"

# Font Sizes
[font_size]
editor = 14.0
shell = 13.0
diff = 13.0

# Explicit individual workspaces
[[workspaces]]
path = "~/workspaces/project-alpha"
name = "Alpha Project"
color = "purple"

[[workspaces]]
path = "~/workspaces/project-beta"
provider = "ssh:remote-server"
color = "blue-4"

# Workspace root directories
[[workspace_roots]]
path = "~/Repos"
provider = "local"

# Custom background colors for SSH hosts
[[hosts]]
host = "remote-server"
color = "orange-5"
```

---

## License

Craic is distributed under the terms of the GNU General Public License Version 3. See [LICENSE](LICENSE) for details.

