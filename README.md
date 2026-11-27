<div align="center">

<img src="assets/docs/banner.svg" alt="MissNix: Local Music Release Radar" width="100%" />

<br />

[![CI](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml/badge.svg)](https://github.com/Irgendeinwer/MissNix/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Irgendeinwer/MissNix?include_prereleases&label=release&color=ea6c34)](https://github.com/Irgendeinwer/MissNix/releases)
[![Nix Flake](https://img.shields.io/badge/Nix-Flake-5277C3?logo=nixos&logoColor=white&labelColor=24345c)](flake.nix)
[![Linux](https://img.shields.io/badge/Linux-AppImage%20%7C%20tar.gz-ea6c34?logo=linux&logoColor=white)](https://github.com/Irgendeinwer/MissNix/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-d97706)](LICENSE)

</div>

**MissNix** scans your local music folders, builds a list of the artists in your library, and checks **Deezer** and **MusicBrainz** for new and upcoming albums, EPs, and singles. No accounts, subscriptions, or API keys required.

<div align="center">
  <img src="assets/docs/ui-preview.svg" alt="MissNix Desktop Interface in Sunset Copper Theme" width="100%" />
</div>

<br />

<div align="center">
  <img src="assets/docs/workflow-cards.svg" alt="How MissNix Works" width="100%" />
</div>

---

## Features

- **Local folder sync:** Watches your music directories (`FLAC`, `MP3`, `Opus`, `Ogg`, `M4A`, `WAV`, `AIFF`, `WavPack`), keeps your artist list up to date, and hides albums you already own on disk.
- **Artist tag cleanup:** Splits collaboration credits into individual artists without breaking names like `Tyler, The Creator`, `Black Country, New Road`, or `AC/DC`. You can also mute or hide any artist in one click.
- **Release radar:** Shows new, unread, and upcoming releases in a cover grid with search and filters for albums, EPs, and singles.
- **30-second track previews:** Play track previews from the release drawer or docked mini-player without leaving the app.
- **Themes and scaling:** Switch between dark and light modes, 7 built-in color palettes (including Copper), custom `#RRGGBB` accent colors, and adjustable UI scaling.
- **Headless CLI:** Run scans, check for new drops, emit JSON status output, and send desktop notifications from a terminal or systemd timer.

---

## Install & Run

### Prebuilt Linux Downloads

Grab the latest `.AppImage` or `.tar.gz` archive from [**Releases**](https://github.com/Irgendeinwer/MissNix/releases):

```bash
chmod +x missnix-v*-linux-x86_64.AppImage
./missnix-v*-linux-x86_64.AppImage
```

### Nix / NixOS

Run directly without installing:

```bash
nix run github:Irgendeinwer/MissNix
```

Or add MissNix to your `flake.nix`:

```nix
{
  inputs.missnix = {
    url = "github:Irgendeinwer/MissNix";
    inputs.nixpkgs.follows = "nixpkgs";
  };
}
```

Then install it in your **NixOS** configuration via `environment.systemPackages` or the NixOS module:

```nix
{ inputs, pkgs, ... }:
{
  environment.systemPackages = [
    inputs.missnix.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];

  # Or using the NixOS module instead:
  # imports = [ inputs.missnix.nixosModules.default ];
  # programs.missnix.enable = true;
}
```

Or in **Home Manager** (with optional scheduled checks and desktop notifications):

```nix
{ inputs, ... }:
{
  imports = [ inputs.missnix.homeManagerModules.default ];

  programs.missnix = {
    enable = true;
    musicDirs = [ "/home/alice/Music" ];
    defaultAccentHex = "#E9B143";
    timer.enable = true;
  };
}
```

### Build from Source

```bash
cargo run --release
```

---

## Command-Line Options

Running `missnix` with no flags launches the desktop app. Pass flags to run headlessly or override startup settings:

```text
missnix [OPTIONS]

  --scan <FOLDER>   Add and scan a local music folder
  --accept-empty-folders  Confirm previously indexed folders are intentionally empty (with --scan)
  --check           Check tracked artists for new releases
  --notify          Send a desktop notification when --check finds new drops
  --status          Print tracked artist and unread release counts
  --json            Emit structured JSON output in headless mode
  --scale <PCT>     Set interface scale percentage (75..250, e.g. 125 or 150)
  --db <FILE>       Use a custom database file path
  -h, --help        Show help
  -V, --version     Show version
```

### Data safety and incomplete checks

- A scan fails without replacing the previous library if a configured folder is missing,
  unreadable, or could not be traversed. If a previously indexed folder suddenly has no
  audio files, the scan is also stopped: reconnect the drive and scan again. If the folder
  really was emptied, remove and re-add it in the app, or use `--accept-empty-folders`
  together with `--scan` in the CLI. This confirmation applies to all empty configured
  folders in that scan.
- Removing music folders or rescanning no longer deletes the corresponding artist's
  release history and user state. Artists no longer found locally are marked **Not in
  library**; they resume tracking if found again, or can be manually tracked from their
  artist details. Manually added artists remain tracked when local files disappear.
- Unknown, incompatible, or newer database schemas are rejected without an automatic
  rebuild. Keep the original database and any SQLite `-wal`/`-shm` files together; use a
  verified backup or an explicit migration rather than opening unrelated files as MissNix
  databases. Existing incorrect provider IDs from earlier scans can be corrected in an
  artist's details. Same-name search results are kept separate, but the database currently
  supports only one artist per normalized name; distinct homonyms cannot yet both be tracked.
- A partial or failed provider check is shown as a warning; `--check` exits non-zero if
  any artist was not fully checked. Successfully fetched releases are retained. The
  **Mark Filtered Seen** action only changes releases matching the current radar filters.
- Cover, avatar and preview caches are namespaced by database path. Older shared-cache
  files are left on disk but are not reused, avoiding cross-database mix-ups.

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

Architecture notes, module layout, build checks, and contributor guidelines are in [`AGENTS.md`](AGENTS.md).

## License

Released under the [MIT License](LICENSE).
