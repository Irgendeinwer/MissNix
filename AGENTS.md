# AGENTS.md — Architecture & Contributor Guide

This document defines the technical architecture, module layout, storage/runtime details, UI invariants, and verification workflow for contributors and coding agents working on **MissNix**.

---

## 1. Project Overview

> *"A desktop release radar for your local music library — scan your folders, track favorite artists, and miss nothing."*

**MissNix** is a standalone desktop application built with **Rust (Edition 2024, Cargo resolver v3)**, **Slint 1.19**, and **SQLite (`rusqlite`, bundled)**:
1. Scans local audio directories (`lofty` metadata tags + `<Artist>/<Year - Album>` directory inference) across FLAC, MP3, Ogg Vorbis, Opus, M4A/MP4, WAV, AIFF, AAC, APE, and WavPack files to build a clean primary-artist roster.
2. Persists library folders, tracked/muted/ignored artists, indexed local albums, discovered releases, cached tracklists, and user preferences in SQLite WAL mode.
3. Queries **Deezer** (fast artist/discography lookup, cover artwork, tracklists, 30s MP3 preview URLs, explicit flags) and **MusicBrainz** (canonical MBIDs, 1.1s rate-limited release groups, Cover Art Archive), scoring homonyms against local albums, collapsing Deluxe/Remastered duplicate editions, and suppressing locally owned albums.
4. Synthesizes full Dark & Light Material 3 tonal palettes in perceptual **OKLCH** color space (`crates/missnix-core/src/palette.rs`) for 7 curated presets plus arbitrary `#RRGGBB` custom seed colors.
5. Plays 30-second MP3 previews directly in-app via pure-Rust `nanomp3` decoding and dynamic `libloading` bindings for PulseAudio / PipeWire (`libpulse-simple.so.0`), ALSA (`libasound.so.2`), or CLI (`pw-cat` / `paplay` / `aplay` / `ffplay`) fallback, alongside an optional headless CLI (`--scan`, `--check`, `--status`).

---

## 2. Workspace & Module Layout

```text
MissNix/
├── crates/missnix-core/              # GUI-independent pure-Rust library (unit-tested offline)
│   └── src/
│       ├── lib.rs                    # Public core API surface
│       ├── error.rs                  # CoreError definitions
│       ├── models.rs                 # Domain types, enums, AppPaths, AppSettings, FeedQuery
│       ├── normalize.rs              # Collaboration splitting, edition stripping, UTC date helpers
│       ├── palette.rs                # Perceptual OKLCH Material 3 tonal palette engine + hex parser
│       ├── sync.rs                   # Provider orchestration, artwork/avatar/MP3 cache, CDN refresh
│       ├── db/                       # SQLite persistence layer
│       │   ├── mod.rs                # Connection setup, WAL/pragmas, transactions, stats
│       │   ├── schema.rs             # Canonical DDL,SCHEMA_VERSION migrations, schema validation
│       │   ├── folders.rs            # Library folder CRUD & scan-outcome reconciliation
│       │   ├── artists.rs            # Artist queries, status transitions, manual additions
│       │   ├── releases.rs           # Transactional release consolidation, feed queries, tracks
│       │   ├── release_match.rs      # Conservative provider-ID, title, date and ambiguity matching
│       │   ├── settings.rs           # Typed key-value AppSettings load/save
│       │   └── tests.rs              # Offline SQLite migration, deduplication, and settings tests
│       ├── scanner/                  # Local filesystem & audio metadata scanner
│       │   ├── mod.rs                # Directory walker, ScanOptions, artist/album aggregation
│       │   ├── tags.rs               # Lofty tag extraction & <Artist>/<Album> folder inference
│       │   └── tests.rs              # Synthetic ID3v2/folder hierarchy unit tests
│       └── providers/                # Keyless remote metadata clients
│           ├── mod.rs                # Shared provider traits & candidate types
│           ├── deezer.rs             # Deezer REST client + local-album homonym scoring
│           └── musicbrainz.rs        # Rate-limited MusicBrainz & Cover Art Archive client
├── src/
│   ├── main.rs                       # CLI flag parser, headless runner, Slint GUI entrypoint
│   ├── folder_picker.rs              # Native folder chooser (zenity/kdialog/osascript/PowerShell)
│   ├── audio_player/                 # Background 30-second MP3 preview engine
│   │   ├── mod.rs                    # Worker thread, HTTP/disk cache, nanomp3 decode, RMS meter
│   │   └── sinks.rs                  # Dynamic PulseAudio, ALSA, and subprocess PCM output sinks
│   └── controller/                   # Non-blocking Slint <-> missnix-core bridge
│       ├── mod.rs                    # UiState setup, Wayland/winit HiDPI scale sync, auto-check timer
│       ├── hydrate.rs                # SQLite -> Slint VecModel projection & multi-line stats formatting
│       ├── radar_actions.rs          # Release Radar filter, inspector, and audio preview callbacks
│       ├── artist_actions.rs         # Tracked Artists filter, search, mute/ignore/delete callbacks
│       ├── settings_actions.rs       # Folder, OKLCH theme, custom hex, and HiDPI scale callbacks
│       └── workers.rs                # Background scan, release sync, tracklist, and cover workers
├── ui/
│   ├── theme.slint                   # Global reactive Material 3 surface & accent tokens
│   ├── models.slint                  # Shared Slint structs (ReleaseCardData, ArtistRowData, etc.)
│   ├── components.slint              # Re-export barrel for reusable UI components
│   ├── app-window.slint              # Top-level Window shell, keyboard shortcuts, layout composition
│   ├── components/
│   │   ├── primitives.slint          # MissNixLogo, M3Button, M3IconButton, M3FilterChip, inputs, swatches
│   │   ├── controls.slint            # NavRailItem, SectionCard, M3Switch, M3Select, M3ProgressBar, M3ScrollBar
│   │   ├── media.slint               # EqualizerBars, CoverArtwork, ArtistAvatar, PreviewMiniPlayer
│   │   ├── release-widgets.slint     # ReleaseCard, TrackRow
│   │   └── library-widgets.slint     # ArtistListRow, SearchCandidateRow, LocalAlbumItem, FolderItemRow
│   └── views/
│       ├── nav-rail.slint            # Left navigation rail, sync actions, multi-line telemetry card
│       ├── radar-view.slint          # Unified Release Radar toolbar & viewport-culled cover grid
│       ├── artists-view.slint        # Tracked Artists filter bar, external search, virtualized list
│       ├── settings-view.slint       # Library folders, OKLCH theme & custom hex, HiDPI & sync settings
│       └── inspector-drawer.slint    # Right slide-out Release & Artist detail inspector
├── assets/
│   ├── missnix.svg                   # Desktop launcher icon (hicolor/scalable/apps/missnix.svg)
│   ├── icons/*.svg                   # Monochrome #FFFFFF vector icons tinted via Slint Image.colorize
│   └── docs/                         # README SVG artwork and app screenshots
└── .github/workflows/
    ├── ci.yml                        # Cargo + Determinate Nix Flake CI with Magic Nix Cache & rust-cache
    └── release.yml                   # Immutable SLSA-attested AppImage + tar.gz GitHub Release pipeline
```

Keep `crates/missnix-core` completely free of `slint` or audio-device dependencies so scanning, normalization, palette synthesis, SQLite migrations, and provider sync compile quickly and run offline unit tests.

---

## 3. Data, Cache & System Integration Details

### XDG Storage & Cache Paths (`AppPaths::discover`)

| Path | Purpose |
| :--- | :--- |
| `$XDG_DATA_HOME/missnix/missnix.db` | SQLite WAL database (folders, artists, local albums, releases, tracks, settings) |
| `$XDG_CACHE_HOME/missnix/db-<path-hash>/covers/` | Database-scoped release cover artwork |
| `$XDG_CACHE_HOME/missnix/db-<path-hash>/avatars/` | Database-scoped artist portraits |
| `$XDG_CACHE_HOME/missnix/db-<path-hash>/previews/` | Database-scoped 30-second MP3 track previews |

### Scheduled Headless Background Checks (`systemd --user`)

MissNix shares the same SQLite WAL database between headless CLI runs and the desktop UI:

```ini
# ~/.config/systemd/user/missnix-check.service
[Unit]
Description=Check local music artists for new releases (MissNix)

[Service]
Type=oneshot
ExecStart=%h/.nix-profile/bin/missnix --check --status
```

### Release Artifact Verification (SHA-256 & SLSA Provenance)

Release builds publish `missnix-vX.Y.Z-linux-x86_64.AppImage`, `missnix-vX.Y.Z-linux-x86_64.tar.gz`, and `SHA256SUMS` with GitHub Artifact Attestations:

```bash
sha256sum -c SHA256SUMS
gh attestation verify missnix-v*-linux-x86_64.AppImage --repo Irgendeinwer/MissNix
```

---

## 4. Core Engineering Invariants

### SQLite Schema & Data Preservation (`crates/missnix-core/src/db/`)
- **Never wipe user data on schema upgrades.** Increment `SCHEMA_VERSION` in `db/schema.rs` and apply explicit, transactional additive migrations while preserving `library_folders`, artists, releases, tracks, and settings. Refuse unknown or newer schemas instead of guessing or rebuilding them in place.

### Artist Discovery & Multi-Artist Tag Hygiene (`normalize.rs` & `scanner/`)
- Default discovery mode is `ArtistDiscoveryMode::AlbumArtistOrFolder`: prefer `ALBUMARTIST` and structured `<Artist>/<Album>/<Track>` directories over track `ARTIST` so featured guests do not pollute the artist list.
- `extract_primary_artist` splits collaboration delimiters (`;`, `\`, ` / `, `feat.`, `, `) while preserving legitimate comma/slash artist names (`Tyler, The Creator`, `Black Country, New Road`, `Earth, Wind & Fire`, `AC/DC`, `+44`).
- Only attach embedded MusicBrainz Artist IDs when the embedded tag matches the resolved primary artist (`safe_tag_mbid`).
- Apply full filesystem scans only with a matching `LibraryScanSnapshot`; incomplete, stale, cancelled, or unexpectedly empty scans must not replace the last good library. Retire missing local artists without deleting their release history, and restore them if they reappear. Keep manual artists and muted/ignored preferences.

### Slint 1.19 Desktop UI, Theme & Scaling Rules (`ui/` & `src/controller/`)
- **Custom themed controls (no unthemed `std-widgets` chrome)**: Slint 1.19's `std-widgets` `Palette` properties are read-only `out` properties, and its `material` `ScrollBar` draws a hollow `1px` border rectangle with no fill. Always use `M3Button`, `M3IconButton`, `M3FilterChip`, `M3SearchField`, `M3LabeledInput`, `M3Switch`, `M3Select`, `M3ProgressBar`, `M3Spinner`, and `M3ScrollBar` (with `vertical-scrollbar-policy: ScrollBarPolicy.always-off` on `ScrollView` / `ListView`), driven by the global `Theme` singleton (`crates/missnix-core/src/palette.rs`).
- **Relative `@image-url` resolution**: Slint resolves `@image-url("...")` relative to the `.slint` file containing the macro call (`../../assets/...` inside `ui/components/*.slint` and `ui/views/*.slint`).
- **Slint dimension constraint rule**: Never set both fixed `height` and `min-height`/`max-height` without a dynamic `max(...)` expression where appropriate, and avoid unbounded stretching on buttons or inputs (`vertical-stretch: 0`).
- **Explicit `x: 0px` on non-layout progress fills**: Any child `Rectangle` inside a plain `Rectangle` whose `width` varies (seek bars, volume bars, progress bars) **must** specify `x: 0px;` so it anchors to the left rather than centering.
- **Startup & Wayland HiDPI UI scaling (`src/controller/mod.rs`)**: `WinitWindowAdapter::ensure_window()` and Wayland's initial post-map `ScaleFactorChanged` / `fractional_scale` events reset `window().scale_factor()` after `AppWindow::new()`. Keep `applied_scale_factor` synchronized once `window().size().width > 0` so persisted `ui_scale_percent` takes effect immediately on launch and across monitor DPI changes.
- **Responsive shell**: Below 980 logical pixels use the compact rail; below 1348 logical pixels overlay the inspector without shrinking the center. Keep task status and Cancel available in the expanded rail, preserve the player and scrollable content in short windows, and allow panning rather than clipping controls at exceptionally narrow sizes.
- **Viewport culling & non-blocking UI**: Keep row culling active in `feed-scroll` (`ui/views/radar-view.slint`), decode cover thumbnails at bounded resolution (`300px`), and run all I/O, scanning, HTTP requests, image decoding, and audio playback on background threads.

### Audio Preview Engine (`src/audio_player/` & `sync.rs`)
- Deezer `preview_url` CDN query strings contain expiring `hdnea=exp=<unix_ts>` tokens. Always check `is_deezer_preview_url_expired` and refresh track URLs via `refresh_release_tracks` on expiration or HTTP 401/403 before failing playback.
- Runtime audio libraries (`libpulse-simple.so.0`, `libasound.so.2`) load dynamically via `libloading` with subprocess fallback (`pw-cat` / `paplay` / `aplay` / `ffplay`); `flake.nix` wraps `libpulseaudio` and `alsa-lib` into `LD_LIBRARY_PATH`.

---

## 5. Verification & Release Workflow

Run all checks locally (inside `nix develop` or a Rust 2024 toolchain environment):

```bash
{
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  nix flake check
} 2>&1 | tee log.txt
```

The responsive Slint interaction tests also run in CI. On Linux, with Node.js 22.18+ and Xvfb installed:

```bash
npm ci --prefix tests/ui --ignore-scripts
xvfb-run -a -s "-screen 0 1920x1200x24" \
  env WINIT_UNIX_BACKEND=x11 SLINT_BACKEND=winit-skia-software npm test --prefix tests/ui
```

These tests dispatch resize, scale, pointer, and key events to the UI; compositor-specific HiDPI behavior still needs testing on a real desktop.

### Publishing an Immutable Release

1. Bump `version` under `[workspace.package]` in root `Cargo.toml` (`crates/missnix-core/Cargo.toml` and `flake.nix` read it automatically) and update `Cargo.lock`.
2. Commit to `main` with `release: vX.Y.Z` (or `chore(release): vX.Y.Z`) and push — `.github/workflows/release.yml` runs tests, builds `missnix-vX.Y.Z-linux-x86_64.AppImage` + `missnix-vX.Y.Z-linux-x86_64.tar.gz` + `SHA256SUMS`, signs SLSA build provenance, creates tag `vX.Y.Z`, and publishes the immutable GitHub Release.
