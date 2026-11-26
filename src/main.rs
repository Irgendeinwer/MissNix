mod audio_player;
mod controller;
mod folder_picker;

use std::path::PathBuf;
use std::process::ExitCode;

use missnix_core::{
    AppPaths, Database, ProgressEvent, ScanOptions, SyncEngine, scan_library_folders_with_options,
};
use slint::ComponentHandle;

slint::include_modules!();

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths = AppPaths::resolve();
    let mut scan_folders_cli: Vec<String> = Vec::new();
    let mut run_check_cli = false;
    let mut print_status_cli = false;
    let mut cli_scale_override: Option<u16> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("missnix {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--db" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    paths.db_path = PathBuf::from(p);
                } else {
                    eprintln!("Error: --db requires a file path");
                    return ExitCode::FAILURE;
                }
            }
            "--scale" => {
                i += 1;
                let Some(raw) = args.get(i) else {
                    eprintln!("Error: --scale requires a percentage (75..250, e.g. 125 or 150)");
                    return ExitCode::FAILURE;
                };
                match raw.trim_end_matches('%').parse::<u16>() {
                    Ok(pct) if (75..=250).contains(&pct) => {
                        cli_scale_override = Some(pct);
                    }
                    _ => {
                        eprintln!("Error: --scale must be between 75 and 250 (got '{raw}')");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--scan" => {
                i += 1;
                if let Some(folder) = args.get(i) {
                    scan_folders_cli.push(folder.clone());
                } else {
                    eprintln!("Error: --scan requires a folder path");
                    return ExitCode::FAILURE;
                }
            }
            "--check" => {
                run_check_cli = true;
            }
            "--status" => {
                print_status_cli = true;
            }
            unknown => {
                eprintln!("Unknown argument: {unknown}\n");
                print_help();
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    // Headless CLI mode when `--scan`, `--check`, or `--status` is requested
    if !scan_folders_cli.is_empty() || run_check_cli || print_status_cli {
        if let Err(err) =
            run_headless_cli(paths, &scan_folders_cli, run_check_cli, print_status_cli)
        {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    // Desktop GUI mode
    let app = match AppWindow::new() {
        Ok(win) => win,
        Err(err) => {
            eprintln!("Failed to initialize Slint GUI window: {err}");
            return ExitCode::FAILURE;
        }
    };

    let (_ui_state, _timers) = match controller::wire_controller(&app, paths, cli_scale_override) {
        Ok(handles) => handles,
        Err(err) => {
            eprintln!("Failed to initialize MissNix state: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = app.run() {
        eprintln!("GUI event loop exited with error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run_headless_cli(
    paths: AppPaths,
    scan_dirs: &[String],
    run_check: bool,
    print_status: bool,
) -> Result<(), String> {
    let mut db = Database::open(&paths.db_path).map_err(|e| e.to_string())?;
    let settings = db.load_settings().map_err(|e| e.to_string())?;

    for raw in scan_dirs {
        db.add_library_folder(raw).map_err(|e| e.to_string())?;
    }

    if !scan_dirs.is_empty() {
        let folders: Vec<PathBuf> = db
            .list_library_folders()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|f| f.enabled)
            .map(|f| PathBuf::from(f.path))
            .collect();

        println!("Scanning {} library folder(s)...", folders.len());
        let scan_opts = ScanOptions::from_settings(&settings);
        let outcome = scan_library_folders_with_options(&folders, &scan_opts, None, |_, _| {})
            .map_err(|e| e.to_string())?;
        db.apply_scan_outcome(&outcome).map_err(|e| e.to_string())?;
        println!(
            "Scan complete: {} audio files -> {} artists, {} local albums indexed.",
            outcome.summary.audio_files_seen,
            outcome.summary.artists_discovered,
            outcome.summary.local_albums_indexed
        );
    }

    if run_check {
        let tracked = db.list_tracked_artists().map_err(|e| e.to_string())?;
        println!(
            "Checking releases for {} tracked artist(s)...",
            tracked.len()
        );
        let engine = SyncEngine::new(paths.cover_cache_dir);
        let summary = engine
            .sync_artists(&mut db, &tracked, &settings, None, |ev| {
                if let ProgressEvent::SyncArtist {
                    current,
                    total,
                    artist_name,
                } = ev
                {
                    println!("  [{current}/{total}] {artist_name}");
                }
            })
            .map_err(|e| e.to_string())?;
        println!(
            "Release check complete: {} checked, {} new releases added ({} owned locally).",
            summary.artists_checked, summary.new_releases_added, summary.owned_deduplicated
        );
    }

    if print_status {
        let stats = db.library_stats().map_err(|e| e.to_string())?;
        println!("MissNix Database: {}", paths.db_path.display());
        println!(
            "Artists: {} total ({} tracked, {} muted, {} ignored)",
            stats.total_artists, stats.tracked_artists, stats.muted_artists, stats.ignored_artists
        );
        println!("Local Albums Indexed: {}", stats.local_albums);
        println!(
            "Discovered Releases:  {} total ({} unseen new drops)",
            stats.total_releases, stats.unseen_releases
        );
    }

    Ok(())
}

fn print_help() {
    println!(
        "MissNix {} — A desktop release radar for your local music library\n\n\
         USAGE:\n  \
           missnix [OPTIONS]\n\n\
         OPTIONS:\n  \
           --db <FILE>       Override SQLite database file path\n  \
           --scale <PCT>     Set HiDPI UI scale percentage (75..250, e.g. 125 or 150)\n  \
           --scan <FOLDER>   Add & scan a local music folder headlessly\n  \
           --check           Check Deezer + MusicBrainz for new releases headlessly\n  \
           --status          Print tracked artist & unread release counts\n  \
           -h, --help        Print help information\n  \
           -V, --version     Print version",
        env!("CARGO_PKG_VERSION")
    );
}
