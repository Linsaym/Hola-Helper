# Hola Helper

Offline Windows voice assistant written in Rust.

## Project goals
- Fully offline voice processing.
- Low CPU and memory usage.
- Simple, modular, and maintainable architecture.
- Wake word: "Hola".
- Audio capture: cpal.

## Development rules
- Keep implementations simple and efficient.
- Prefer small, focused changes.
- Avoid unnecessary dependencies and abstractions.
- Do not change unrelated files.
- Use idiomatic Rust.
- Handle errors explicitly.
- Keep audio processing separate from application logic.
- Separate speech recognition from action execution.
- Never introduce cloud services for voice processing.
- Explain architectural decisions briefly.

## Workflow
- Inspect existing code before editing.
- Ask before making major architectural changes.
- Do not implement features beyond the requested scope.
- Update README.md when functionality, setup, or usage changes.
- After code changes, run:
  - `cargo fmt --check`
  - `cargo check`
  - `cargo test`
- Report verification results and any remaining issues.

## Communication
- Communicate with the user in Russian.
- Explain implementation decisions, debugging findings, and results in Russian.
- Keep source code, identifiers, and technical terminology in English.