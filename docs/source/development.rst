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

Build cache cleanup
-------------------

Remove incremental compiler caches while keeping compiled dependencies and
application binaries:

.. code-block:: console

   $ make clean-incremental

This also handles target-specific build directories and respects
``CARGO_TARGET_DIR``. Stop active builds before cleaning their caches.
Use ``make clean`` to remove all Cargo build output. To prevent new incremental
caches during an individual build, use ``CARGO_INCREMENTAL=0 make build``.

Fedora CI and publishing
------------------------

The GitHub Actions workflow builds Fedora 44 x86_64 RPMs on pull requests and
pushes to ``main``. It checks formatting, runs ``cargo check --release --locked``
in the RPM build tree, builds binary and source RPMs, validates the desktop
entry, and installs the package from the generated DNF repository. CI caches
Cargo downloads and disables incremental compilation.

After a successful build on ``main``, the workflow deploys the documentation
and a DNF repository together to GitHub Pages. Pages must use GitHub Actions
as its publishing source. The repository lives at
``rpm/fedora/44/x86_64/`` beneath the Pages URL, with a generated configuration
at ``rpm/craic.repo`` and the corresponding source RPM at ``rpm/craic.src.rpm``.
Each deployment replaces the previous development build. The package version
comes from ``Cargo.toml``; the RPM release includes the workflow run number,
attempt, and commit identifier so later builds can be installed with DNF updates.

To retry publication without rebuilding the RPMs, run the workflow manually on
``main`` with ``fedora_run_id`` set to a completed run whose Fedora build
succeeded. The RPM artifact is retained for seven days.

Packages are unsigned by default. To enable package signatures, store an
ASCII-armored, unencrypted GPG signing private key in the repository's
``RPM_SIGNING_KEY`` Actions secret, for example with
``gh secret set RPM_SIGNING_KEY < signing-key.asc``. The workflow publishes the
public key and enables package signature checking in the generated repository
configuration. Repository metadata is served over HTTPS without a separate
signature. Keep the signing key stable across deployments.

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
