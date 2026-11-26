use std::path::PathBuf;
use std::process::Command;

/// Open a native desktop folder selection dialog without pulling GTK3 C build dependencies.
/// Runs on a worker thread and tries desktop-native tools (`zenity`, `kdialog`, `yad`,
/// `osascript` on macOS, or PowerShell on Windows). Returns `Ok(Some(path))` if selected,
/// `Ok(None)` if cancelled, or `Err(reason)` if no dialog helper is installed.
pub fn pick_music_folder() -> Result<Option<PathBuf>, String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"Select Music Library Folder\")",
            ])
            .output()
            .map_err(|e| format!("Failed to invoke macOS folder chooser: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }

    #[cfg(target_os = "windows")]
    {
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
                      $f = New-Object System.Windows.Forms.FolderBrowserDialog; \
                      $f.Description = 'Select Music Library Folder'; \
                      if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }";
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", script])
            .output()
            .map_err(|e| format!("Failed to invoke Windows folder chooser: {e}"))?;
        if !out.status.success() {
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let candidates: &[(&str, &[&str])] = &[
            (
                "zenity",
                &[
                    "--file-selection",
                    "--directory",
                    "--title=Select Music Library Folder",
                ],
            ),
            (
                "kdialog",
                &[
                    "--getexistingdirectory",
                    ".",
                    "--title",
                    "Select Music Library Folder",
                ],
            ),
            (
                "yad",
                &[
                    "--file",
                    "--directory",
                    "--title=Select Music Library Folder",
                ],
            ),
        ];

        for (bin, args) in candidates {
            if let Ok(out) = Command::new(bin).args(*args).output() {
                if out.status.success() {
                    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !chosen.is_empty() {
                        return Ok(Some(PathBuf::from(chosen)));
                    }
                }
                return Ok(None);
            }
        }

        Err(
            "No desktop dialog helper (zenity / kdialog) found; type or paste your folder path directly."
                .to_string(),
        )
    }
}
