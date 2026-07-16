Features
========

Workspaces
----------

Craic can open local projects and projects available through an SSH host.
Workspace colors make it easier to distinguish projects and remote systems.

Editing and files
-----------------

The file browser and editor provide syntax highlighting, spellchecking,
Markdown linting, search, and common file operations. Craic can preview
Markdown, SVG, PDF, images, audio, video, SQLite databases, Jupyter notebooks,
fonts, and SafeTensors metadata without leaving the application.

Git hosting and changes
-----------------------

The changes view combines repository status, diffs, staging, commits, pull and
push operations, and commit history. Hosting integrations support common
GitHub, GitLab, and Bitbucket workflows when their command-line tools and
credentials are available.

Containers
----------

The containers view exposes Docker and Compose operations for a workspace.
Remote workspaces run supported container commands on their SSH host.

Coding agents
-------------

Craic integrates with Codex, AGY, OpenCode, and Ollama. Agents can help draft
commit messages and can be selected for supported smart features. Each
provider must be installed and authenticated independently before Craic can
use it.

Keyboard shortcuts
------------------

Open the in-app shortcut window with :kbd:`Ctrl+?`. Frequently used global
shortcuts include:

.. list-table::
   :header-rows: 1
   :widths: 55 45

   * - Action
     - Shortcut
   * - Open a new window
     - :kbd:`Ctrl+N`
   * - Open preferences
     - :kbd:`Ctrl+,`
   * - Refresh repository status
     - :kbd:`Ctrl+R`
   * - Pull remote changes
     - :kbd:`Ctrl+P`
   * - Push local commits
     - :kbd:`Ctrl+U`
   * - Search the current editor
     - :kbd:`Ctrl+F`
