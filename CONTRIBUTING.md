# Contributing to Tiiny Studio

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Reporting a bug or asking for a feature

Use the [issue forms](https://github.com/thezolon/tiiny-studio/issues/new/choose). For a bug, the version (hover the
gear icon, top right), how you run the studio, how the Tiiny is connected and the model involved save a round of
questions. Logs and screenshots help; please leave out API keys and your network addresses.

## Pull requests

- **Fork the repository and open a pull request against `main`.** `main` only takes changes through pull requests,
  including the maintainer's own.
- **One change per pull request,** on its own branch, with a short description: what it changes, why, and how you
  tested it. Smaller is easier to review and merge quickly.
- **Match the surrounding code:** the smallest change that does the job, in the style of the file you're in.
- **Contributions are under the project's license** ([Apache-2.0](LICENSE)).

### Where things live

| Part | Where |
|---|---|
| The studio's backend (Rust) | `rust/studio-core` |
| Headless server / Docker | `rust/studio-server` |
| Desktop app (Tauri) | `rust/studio-desktop` |
| The UI (plain JavaScript, no build step) | `static/` |
| The original Python server (kept for now; not tested any more, and without 0.7.1's network protections) | `server.py`, `media.py`, `chat.py`, `extract.py` |
| What the Tiiny's API does, as found by testing | `docs/DEVICE-API.md` |
| A fake Tiiny and the tests | `testdata/` |

Integrations that package the studio for somewhere else (an agent store, a launcher, …) go in their own top-level
folder, so they stay apart from the app.

### Running and testing

- Run it: `cd rust && cargo run -p studio-server`, then open <http://localhost:7860>. A change in `static/` is built
  into the Rust server, so rebuild after editing it (`python server.py` serves `static/` directly).
- No Tiiny needed: `python testdata/fake_tiiny.py --port 7791 --key k` is a stand-in device; point the studio at
  `http://127.0.0.1:7791` with the key `k`.
- What CI runs on every pull request that touches code (worth running before you push):
  - `cd rust && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
  - `ruff check --select F,B server.py media.py chat.py extract.py safety.py tools testdata`
  - `python -m unittest discover testdata/contract` (contract tests against the fake Tiiny)
  - `python testdata/a11y_check.py` (accessibility, WCAG 2.2 AA; needs Playwright)
- On a real Tiiny: name anything a test creates with `TEST` and delete it afterwards, and unload any model the test
  loaded.

## A few rules the project keeps

- **Privacy:** no API keys, device names, serial numbers, MAC addresses or real network addresses in the repository;
  use neutral examples such as `192.168.1.50`. Results stay on the user's computer, and the studio sends no telemetry.
- **Content that reaches a model:** documents, OCR text, transcripts and anything else from outside go to a model only
  inside clear delimiters and after the studio's injection screening, and a suspected injection is shown to the user,
  never silently dropped. Prompts and instructions added for a model are reviewed like code.
- **Accessibility:** everything works from the keyboard and with screen readers, in both themes.
- **The device:** one request at a time; don't add anything that powers off or reboots the Tiiny without the user
  asking.

Questions? Open an issue, or find the project on the [Tiiny Discord](https://discord.com/invite/R5CHUuXy4A)
(#project-showcase → Tiiny Studio).
