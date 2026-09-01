Do not write tests for UI.
Unless specified, you should not write any test.

Focus on your assigned task. There might be other agents working on it at the same time.

Static analysis are fine. Run rust check before reporting. You must run cargo fmt.

On macOS, never run Cargo build, check, test, or clean commands directly. Use the matching Make target so rust-skia always receives the repository's Xcode Clang, SDK, and libclang environment.

Avoid custom CSS unless absolutely needed.

Prefer tokio over GIO

Remember to log while dealing with tricky life cycle problem. But avoid logging in a loop or trivial stuff.

When writing docs avoid leaking personal information (ip, specific repo names etc).

Remember to run outside of the sandbox.

Avoid trivial functions. If it can be done in a few lines, then just write that few lines instead of trying to abstract it.
