<div align="center">

<img src="assets/docs/banner.svg" alt="MissNix — Local Music Release Radar" width="100%" />

<br />

[![CI](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml/badge.svg)](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Irgendeinwer/MissNix?include_prereleases&label=release&color=ea6c34)](https://github.com/Irgendeinwer/MissNix/releases)
[![Nix Flake](https://img.shields.io/badge/Nix-Flake-ffb596?logo=nixos&logoColor=1b120e&labelColor=3c1e11)](flake.nix)
[![Linux](https://img.shields.io/badge/Linux-AppImage%20%7C%20tar.gz-ea6c34?logo=linux&logoColor=white)](https://github.com/Irgendeinwer/MissNix/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-d97706)](LICENSE)

</div>

**MissNix** watches your local music folders, builds a clean list of the artists you actually listen to, and checks **Deezer** and **MusicBrainz** for new and upcoming albums, EPs, and singles — without accounts, subscriptions, or API keys.

<div align="center">
  <img src="assets/docs/ui-preview.svg" alt="MissNix Desktop Interface in Sunset Copper Theme" width="100%" />
</div>

<br />

<div align="center">
  <img src="assets/docs/workflow-cards.svg" alt="How MissNix Works" width="100%" />
</div>

---

## Highlights

- **Follows Your Local Collection Automatically** — Point MissNix at your music directories (`FLAC`, `MP3`, `Opus`, `Ogg`, `M4A`, `WAV`, `AIFF`, `WavPack`) and it keeps your artist list in sync while automatically hiding albums you already own.
- **Smart Artist Hygiene** — Separates collaboration credits into primary artists without breaking real band names (`Tyler, The Creator`, `Black Country, New Road`, `AC/DC`), and lets you mute or hide any artist in one click.
- **Unified Release Radar** — Browse new, unread, and upcoming releases in a responsive cover grid with instant search and filters for **Albums**, **EPs**, and **Singles**.
- **Built-In 30-Second Track Previews** — Sample tracks directly from the release drawer or docked mini-player without leaving the app.
- **Personal Themes & Crisp HiDPI Scaling** — Choose between Dark and Light modes, 7 curated color themes (including **Copper**), any custom `#RRGGBB` accent color, and `90%`–`200%` interface scaling.
- **Optional Headless CLI** — Scan folders or check for new releases from a terminal or scheduled background timer.

---

## Install & Run

### Prebuilt Linux Downloads (AppImage & Archive)

Grab the latest portable **AppImage** or standalone `.tar.gz` bundle from [**Releases**](https://github.com/Irgendeinwer/MissNix/releases):

```bash
chmod +x missnix-v*-linux-x86_64.AppImage
./missnix-v*-linux-x86_64.AppImage
```

### Nix / NixOS

Run directly without installing:

```bash
nix run github:Irgendeinwer/MissNix
```

Or add MissNix to a NixOS / Home Manager flake:

```nix
{
  inputs.missnix.url = "github:Irgendeinwer/MissNix";

  # In your system or Home Manager packages:
  environment.systemPackages = [ inputs.missnix.packages.${pkgs.system}.default ];
}
```

### Build from Source

```bash
cargo run --release
```

---

## Command-Line Options

Running `missnix` with no flags opens the desktop app. Pass flags to run headlessly or override startup options:

```text
missnix [OPTIONS]

  --scan <FOLDER>   Add and scan a local music folder
  --check           Check tracked artists for new releases
  --status          Print tracked artist and unread release counts
  --scale <PCT>     Set interface scale percentage (75..250, e.g. 125 or 150)
  --db <FILE>       Use a custom database file path
  -h, --help        Show help
  -V, --version     Show version
```

---

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `/` | Focus the search bar in Release Radar or Tracked Artists |
| `Space` | Play or pause the active 30-second track preview |
| `Esc` | Close the right inspector drawer or Add Artist bar |
| `Ctrl + R` | Check all tracked artists for new releases |
| `Ctrl + =` / `Ctrl + -` | Increase or decrease interface scale |
| `Ctrl + 0` | Reset interface scale to `100%` |

---

## Contributing & Architecture

Technical architecture, module layout, build verification, and contributor guidelines live in [`AGENTS.md`](AGENTS.md).

## License

Released under the [MIT License](LICENSE).
