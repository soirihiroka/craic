Development
===========

Application
-----------

Useful development commands are exposed by the project Makefile:

.. code-block:: console

   $ make run
   $ make check
   $ make build
   $ make release

Run ``cargo fmt`` before submitting Rust changes.

Resource diagnostics
--------------------

Watcher lifecycle logs include stable subscription identifiers, active watcher
counts, watched path counts, callback batches, and teardown summaries. Enable
the relevant debug logs while reproducing a slowdown:

.. code-block:: console

   $ RUST_LOG=craic_system=debug,craic_ui_agent=debug,craic_ui_file=debug make run

In a second terminal, sample the running process. The output is tab-separated
and tracks CPU, resident memory, threads, file descriptors, descendant
processes, and inotify marks over time:

.. code-block:: console

   $ make resource-watch PID=$(pgrep -n -x craic) INTERVAL=5

Repeat the operation under investigation and verify that thread, descriptor,
inotify, and memory counts return to a stable baseline after subscriptions are
closed or workspaces are changed.

Documentation
-------------

The wiki follows the GNOME Human Interface Guidelines documentation setup: it
uses reStructuredText, Sphinx, and the HIG's Furo layout and theme styles.
Python and theme dependencies are isolated and locked with uv.

Install or update the documentation environment:

.. code-block:: console

   $ uv sync --project docs

Build the HTML documentation and treat Sphinx warnings as errors:

.. code-block:: console

   $ make doc

The generated site is written to ``docs/build/index.html``. Source pages live
under ``docs/source``; add new pages to the ``toctree`` in ``index.rst`` so
Sphinx includes them in navigation and checks their links.
