# Publishing Tiiny Studio (maintainer checklist)

For 0.2.0, the first public release (0.1.0, the Python version, was never published). Not part of release
packages (`tools/` is export-ignored).

What `.github/workflows/release.yml` builds:

| job | runs on | output |
|---|---|---|
| Desktop app for Windows x64 | `windows-latest` | `Tiiny Studio_<v>_x64-setup.exe`, `Tiiny Studio_<v>_x64_en-US.msi` |
| Desktop app for Windows ARM | `windows-11-arm` | `Tiiny Studio_<v>_arm64-setup.exe`, `Tiiny Studio_<v>_arm64_en-US.msi` |
| Desktop app for macOS Apple Silicon | `macos-latest` | `Tiiny Studio_<v>_aarch64.dmg` |
| Desktop app for macOS Intel | `macos-15-intel` | `Tiiny Studio_<v>_x64.dmg` |
| Desktop app for Linux x64 | `ubuntu-22.04` | `Tiiny Studio_<v>_amd64.deb`, `Tiiny Studio_<v>_amd64.AppImage` |
| Desktop app for Linux ARM | `ubuntu-22.04-arm` | `Tiiny Studio_<v>_arm64.deb`, `Tiiny Studio_<v>_aarch64.AppImage` |
| Docker image | `ubuntu-latest` | `ghcr.io/<owner>/tiiny-studio:<v>` and `:latest` (amd64 + arm64) |
| GitHub release | `ubuntu-latest` | source zip (`tools/release.py`), the installers, `SHA256SUMS.txt`, notes from `CHANGELOG.md` |

Each app job also builds `studio-server` and smoke-tests it (starts, serves the page and `/api/settings`). The
installers are **unsigned** (see ROADMAP.md).

**Started by a tag (`v*`)** everything runs: the Docker image is pushed and the release is created.
**Started by hand** (`workflow_dispatch`) it's a dry run: the installers are kept as workflow artifacts on the
run's page, the Docker image is built but **not pushed**, and **no release** is created. `rust.yml` (tests on
Windows/macOS/Linux, contract tests, document parity) runs on every push that touches the code.

## 1. Private push and dry run (no tag, nothing public)

- [ ] `git status` clean; `cargo test --workspace` in `rust/` and `python -m unittest discover testdata/contract`
      pass.
- [ ] `git log -p | grep` for keys, device names and LAN addresses: only `192.168.1.x`/`172.20.31.109`
      examples. `data/` and `.env` are ignored.

```bash
gh repo create thezolon/tiiny-studio --private --source . --remote origin --push
gh workflow run release.yml --ref main
gh run watch
gh run download <run-id> --dir build/ci        # the installers, to try by hand
```

macOS runners count 10× against a private repo's Actions minutes (Linux 1×, Windows 2×); a full dry run is
roughly 15–25 minutes per job.

- [ ] All six app jobs and the Docker job green; `rust.yml` green.
- [ ] Try the macOS and Linux installers by hand if you can (only Windows has been tried so far).

## 2. Go public (hard to undo)

- [ ] `VERSION`, `rust/studio-desktop/tauri.conf.json` and the top of `CHANGELOG.md` say the version you're
      tagging; change `## 0.2.0 (unreleased)` to the date. The release notes are the `## <VERSION>` section.
- [ ] README and ROADMAP.md current.
- [x] Description and topics set on the repo (2026-10-28).

```bash
gh repo edit thezolon/tiiny-studio --visibility public --accept-visibility-change-consequences
git tag -a v0.2.0 -m "Tiiny Studio 0.2.0"
git push origin v0.2.0
gh run watch
```

## 3. After

- [ ] Release page: 10 installers (2 Windows x64, 2 Windows ARM, 2 macOS, 2 Linux x64, 2 Linux ARM), the source zip and
      `SHA256SUMS.txt`. If one platform failed, the release still goes out without it; re-run that job, then
      `gh release upload v0.2.0 <file>` and update `SHA256SUMS.txt`.
- [ ] Container package: github.com/thezolon?tab=packages → `tiiny-studio` → **Package settings → Public**, so
      `docker compose up -d` can pull without logging in. Check it's linked to the repo.
- [ ] In a fresh folder with the source zip: `docker compose up -d`, open <http://localhost:7860>.
- [ ] Install one desktop app on a machine that never had the studio; first-run screen, connect, one image.

## Later releases

After a TiinyOS update, `python tools/api_snapshot.py --check` and the live tests first. Bump `VERSION` and
`tauri.conf.json`, add a `## <version>` section to `CHANGELOG.md`, update ROADMAP.md, commit,
`git tag -a v<version> -m …`, `git push origin main v<version>`.
