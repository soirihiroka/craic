%global debug_package %{nil}

Name:           craic
Version:        %{?package_version}%{!?package_version:0.1.0}
Release:        %{?package_release}%{!?package_release:1}%{?dist}
Summary:        A development environment with integrated coding agents
License:        GPL-3.0-or-later AND OFL-1.1 AND MIT AND BSD-3-Clause AND Unicode-3.0
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64

BuildRequires:  cargo
BuildRequires:  rust >= 1.94
BuildRequires:  clang
BuildRequires:  clang-devel
BuildRequires:  llvm
BuildRequires:  gcc-c++
BuildRequires:  make
BuildRequires:  python3
BuildRequires:  git
BuildRequires:  desktop-file-utils
BuildRequires:  curl
BuildRequires:  xz
BuildRequires:  pkgconfig(gtk4) >= 4.10
BuildRequires:  pkgconfig(libadwaita-1) >= 1.9
BuildRequires:  pkgconfig(poppler-glib) >= 26.1
BuildRequires:  pkgconfig(libpanel-1)
BuildRequires:  pkgconfig(webkitgtk-6.0)
BuildRequires:  pkgconfig(fontconfig)
BuildRequires:  pkgconfig(freetype2)
BuildRequires:  pkgconfig(egl)
BuildRequires:  pkgconfig(gl)
BuildRequires:  pkgconfig(wayland-egl)
Provides:       bundled(libghostty-vt) = 1.0.1
Provides:       bundled(highway) = 1.2.0
Provides:       bundled(simdutf) = 9.0.0
Provides:       bundled(translate_c) = 2.0.0
Provides:       bundled(uucode) = 0.2.0
Provides:       bundled(wuffs)
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
%license %{_datadir}/licenses/%{name}/ghostty-vt/
%{_bindir}/craic
%{_datadir}/applications/dev.craic.Craic.desktop
%{_datadir}/icons/hicolor/scalable/apps/dev.craic.Craic.svg
%{_datadir}/craic/
%{_datadir}/fonts/craic/
