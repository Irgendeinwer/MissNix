<div align="center">

<img src="assets/docs/banner.svg" alt="MissNix — Local Music Release Radar" width="100%" />

<br />

[![CI](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml/badge.svg)](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Irgendeinwer/MissNix?include_prereleases&label=release&color=7c66b8)](https://github.com/Irgendeinwer/MissNix/releases)
[![Nix Flake](https://img.shields.io/badge/Nix-Flake-5277C3?logo=nixos&logoColor=white)](flake.nix)
[![Rust](https://img.shields.io/badge/Rust-2024-d0bcff?logo=rust&logoColor=white)](Cargo.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-34d399)](LICENSE)

</div>

**A desktop release radar for your local music library — scan your folders, track favorite artists, and miss nothing.**

MissNix indexes the artists in your local audio collection and checks **Deezer** and **MusicBrainz** for new and upcoming albums, EPs, and singles — hiding releases you already own locally and playing 30-second previews directly in the app.

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

### Prebuilt Linux Binaries (AppImage & Tarball)

Download the portable **AppImage** or standalone `.tar.gz` archive from [**Releases**](https://github.com/Irgendeinwer/MissNix/releases):

```bash
# Run portable AppImage directly:
chmod +x missnix-v*-linux-x86_64.AppImage
./missnix-v*-linux-x86_64.AppImage

# Optional: verify checksums & SLSA build provenance
sha256sum -c SHA256SUMS
gh attestation verify missnix-v*-linux-x86_64.AppImage --repo Irgendeinwer/MissNix
```

### Cargo (From Source)

Requires Rust stable (Edition 2024) and standard Wayland/X11/OpenGL development headers (or `nix develop`):

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

## License

Released under the [MIT License](LICENSE).
