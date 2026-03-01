# r4dio — Project Status and Direction

Internal developer document (local/dev workflow reference).

## Current product snapshot (v1.1 line)

### Shipped capabilities

- Single-process TUI player (`radio-tui`) for stations + local files
- Station stream proxy (`:8990`) used by playback and meter/scope path
- Passive station polling with NTS + non-NTS resolvers (`p` toggle)
- NTS Infinite Mixtape metadata integration
- Song identification (`vibra`) and NTS download (`yt-dlp`)
- Optional HTTP control API on `:8989`
- Bundled cross-platform distribution with runtime discovery for external tools

### Current technical baseline

- Active binary: `crates/radio-tui`
- Shared models/config: `crates/radio-proto`
- Legacy reference only: `crates/radio-daemon`
- Old prototype (ignored by workspace): `src/`

## Active engineering directions

1. Remove crash-prone `unwrap/expect` sites in playback/proxy code paths.
2. Reduce duplication in playback teardown and platform audio-observer parsing.
3. Continue warnings cleanup and dead-code removal without changing runtime behavior.
4. Keep polling behavior stable and bounded under poor networks.
5. Preserve release portability across macOS/Linux/Windows bundles.

## Known constraints

- Scope currently follows station PCM path; local files still rely on lavfi level path.
- macOS bundles are functional but unsigned unless signing pipeline is configured.
- Fallback M3U URL policy should avoid private-only sources.

## Branch and documentation policy

- `dev` is **local-only** development branch; do not push `dev` to GitHub.
- GitHub should publish from `main` only.
- Keep internal planning docs local (for example: `PROJECT.md`, `AGENTS.md`, iteration notes).
- Public/main-branch docs should stay focused on user-facing + technical runtime docs (`README.md`, `architecture.md`) and source code.

## Merge/release guardrails

1. Develop and test on local `dev`.
2. Merge into `main` only when release-ready.
3. Before pushing `main`, verify internal-only docs are not staged for publication.
4. Tag release on `main`; CI builds platform artifacts.
