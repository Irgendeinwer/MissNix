<div align="center">

<img src="assets/docs/banner.svg" alt="MissNix — Local Music Release Radar" width="100%" />

<br />

[![CI](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml/badge.svg)](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Irgendeinwer/MissNix?include_prereleases&label=release&color=7c66b8)](https://github.com/Irgendeinwer/MissNix/releases)
[![Nix Flake](https://img.shields.io/badge/Nix-Flake-5277C3?logo=nixos&logoColor=white)](flake.nix)
[![Rust](https://img.shields.io/badge/Rust-2021-d0bcff?logo=rust&logoColor=white)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-34d399)](LICENSE)

</div>

**MissNix** scans your local music folders, builds a clean list of the artists you listen to, and checks **Deezer** and **MusicBrainz** for new and upcoming albums, EPs, and singles — hiding releases you already own locally and playing 30-second previews directly in the app.

<div align="center">
  <img src="assets/docs/ui-preview.svg" alt="MissNix Desktop Interface" width="100%" />
</div>

---

## Features

- **Clean Local Artist Discovery (`lofty` + folder hierarchy)**
  - Scans FLAC, MP3, Ogg Vorbis, Opus, M4A/MP4, WAV, AIFF, AAC, APE, and WavPack files.
  - Configurable discovery source: **Album Artist + Folder** (default), **Folder Structure Only**, or **All Metadata Tags + Folder**.
  - Splits multi-artist collaboration tags (`;`, `/`, `feat.`, `, `) into primary artists while keeping real comma/slash band names (`Tyler, The Creator`, `Black Country, New Road`, `Earth, Wind & Fire`, `AC/DC`) intact.
- **Dual-Source Release Lookup & Deduplication**
  - Queries **Deezer** (artwork, tracklists, 30s previews, explicit flags) and **MusicBrainz** (canonical MBIDs, Cover Art Archive, upcoming release groups) with no API keys required.
  - Scores artist homonyms against your local albums, collapses duplicate Deluxe/Remastered editions, and suppresses albums already present on disk.
- **In-App 30-Second Audio Preview Player**
  - Pure-Rust MP3 decoding (`nanomp3`) with local preview caching, automatic refresh of expired Deezer CDN tokens, and native audio output via PulseAudio / PipeWire (`libpulse-simple`), ALSA (`libasound`), or `pw-cat` / `paplay` fallback.
- **Material 3 Desktop Interface (`Slint`)**
  - Unified **Release Radar** with status (`All`, `Unread`, `Upcoming`) and type (`Albums`, `EPs`, `Singles`) filters, viewport-culled cover grid, and one-click **Mute** / **Hide** artist controls right inside the sidebar.
  - Built-in **HiDPI UI Scaling** (`90%`–`200%`, `Ctrl +` / `Ctrl -` / `Ctrl 0`) for high-resolution displays without compositor scaling, Dark/Light modes, and 6 accent palettes.
- **Headless CLI Mode**
  - Run `--scan`, `--check`, and `--status` from scripts or a `systemd --user` timer against the same SQLite database (`$XDG_DATA_HOME/missnix/missnix.db`).

---

## Installation & Usage

### Nix Flake (NixOS / Linux / macOS)

Run directly without installing:

```bash
nix run github:Irgendeinwer/MissNix
```

Build from a local checkout:

```bash
nix build
./result/bin/missnix
```

Add to a NixOS or Home Manager flake configuration:

```nix
{
  inputs.missnix.url = "github:Irgendeinwer/MissNix";

  # In NixOS / Home Manager modules:
  environment.systemPackages = [ inputs.missnix.packages.${pkgs.system}.default ];
}
```

### Prebuilt Release Archives

Download the latest archive from [**Releases**](https://github.com/Irgendeinwer/MissNix/releases) and optionally verify its cryptographic SLSA build provenance with the GitHub CLI:

```bash
gh attestation verify missnix-v*-x86_64-unknown-linux-gnu.tar.gz --repo Irgendeinwer/MissNix
sha256sum -c SHA256SUMS
```

### Cargo (From Source)

Requires Rust 1.85+ and standard Wayland/X11/OpenGL development headers (or `nix develop`):

```bash
cargo run --release
```

---

## Headless CLI & `systemd` Timer

```text
USAGE:
  missnix [OPTIONS]

OPTIONS:
  --db <FILE>       Override SQLite database file path
  --scale <PCT>     Set HiDPI UI scale percentage (75..250, e.g. 125 or 150)
  --scan <FOLDER>   Add & scan a local music folder headlessly
  --check           Check Deezer + MusicBrainz for new releases headlessly
  --status          Print tracked artist & unread release counts
  -h, --help        Print help information
  -V, --version     Print version
```

Example background check via `systemd --user`:

```ini
# ~/.config/systemd/user/missnix-check.service
[Unit]
Description=Check local music artists for new releases (MissNix)

[Service]
Type=oneshot
ExecStart=%h/.nix-profile/bin/missnix --check --status
```

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `/` | Focus search bar in Release Radar or Tracked Artists |
| `Space` | Play / pause active 30-second track preview |
| `Esc` | Close right inspector drawer or Add Artist panel |
| `Ctrl + R` | Trigger release check across tracked artists |
| `Ctrl + =` / `Ctrl + -` | Increase / decrease HiDPI interface scale |
| `Ctrl + 0` | Reset interface scale to `100%` |

---

## Data & Cache Locations

| Path | Purpose |
| :--- | :--- |
| `$XDG_DATA_HOME/missnix/missnix.db` | SQLite WAL database (artists, local albums, releases, settings) |
| `$XDG_CACHE_HOME/missnix/covers/` | Cached release cover artwork |
| `$XDG_CACHE_HOME/missnix/avatars/` | Cached artist portraits |
| `$XDG_CACHE_HOME/missnix/previews/` | Cached 30-second MP3 track previews |

---

## Creating a Release

Releases are built, attested (SLSA provenance + `SHA256SUMS`), and published automatically by [`.github/workflows/release.yml`](.github/workflows/release.yml). Published tags are **immutable** (existing releases are never overwritten; enable *Settings → General → Releases → Enable release immutability* in GitHub for server-side enforcement).

To cut a new release (e.g. `v0.2.0`):

1. Update `version = "0.2.0"` in both `Cargo.toml` and `crates/missnix-core/Cargo.toml` (`flake.nix` reads `Cargo.toml` automatically) and run `cargo check` so `Cargo.lock` updates.
2. Commit with a `release: vX.Y.Z` subject on `main` and push:
   ```bash
   git commit -am "release: v0.2.0"
   git push origin main
   ```
   *(Alternatively, push a `v0.2.0` git tag directly or trigger the **Release** workflow manually via `workflow_dispatch` in GitHub Actions.)*

---

## License

Released under the [MIT License](LICENSE).
