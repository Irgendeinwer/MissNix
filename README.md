# MissNix

**MissNix** is a standalone desktop **Local Music Release Tracker** written in **Rust** with a **Slint** Material interface and **SQLite (`rusqlite`)** persistence. Point it at your local music folders and MissNix builds an artist roster from your audio files, checks **Deezer** and **MusicBrainz** for new albums, EPs, and singles, deduplicates against albums you already own locally, and surfaces genuinely new drops in a cover-art-driven desktop feed.

---

## Key Capabilities

- **Local Music Artist Discovery (`lofty` + folder fallback)**
  - Recursively scans FLAC, MP3, Ogg Vorbis, Opus, M4A/MP4, WAV, AIFF, AAC, APE, and WavPack files.
  - Prioritizes `ALBUMARTIST` / `MusicBrainz Album Artist Id` over track `ARTIST` to prevent guest-feature pollution, automatically skipping `Various Artists` / soundtracks unless opted in.
  - Falls back cleanly to `<Artist>/<Year - Album>/<Track>` folder conventions when embedded tags are missing.
- **Persistent SQLite State (`rusqlite`)**
  - Stores library folders, discovered & manual artists, indexed local albums, remote releases, cached tracklists, and preferences in `$XDG_DATA_HOME/missnix/missnix.db` (WAL mode).
  - Remembers `unseen`, `seen`, and `dismissed` release states as well as `tracked`, `muted`, and `ignored` artist states across restarts and rescans.
- **Dual-Provider Release Radar (Deezer + MusicBrainz)**
  - **Deezer API** provides fast artist search, discography lookups, high-resolution cover artwork, explicit badges, tracklists, and 30-second audio preview links with zero API keys.
  - **MusicBrainz API + Cover Art Archive** resolves canonical MBIDs (including embedded `MusicBrainz Artist Id` tags from Picard/beets), extracts linked Deezer artist URLs, and cross-checks release groups so indie/Bandcamp-first releases aren't missed.
  - Disambiguates artist homonyms by scoring candidate discographies against the albums you already own locally, collapses duplicate Deluxe/Remastered/Regional editions, and flags locally owned albums automatically.
- **Responsive Slint Material Desktop UI**
  - Built with Slint's Material component library (`material-dark` / `material-light` runtime theme switching).
  - Multi-pane layout: left navigation rail with live background scan/sync progress & cancellation, responsive cover-art grid (**New Drops** & **All Releases**), right-hand **Release Inspector** with tracklist and external links, **Tracked Artists** master-detail manager with provider ID overrides & online artist search, and **Library & Settings**.
  - All filesystem scanning, HTTP provider lookups, and cover-art thumbnail decoding run on background worker threads.

---

## Workspace Layout

```text
MissNix/
├── flake.nix                         # Nix package, app, overlay, check, and devShell
├── Cargo.toml                        # Workspace + Slint desktop binary crate (`missnix`)
├── Cargo.lock                        # Pinned dependency lockfile
├── build.rs                          # Compiles Slint UI with Material style
├── src/
│   ├── main.rs                       # Entrypoint (Slint GUI + optional headless CLI flags)
│   ├── controller.rs                 # Non-blocking UI <-> missnix-core reactive bridge
│   └── folder_picker.rs              # Native desktop folder selection helper
├── ui/
│   ├── models.slint                  # Shared UI structs
│   ├── components.slint              # Reusable Material cards, badges, and list rows
│   └── app-window.slint              # 3-pane desktop application window
└── crates/
    └── missnix-core/                 # Pure-Rust GUI-independent core library
        └── src/
            ├── db.rs                 # SQLite schema, migrations, queries & deduplication
            ├── models.rs             # Domain types & settings
            ├── normalize.rs          # Artist/edition normalization & UTC date helpers
            ├── scanner.rs            # Audio tag + directory structure scanner
            ├── sync.rs               # Multi-provider sync engine & cover cache
            └── providers/
                ├── deezer.rs         # Deezer REST client & homonym scorer
                └── musicbrainz.rs    # MusicBrainz WS/2 client (1.1s rate-limited)
```

---

## Building & Running with Nix

Run directly with Flakes enabled:

```bash
nix run .
```

Build the standalone package (includes desktop entry, scalable icon, and Wayland/X11/OpenGL runtime wrapper):

```bash
nix build
./result/bin/missnix
```

Enter a development shell with `rustc`, `cargo`, `clippy`, `rustfmt`, `rust-analyzer`, `slint-lsp`, and GUI libraries configured:

```bash
nix develop
cargo test --workspace
cargo run
```

---

## Headless CLI Mode (Optional)

While MissNix launches its desktop GUI by default, it also supports headless flags for scripting or `systemd --user` timers:

```bash
# Add & scan a music directory, check for new releases, and print summary counts
missnix --scan ~/Music --check --status

# Use a custom SQLite database path
missnix --db /path/to/custom.db
```
