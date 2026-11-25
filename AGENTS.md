Do not write tests for UI.
Unless specified, you should not write any test.

Focus on your assigned task. There might be other agents working on it at the same time.

Never build, package, or run compiler-backed checks or tests locally, including
through Make, a local container, or an IDE task. Use remote CI for builds and
run the Rust check there before reporting. Run `cargo fmt --all` locally.

On remote macOS runners, use the matching Make targets so rust-skia receives
the repository's Xcode Clang, SDK, and libclang environment.

Create commits with `TZ=UTC git commit` and verify that both the author and
committer timestamps use `+0000` with `git log -1 --format='%ai%n%ci'`.

Avoid custom CSS unless absolutely needed.

Prefer tokio over GIO

Remember to log while dealing with tricky life cycle problem. But avoid logging in a loop or trivial stuff.

When writing docs avoid leaking personal information (ip, specific repo names etc).

Remember to run outside of the sandbox.

Avoid trivial functions. If it can be done in a few lines, then just write that few lines instead of trying to abstract it.
