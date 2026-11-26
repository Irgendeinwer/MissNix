# AGENTS.md — Architecture & Contributor Guide

This document defines the architecture, invariants, UI constraints, and verification workflow for contributors and coding agents working on **MissNix**.

---

## 1. Project Overview

**MissNix** is a standalone Linux/macOS/Windows desktop **Local Music Release Radar**:
1. Scans local audio directories (`lofty` tags + `<Artist>/<Year - Album>` folder structure) to build a clean primary-artist roster.
2. Persists folders, artists, indexed local albums, remote releases, cached tracklists, and user preferences in SQLite (`rusqlite`, WAL mode).
3. Queries **Deezer** (fast artist/discography lookup, artwork, tracklists, 30s MP3 previews) and **MusicBrainz** (canonical MBIDs, 1.1s rate-limited release groups, Cover Art Archive), deduplicating editions and flagging locally owned albums.
4. Presents everything in a responsive **Slint 1.19** Material 3 desktop GUI with an in-app 30-second audio preview player and an optional headless CLI (`--scan`, `--check`, `--status`).

---

## 2. Workspace Layout & Separation of Concerns

```text
MissNix/
├── crates/missnix-core/          # GUI-independent pure-Rust library (unit-tested offline)
│   └── src/
│       ├── models.rs             # Domain types, enums, AppSettings, FeedQuery
│       ├── normalize.rs          # Artist/collaboration splitting, edition stripping, UTC dates
│       ├── scanner.rs            # Audio tag + directory hierarchy scanner (ScanOptions)
│       ├── db.rs                 # SQLite schema (SCHEMA_VERSION), non-destructive migrations, queries
│       ├── sync.rs               # Provider orchestration, cover/avatar/MP3 caching, URL refresh
│       └── providers/            # Deezer & MusicBrainz REST clients + homonym scoring
├── src/
│   ├── main.rs                   # CLI parser + Slint GUI entrypoint
│   ├── controller.rs             # Non-blocking UI <-> missnix-core bridge, image worker, HiDPI scaling
│   ├── audio_player.rs           # Dedicated MP3 preview decoder (nanomp3) & PulseAudio/ALSA/CLI sink
│   └── folder_picker.rs          # Native desktop folder chooser helper (zenity/kdialog/osascript/PS)
├── ui/
│   ├── theme.slint               # Material 3 Dark/Light surface tokens & 6 accent presets
│   ├── models.slint              # Shared UI row/card structs
│   ├── components.slint          # Bounded Material 3 desktop controls, cards, mini-player
│   └── app-window.slint          # 3-destination navigation rail, unified Release Radar, right inspector
├── assets/
│   ├── missnix.svg               # Launcher icon (installed to share/icons/hicolor/scalable/apps/)
│   ├── icons/*.svg               # Monochrome #FFFFFF vector icons tinted via Slint Image.colorize
│   └── docs/*.svg                # README banner & interface preview graphics
└── .github/workflows/
    ├── ci.yml                    # Cargo (fmt, clippy -D warnings, test, build, CLI) + Nix Flake check
    └── release.yml               # Immutable SLSA-attested release pipeline
```

Keep `crates/missnix-core` completely free of `slint` or audio-device dependencies so scanning, normalization, SQLite migrations, and provider sync remain fast to compile and unit-test headlessly.

---

## 3. Core Engineering Invariants

### SQLite Schema & Data Preservation (`crates/missnix-core/src/db.rs`)
- **Never wipe user data on schema upgrades.** If columns or tables change, increment `SCHEMA_VERSION` and add an additive `ALTER TABLE` migration (preserving `library_folders`, `artists` including `muted`/`ignored` statuses and manual entries, `releases` `seen`/`dismissed` states, and `settings`).

### Artist Discovery & Multi-Artist Tag Hygiene (`normalize.rs` & `scanner.rs`)
- Default discovery mode is `ArtistDiscoveryMode::AlbumArtistOrFolder`: prefer `ALBUMARTIST` and structured `<Artist>/<Album>/<Track>` directories over track `ARTIST` so featured guests do not pollute the artist list.
- `extract_primary_artist` splits collaboration delimiters (`;`, `\`, ` / `, `feat.`, `, `) while preserving legitimate comma/slash artist names (`Tyler, The Creator`, `Black Country, New Road`, `Earth, Wind & Fire`, `AC/DC`, `+44`).
- Only attach embedded MusicBrainz Artist IDs when the embedded tag matches the resolved primary artist (`safe_tag_mbid`).
- `apply_scan_outcome` prunes stale `source = 'local' AND status = 'tracked' AND local_track_count = 0` entries on rescan while keeping manual artists and `muted`/`ignored` preferences.

### Slint 1.19 Desktop UI & Performance Rules (`ui/*.slint` & `src/controller.rs`)
- **Bounded desktop controls**: Never use unbounded `std-widgets` `Button` or `LineEdit` in flexible layouts where they can stretch vertically or horizontally with window size. Use `M3Button`, `M3IconButton`, `M3FilterChip`, `M3SearchField`, `M3LabeledInput`, and `TonalBadge` with explicit fixed `height` and `vertical-stretch: 0`.
- **Slint dimension constraint rule**: Never set both `height` and `min-height`/`max-height` (or both `width` and `min-width`/`max-width`) on the same Slint element — Slint treats that as a compile error.
- **Explicit `x: 0px` on non-layout progress fills**: Any child `Rectangle` inside a plain `Rectangle` whose `width` varies (e.g. seek bars, volume bars) **must** specify `x: 0px;` — otherwise Slint centers the child horizontally and animates outward from the middle.
- **Full-width `ScrollView` children**: Inside a `ScrollView`, bind the root `VerticalLayout` / `HorizontalLayout` width to `scroll.visible-width` and `alignment: start` so headers and cards span the full panel width without collapsing or stretching vertically.
- **Viewport culling on large grids**: Keep viewport row culling active in `feed-scroll` (`ui/app-window.slint`) and decode grid cover thumbnails at bounded resolution (`300px`) so window resizing stays smooth even with hundreds of releases.
- **HiDPI UI scaling**: Runtime UI scaling dispatches `slint::platform::WindowEvent::ScaleFactorChanged` + `WindowEvent::Resized` relative to the OS monitor's native `base_scale_factor` so fonts, SVGs, hit-testing, and layouts scale crisply without requiring a restart.
- **Non-blocking UI thread**: All filesystem scanning, HTTP requests, image decoding, and MP3 decoding/playback must run on background threads and communicate back via `slint::invoke_from_event_loop` or channels.

### Audio Preview Engine (`src/audio_player.rs` & `sync.rs`)
- Deezer `preview_url` CDN query strings contain expiring `hdnea=exp=<unix_ts>` tokens. Always check `is_deezer_preview_url_expired` and refresh track URLs via `refresh_release_tracks` on expiration or HTTP 401/403 before failing playback.
- Runtime audio libraries (`libpulse-simple.so.0`, `libasound.so.2`) are loaded dynamically via `libloading` with a subprocess fallback (`pw-cat` / `paplay` / `play`), keeping compile-time dependencies light while `flake.nix` wraps `libpulseaudio` and `alsa-lib` into `LD_LIBRARY_PATH`.

---

## 4. Verification & Testing Workflow

Run all checks locally (inside `nix develop` or a Rust toolchain environment). When sharing logs for debugging, tee output to `log.txt`:

```bash
{
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  nix flake check
} 2>&1 | tee log.txt
```

Quick headless CLI smoke test:

```bash
cargo run -- --db /tmp/missnix-smoke.db --scan ~/Music --status
```

---

## 5. Release Process (Immutable Releases)

1. Bump `version` identically in `Cargo.toml` and `crates/missnix-core/Cargo.toml` (`flake.nix` reads `Cargo.toml` automatically) and update `Cargo.lock`.
2. Commit to `main` with `release: vX.Y.Z` (or `chore(release): vX.Y.Z`) and push — `.github/workflows/release.yml` verifies tests, builds the release archive + `SHA256SUMS`, signs SLSA build provenance, creates tag `vX.Y.Z`, and publishes the immutable GitHub Release.
