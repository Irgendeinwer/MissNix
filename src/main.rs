mod audio_player;
mod controller;
mod folder_picker;

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use missnix_core::{
    AppPaths, Database, HeadlessReport, ProgressEvent, SyncEngine,
    scan_library_folders_with_options,
};
use slint::ComponentHandle;

slint::include_modules!();

#[derive(Default)]
struct HeadlessOptions {
    scan_dirs: Vec<String>,
    accept_empty_folders: bool,
    run_check: bool,
    print_status: bool,
    json_output: bool,
    notify: bool,
}

impl HeadlessOptions {
    fn is_active(&self) -> bool {
        !self.scan_dirs.is_empty() || self.run_check || self.print_status || self.json_output
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut paths = AppPaths::resolve();
    let mut headless = HeadlessOptions::default();
    let mut cli_scale_override: Option<u16> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{}", help_text());
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("missnix {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--db" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    paths.set_database_path(PathBuf::from(p));
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
                    headless.scan_dirs.push(folder.clone());
                } else {
                    eprintln!("Error: --scan requires a folder path");
                    return ExitCode::FAILURE;
                }
            }
            "--check" => {
                headless.run_check = true;
            }
            "--accept-empty-folders" => {
                headless.accept_empty_folders = true;
            }
            "--status" => {
                headless.print_status = true;
            }
            "--json" => {
                headless.json_output = true;
            }
            "--notify" => {
                headless.notify = true;
            }
            unknown => {
                eprintln!("Unknown argument: {unknown}\n\n{}", help_text());
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    if headless.accept_empty_folders && headless.scan_dirs.is_empty() {
        eprintln!("Error: --accept-empty-folders requires --scan");
        return ExitCode::FAILURE;
    }
    if headless.notify && !headless.run_check {
        eprintln!("Error: --notify requires --check");
        return ExitCode::FAILURE;
    }

    // `--json` alone defaults to emitting `--status` as JSON.
    if headless.json_output && headless.scan_dirs.is_empty() && !headless.run_check {
        headless.print_status = true;
    }

    // Headless CLI mode when `--scan`, `--check`, `--status`, or `--json` is requested
    if headless.is_active() {
        if let Err(err) = run_headless_cli(paths, &headless) {
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

fn run_headless_cli(paths: AppPaths, opts: &HeadlessOptions) -> Result<(), String> {
    let mut db = Database::open(&paths.db_path).map_err(|e| e.to_string())?;
    let settings = db.load_settings().map_err(|e| e.to_string())?;
    let mut report = HeadlessReport {
        db_path: paths.db_path.to_string_lossy().to_string(),
        ..HeadlessReport::default()
    };

    for raw in &opts.scan_dirs {
        db.add_library_folder(raw).map_err(|e| e.to_string())?;
    }

    if !opts.scan_dirs.is_empty() {
        let snapshot = db.prepare_library_scan().map_err(|e| e.to_string())?;
        if !opts.json_output {
            println!("Scanning {} library folder(s)...", snapshot.folders.len());
        }
        let outcome = scan_library_folders_with_options(
            &snapshot.folders,
            &snapshot.options,
            None,
            |_, _| {},
        )
        .map_err(|e| e.to_string())?;
        db.apply_scan_outcome_checked(&outcome, &snapshot, opts.accept_empty_folders, None)
            .map_err(|e| e.to_string())?;
        if !opts.json_output {
            println!(
                "Scan complete: {} audio files -> {} artists, {} local albums indexed.",
                outcome.summary.audio_files_seen,
                outcome.summary.artists_discovered,
                outcome.summary.local_albums_indexed
            );
        }
        report.scan = Some(outcome.summary);
    }

    let mut sync_failure = None;
    if opts.run_check {
        let tracked = db.list_tracked_artists().map_err(|e| e.to_string())?;
        if !opts.json_output {
            println!(
                "Checking releases for {} tracked artist(s)...",
                tracked.len()
            );
        }
        let engine = SyncEngine::new(paths.cover_cache_dir);
        let json_mode = opts.json_output;
        let summary = engine
            .sync_artists(&mut db, &tracked, &settings, None, |ev| {
                if !json_mode
                    && let ProgressEvent::SyncArtist {
                        current,
                        total,
                        artist_name,
                    } = ev
                {
                    println!("  [{current}/{total}] {artist_name}");
                }
            })
            .map_err(|e| e.to_string())?;
        let not_attempted = tracked
            .len()
            .saturating_sub(summary.artists_checked + summary.artists_failed);
        if !opts.json_output {
            println!(
                "Release check: {} checked ({} partial), {} failed, {} not checked; {} new releases ({} owned locally).",
                summary.artists_checked,
                summary.artists_partial,
                summary.artists_failed,
                not_attempted,
                summary.new_releases_added,
                summary.owned_deduplicated
            );
        }
        if summary.artists_failed > 0 || summary.artists_partial > 0 || not_attempted > 0 {
            sync_failure = Some(format!(
                "Release check was incomplete: {}",
                summary
                    .last_error
                    .as_deref()
                    .unwrap_or("some artists were not checked")
            ));
        }
        let new_added = summary.new_releases_added;
        report.check = Some(summary);

        if opts.notify && new_added > 0 {
            let stats = db.library_stats().map_err(|e| e.to_string())?;
            send_release_notification(new_added, stats.unseen_releases);
            if opts.print_status {
                report.status = Some(stats);
            }
        }
    }

    if opts.print_status {
        let stats = match report.status.clone() {
            Some(existing) => existing,
            None => db.library_stats().map_err(|e| e.to_string())?,
        };
        if !opts.json_output {
            println!("MissNix Database: {}", paths.db_path.display());
            println!(
                "Artists: {} total ({} tracked, {} muted, {} ignored, {} not in library)",
                stats.total_artists,
                stats.tracked_artists,
                stats.muted_artists,
                stats.ignored_artists,
                stats.missing_artists
            );
            println!("Local Albums Indexed: {}", stats.local_albums);
            println!(
                "Discovered Releases:  {} total ({} unseen new drops)",
                stats.total_releases, stats.unseen_releases
            );
        }
        report.status = Some(stats);
    }

    if opts.json_output {
        println!("{}", report.to_json_pretty()?);
    }
    if let Some(error) = sync_failure {
        return Err(error);
    }
    Ok(())
}

fn send_release_notification(new_added: usize, unseen_total: usize) {
    let noun = if new_added == 1 {
        "new release"
    } else {
        "new releases"
    };
    let body = format!("{new_added} {noun} discovered ({unseen_total} unseen total)");
    match Command::new("notify-send")
        .args(["-a", "MissNix", "-i", "missnix", "MissNix Release Radar"])
        .arg(&body)
        // A notification helper must not contaminate structured CLI output.
        .stdout(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("Warning: notify-send exited with status {status}");
        }
        Err(err) => {
            eprintln!("Warning: failed to run notify-send: {err}");
        }
    }
}

fn help_text() -> String {
    format!(
        "MissNix {} — A desktop release radar for your local music library\n\n\
         USAGE:\n  \
           missnix [OPTIONS]\n\n\
         OPTIONS:\n  \
           --db <FILE>       Override SQLite database file path\n  \
           --scale <PCT>     Set HiDPI UI scale percentage (75..250, e.g. 125 or 150)\n  \
           --scan <FOLDER>   Add & scan a local music folder headlessly\n  \
           --accept-empty-folders  Confirm previously indexed folders are intentionally empty\n  \
           --check           Check Deezer + MusicBrainz for new releases headlessly\n  \
           --notify          Send a desktop notification when --check finds new drops\n  \
           --status          Print tracked artist & unread release counts\n  \
           --json            Emit structured JSON output in headless mode\n  \
           -h, --help        Print help information\n  \
           -V, --version     Print version",
        env!("CARGO_PKG_VERSION")
    )
}
