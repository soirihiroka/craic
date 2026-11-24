Getting Started
===============

Fedora development packages
---------------------------

GitHub Pages hosts a DNF repository for Fedora 44 on x86_64 alongside this
documentation. Download the `repository configuration <rpm/craic.repo>`_ and
save it as ``/etc/yum.repos.d/craic.repo``, then install:

.. code-block:: console

   $ sudo dnf install craic

Use ``sudo dnf upgrade craic`` to install subsequent development builds.
The `source RPM <rpm/craic.src.rpm>`_ is published alongside each build.
Packages are unsigned unless the maintainer configures the CI signing secret;
the repository configuration reflects whether package signatures are enabled.

Requirements
------------

Craic requires Rust, Cargo, GTK 4, Libadwaita, GtkSourceView 5, WebKitGTK,
Poppler GLib, GObject Introspection, and pkg-config.

Fedora
~~~~~~

.. code-block:: console

   $ sudo dnf install rust cargo clang clang-devel llvm gcc-c++ make python3 git \
       gtk4-devel webkitgtk6.0-devel libpanel-devel fontconfig-devel \
       freetype-devel mesa-libEGL-devel mesa-libGL-devel wayland-devel \
       gtksourceview5-devel libadwaita-devel curl xz \
       poppler-glib-devel gobject-introspection-devel pkgconf-pkg-config

Ubuntu and Debian
~~~~~~~~~~~~~~~~~

.. code-block:: console

   $ sudo apt install rustc cargo libgtk-4-dev libgtksourceview-5-dev \
       libadwaita-1-dev curl xz-utils llvm libclang-dev libpoppler-glib-dev \
       gobject-introspection pkg-config

Terminal engine
~~~~~~~~~~~~~~~

Craic statically links Ghostty's terminal emulation library, libghostty-vt.
The first build fetches and verifies the pinned Ghostty release. On Linux,
the build uses Zig 0.17 or downloads the pinned compiler if needed. On
macOS, the build uses Ghostty's published macOS static library and requires
the Xcode tools. Craic also uses ``llvm-nm`` and ``llvm-objcopy`` to keep
Ghostty's bundled Wuffs library separate from Skia's. On macOS these are
available through Homebrew's ``llvm`` package. Set ``GHOSTTY_VT_DIR`` to
use a local installation of
libghostty-vt 1.0.1 or a compatible 1.x release. The directory must
contain ``include/ghostty/vt.h`` and a self-contained static archive at
``lib/libghostty-vt.a``.

Run from source
---------------

From the project directory:

.. code-block:: console

   $ make run

Set ``RUST_LOG`` to adjust application logging when diagnosing a problem:

.. code-block:: console

   $ RUST_LOG=craic=debug make run

Install
-------

The default installation prefix is ``$HOME/.local``.

.. code-block:: console

   $ make install

On macOS, this installs ``Craic.app`` in ``$HOME/Applications`` and the
command-line executable in ``$PREFIX/bin``. Use ``APPLICATIONSDIR`` to choose
a different app bundle destination, and use the same value when uninstalling.
On Linux, this installs the executable, desktop entry, icons, and fonts beneath
``PREFIX``.

Use ``PREFIX`` to select a different prefix. Use the matching value when
uninstalling:

.. code-block:: console

   $ make install PREFIX=/opt/craic
   $ make uninstall PREFIX=/opt/craic

Open a workspace
----------------

Craic discovers projects beneath configured workspace roots. By default it
looks in ``$HOME/Repos``. Select a project from the workspace picker to open
its editor, repository status, terminal, and available project actions.

See :doc:`configuration` to add other local roots or SSH workspaces.
