Configuration
=============

User configuration is stored in ``$HOME/.craic/config.toml``. Preferences set
in the application are written to the same file.

Workspaces
----------

Use ``workspace_roots`` to discover projects beneath a directory and
``workspaces`` to add individual projects. A provider is either ``local`` or
``ssh:<host>``. SSH host names correspond to entries in ``$HOME/.ssh/config``.

.. code-block:: toml

   [[workspace_roots]]
   path = "~/Projects"
   provider = "local"
   color = "blue-4"

   [[workspace_roots]]
   path = "~/work"
   provider = "ssh:build-machine"
   name = "Remote projects"
   color = "purple-4"

   [[workspaces]]
   path = "~/Projects/example-app"
   name = "Example App"
   color = "green-4"

Colors can be hexadecimal values or named Adwaita palette colors such as
``blue-4``, ``green-3``, ``orange-5``, and ``purple-2``. A more specific
workspace color overrides its root or host color.

Remote host colors
------------------

A host-wide color applies to every workspace using that SSH host unless a
workspace or workspace root provides a more specific color.

.. code-block:: toml

   [[hosts]]
   host = "build-machine"
   color = "orange-5"

Coding agents
-------------

Choose the provider and optional model used to draft commit messages:

.. code-block:: toml

   commit_message_provider = "opencode"
   commit_message_model = "example-model"

Smart features can use a different provider and model for each agent shell:

.. code-block:: toml

   [smart_feature.codex]
   provider = "opencode"
   model = "example-model"

   [smart_feature.opencode]
   provider = "ollama"
   model = "local-model"

For a local Ollama server at a non-default address:

.. code-block:: toml

   [ollama]
   base_url = "http://localhost:11434"

Font sizes
----------

Editor, terminal, and diff font sizes accept values from 8 through 32 points.

.. code-block:: toml

   [font_size]
   editor = 14.0
   shell = 13.0
   diff = 13.0

Project configuration
---------------------

Repository-specific settings live in ``.craic/config.toml``. Machine-local
settings selected through Craic are stored separately under ``.craic/local``
and ignored by Git. These settings cover project actions and repository Git or
hosting preferences.
