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

Documentation
-------------

The wiki follows the GNOME Human Interface Guidelines documentation setup: it
uses reStructuredText, Sphinx, and the Furo theme with a small GNOME-inspired
style layer. Python dependencies are isolated and locked with uv.

Install or update the documentation environment:

.. code-block:: console

   $ uv sync --project docs

Build the HTML documentation and treat Sphinx warnings as errors:

.. code-block:: console

   $ make doc

The generated site is written to ``docs/build/index.html``. Source pages live
under ``docs/source``; add new pages to the ``toctree`` in ``index.rst`` so
Sphinx includes them in navigation and checks their links.
