Getting Started
===============

Requirements
------------

Craic requires Rust, Cargo, GTK 4, Libadwaita, GtkSourceView 5, VTE, WebKitGTK,
Poppler GLib, GObject Introspection, and pkg-config.

Fedora
~~~~~~

.. code-block:: console

   $ sudo dnf install rust cargo gtk4-devel webkitgtk6.0-devel \
       gtksourceview5-devel libadwaita-devel vte291-gtk4-devel \
       poppler-glib-devel gobject-introspection-devel pkgconf-pkg-config

Ubuntu and Debian
~~~~~~~~~~~~~~~~~

.. code-block:: console

   $ sudo apt install rustc cargo libgtk-4-dev libgtksourceview-5-dev \
       libadwaita-1-dev libvte-2.91-gtk4-dev libpoppler-glib-dev \
       gobject-introspection pkg-config

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
