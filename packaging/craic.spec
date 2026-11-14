%global debug_package %{nil}

Name:           craic
Version:        %{?package_version}%{!?package_version:0.1.0}
Release:        %{?package_release}%{!?package_release:1}%{?dist}
Summary:        A development environment with integrated coding agents
License:        GPL-3.0-or-later AND OFL-1.1
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64

BuildRequires:  cargo
BuildRequires:  rust >= 1.94
BuildRequires:  clang
BuildRequires:  clang-devel
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  python3
BuildRequires:  git
BuildRequires:  desktop-file-utils
BuildRequires:  pkgconfig(gtk4) >= 4.10
BuildRequires:  pkgconfig(libadwaita-1) >= 1.9
BuildRequires:  pkgconfig(gtksourceview-5)
BuildRequires:  pkgconfig(vte-2.91-gtk4) >= 0.84
BuildRequires:  pkgconfig(poppler-glib) >= 26.1
BuildRequires:  pkgconfig(libpanel-1)
BuildRequires:  pkgconfig(webkitgtk-6.0)
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(freetype2)
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(gl)
BuildRequires:  pkgconfig(wayland-egl)
Requires:       glycin-loaders
Requires:       hicolor-icon-theme
Recommends:     git

%description
Craic is a GTK development environment with integrated coding agents,
repository tools, terminals, and document previews.

%prep
%autosetup

%build
export CARGO_INCREMENTAL=0
cargo build --release --locked

%install
%make_install -o release PREFIX=%{_prefix} \
    GTK_UPDATE_ICON_CACHE=true UPDATE_DESKTOP_DATABASE=true FC_CACHE=true

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/dev.craic.Craic.desktop

%files
%license LICENSE
%license crates/craic-ui-core/assets/fonts/JetBrainsMono/OFL.txt
%{_bindir}/craic
%{_datadir}/applications/dev.craic.Craic.desktop
%{_datadir}/icons/hicolor/scalable/apps/dev.craic.Craic.svg
%{_datadir}/craic/
%{_datadir}/fonts/craic/
