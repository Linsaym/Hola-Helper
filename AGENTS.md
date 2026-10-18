# Hola Helper

Offline Windows voice assistant written in Rust.

## Project goals
- Fully offline voice processing
- Low CPU and memory usage
- Modular and maintainable architecture
- Wake word: "Hola"
- Wake word engine: sherpa-onnx
- Audio capture: cpal

## Development rules
- Keep implementations simple and efficient.
- Prefer small, focused changes.
- Do not introduce unnecessary dependencies.
- Do not change unrelated files.
- Use idiomatic Rust.
- Handle errors explicitly.
- Keep audio processing separate from application logic.
- Never introduce cloud services for voice processing.
- Run cargo fmt and cargo check after changes.
- Explain architectural decisions briefly.

## Workflow
- Inspect existing code before editing.
- Ask before making major architectural changes.
- Do not implement features beyond the requested scope.

## Communication
- Communicate with the user in Russian.
- Explain implementation decisions, debugging findings, and results in Russian.
- Keep source code, identifiers, and technical terminology in English.