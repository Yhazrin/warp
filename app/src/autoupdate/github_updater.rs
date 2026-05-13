//! GitHub Releases-based auto-updater for the OSS channel.
//!
//! Checks `https://api.github.com/repos/{owner}/{repo}/releases/latest` for new versions,
//! downloads the platform-appropriate asset, and replaces the current executable.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use warp_core::channel::ChannelState;
use warpui::r#async::Timer;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const GITHUB_REPO_OWNER: &str = "Yhazrin";
const GITHUB_REPO_NAME: &str = "warp";
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Delay after app launch before the first update check.
const INITIAL_CHECK_DELAY: Duration = Duration::from_secs(5);

/// How often to re-check for updates while the app is running.
const POLL_INTERVAL: Duration = Duration::from_secs(3600);

const USER_AGENT: &str = "warp-oss-updater/1.0";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The current stage of the GitHub update lifecycle.
#[derive(Clone, Debug, PartialEq)]
pub enum GitHubUpdateStage {
    /// No check has been performed yet.
    Idle,
    /// Currently contacting GitHub to check for a new release.
    Checking,
    /// Checked and the current version is up to date.
    UpToDate {
        current_version: String,
    },
    /// A newer release was found and is being downloaded.
    Downloading {
        version: String,
        release_notes: String,
        progress: f32,
    },
    /// The update has been downloaded and is ready to be applied.
    UpdateReady {
        version: String,
        release_notes: String,
    },
    /// Applying the update (replacing files).
    Applying {
        version: String,
    },
    /// An error occurred during the update process.
    Error(String),
}

impl Default for GitHubUpdateStage {
    fn default() -> Self {
        GitHubUpdateStage::Idle
    }
}

// ---------------------------------------------------------------------------
// GitHub API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct GitHubUpdater {
    stage: GitHubUpdateStage,
    current_version: String,
    downloaded_path: Option<PathBuf>,
}

impl GitHubUpdater {
    pub fn new() -> Self {
        let current_version = ChannelState::app_version()
            .unwrap_or("unknown")
            .to_string();
        Self {
            stage: GitHubUpdateStage::Idle,
            current_version,
            downloaded_path: None,
        }
    }

    pub fn register(ctx: &mut AppContext) {
        ctx.add_singleton_model(move |ctx| {
            let updater = Self::new();

            // Delayed initial check after app launch.
            ctx.spawn(
                async move {
                    Timer::after(INITIAL_CHECK_DELAY).await;
                },
                |me, _, ctx| {
                    me.check_for_update(ctx);
                },
            );

            // Periodic polling loop.
            ctx.spawn(
                async move {
                    Timer::after(POLL_INTERVAL).await;
                },
                |me, _, ctx| {
                    me.poll_for_update(ctx);
                },
            );

            updater
        });
    }

    /// Recursive periodic polling loop.
    fn poll_for_update(&mut self, ctx: &mut ModelContext<Self>) {
        self.check_for_update(ctx);
        ctx.spawn(
            async move {
                Timer::after(POLL_INTERVAL).await;
            },
            |me, _, ctx| {
                me.poll_for_update(ctx);
            },
        );
    }

    // -- Public API ----------------------------------------------------------

    pub fn stage(&self) -> &GitHubUpdateStage {
        &self.stage
    }

    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// Triggered by the user from the Settings UI.
    pub fn manual_check(&mut self, ctx: &mut ModelContext<Self>) {
        self.check_for_update(ctx);
    }

    /// Triggered by the user to apply a downloaded update.
    pub fn apply_and_relaunch(&mut self, ctx: &mut ModelContext<Self>) {
        let GitHubUpdateStage::UpdateReady { version, .. } = &self.stage else {
            return;
        };
        let version = version.clone();
        self.stage = GitHubUpdateStage::Applying {
            version: version.clone(),
        };
        ctx.notify();

        ctx.spawn(
            async move { apply_update_to_disk().await },
            |updater, result, ctx| match result {
                Ok(()) => {
                    relaunch_app();
                }
                Err(e) => {
                    updater.stage =
                        GitHubUpdateStage::Error(format!("Failed to apply update: {}", e));
                    ctx.notify();
                }
            },
        );
    }

    // -- Internals -----------------------------------------------------------

    fn check_for_update(&mut self, ctx: &mut ModelContext<Self>) {
        self.stage = GitHubUpdateStage::Checking;
        ctx.notify();

        let current_version = self.current_version.clone();
        ctx.spawn(
            async move { fetch_and_compare(&current_version).await },
            |updater, result, ctx| match result {
                Ok(UpdateCheckResult::UpToDate) => {
                    updater.stage = GitHubUpdateStage::UpToDate {
                        current_version: updater.current_version.clone(),
                    };
                    ctx.notify();
                }
                Ok(UpdateCheckResult::NewVersion {
                    version,
                    release_notes,
                    asset_url,
                    ..
                }) => {
                    updater.stage = GitHubUpdateStage::Downloading {
                        version: version.clone(),
                        release_notes,
                        progress: 0.0,
                    };
                    ctx.notify();

                    // Start download.
                    ctx.spawn(
                        async move { download_asset(&asset_url).await },
                        |updater, result, ctx| match result {
                            Ok(path) => {
                                let stage = updater.stage.clone();
                                if let GitHubUpdateStage::Downloading {
                                    version, release_notes, ..
                                } = stage
                                {
                                    updater.downloaded_path = Some(path);
                                    updater.stage = GitHubUpdateStage::UpdateReady {
                                        version,
                                        release_notes,
                                    };
                                }
                                ctx.notify();
                            }
                            Err(e) => {
                                updater.stage =
                                    GitHubUpdateStage::Error(format!("Download failed: {}", e));
                                ctx.notify();
                            }
                        },
                    );
                }
                Err(e) => {
                    updater.stage =
                        GitHubUpdateStage::Error(format!("Update check failed: {}", e));
                    ctx.notify();
                }
            },
        );
    }
}

impl Entity for GitHubUpdater {
    type Event = ();
}

impl SingletonEntity for GitHubUpdater {}

// ---------------------------------------------------------------------------
// Update check logic
// ---------------------------------------------------------------------------

enum UpdateCheckResult {
    UpToDate,
    NewVersion {
        version: String,
        release_notes: String,
        asset_url: String,
        asset_size: u64,
    },
}

async fn fetch_and_compare(current_version: &str) -> Result<UpdateCheckResult> {
    let url = format!(
        "{}/repos/{}/{}/releases/latest",
        GITHUB_API_BASE, GITHUB_REPO_OWNER, GITHUB_REPO_NAME
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "GitHub API returned status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    let release: GitHubRelease = response.json().await?;
    let latest_version = strip_v_prefix(&release.tag_name);

    log::info!(
        "GitHub update check: current={}, latest={}",
        current_version,
        latest_version
    );

    if !is_newer(current_version, &latest_version) {
        return Ok(UpdateCheckResult::UpToDate);
    }

    // Find the appropriate asset for the current platform.
    let asset = find_platform_asset(&release.assets)?;
    let asset_url = asset.browser_download_url.clone();
    let asset_size = asset.size;

    log::info!(
        "New version available: {} (asset: {}, size: {} bytes)",
        latest_version,
        asset.name,
        asset_size
    );

    Ok(UpdateCheckResult::NewVersion {
        version: latest_version,
        release_notes: release.body.unwrap_or_default(),
        asset_url,
        asset_size,
    })
}

fn strip_v_prefix(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

/// Compare two semver strings (major.minor.patch). Returns true if `latest` is newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    let c = parse(current);
    let l = parse(latest);
    l > c
}

// ---------------------------------------------------------------------------
// Platform asset selection
// ---------------------------------------------------------------------------

fn expected_asset_name() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("windows", "x86_64") => "warp-oss-windows-x86_64.exe".to_string(),
        ("macos", "aarch64") => "warp-oss-macos-aarch64".to_string(),
        ("macos", "x86_64") => "warp-oss-macos-x86_64".to_string(),
        ("linux", "x86_64") => "warp-oss-linux-x86_64".to_string(),
        _ => format!("warp-oss-{}-{}", os, arch),
    }
}

fn find_platform_asset(assets: &[GitHubAsset]) -> Result<GitHubAsset> {
    let expected = expected_asset_name();

    // Try exact match first, then prefix match (for .tar.gz, .zip, etc.)
    assets
        .iter()
        .find(|a| a.name == expected || a.name.starts_with(&expected))
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "No release asset found for platform '{}'. Available: [{}]",
                expected,
                assets
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Download the asset to a temporary file. Returns the path to the downloaded file.
async fn download_asset(url: &str) -> Result<PathBuf> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let temp_dir = std::env::temp_dir();
    let file_name = format!("warp-update-{}", std::process::id());
    let dest = temp_dir.join(file_name);

    let bytes = response.bytes().await?;
    std::fs::write(&dest, &bytes)?;

    log::info!(
        "Downloaded {} bytes to {}",
        bytes.len(),
        dest.display()
    );

    Ok(dest)
}

// ---------------------------------------------------------------------------
// Apply update
// ---------------------------------------------------------------------------

async fn apply_update_to_disk() -> Result<()> {
    let current_exe = std::env::current_exe()?;
    let temp_dir = std::env::temp_dir();

    // Find the downloaded update file.
    let update_file = temp_dir
        .read_dir()?
        .filter_map(|e| e.ok())
        .find(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("warp-update-"))
        })
        .ok_or_else(|| anyhow!("No downloaded update file found"))?;

    let update_path = update_file.path();

    cfg_if::cfg_if! {
        if #[cfg(windows)] {
            // On Windows: rename current → .old, copy downloaded → current
            let old_path = current_exe.with_extension("exe.old");
            // Remove any leftover .old file from a previous update.
            let _ = std::fs::remove_file(&old_path);
            std::fs::rename(&current_exe, &old_path)?;
            std::fs::copy(&update_path, &current_exe)?;
            let _ = std::fs::remove_file(&update_path);
        } else if #[cfg(target_os = "macos")] {
            // On macOS: if the current exe is inside a .app bundle, replace it in place.
            // Otherwise, do a simple rename+copy.
            let old_path = current_exe.with_extension("old");
            let _ = std::fs::remove_file(&old_path);
            std::fs::rename(&current_exe, &old_path)?;
            std::fs::copy(&update_path, &current_exe)?;
            // Make the new binary executable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::remove_file(&update_path);
        } else {
            // Linux: same pattern as macOS.
            let old_path = current_exe.with_extension("old");
            let _ = std::fs::remove_file(&old_path);
            std::fs::rename(&current_exe, &old_path)?;
            std::fs::copy(&update_path, &current_exe)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::remove_file(&update_path);
        }
    }

    log::info!("Update applied successfully");
    Ok(())
}

/// Clean up leftover `.old`/`.exe.old` files from a previous update.
pub fn cleanup_old_executables() {
    if let Ok(current_exe) = std::env::current_exe() {
        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                let old = current_exe.with_extension("exe.old");
                let _ = std::fs::remove_file(old);
            } else {
                let old = current_exe.with_extension("old");
                let _ = std::fs::remove_file(old);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Relaunch
// ---------------------------------------------------------------------------

fn relaunch_app() {
    let Ok(current_exe) = std::env::current_exe() else {
        log::error!("Cannot relaunch: failed to get current exe path");
        return;
    };

    log::info!("Relaunching app: {}", current_exe.display());

    // Spawn a new instance of the app.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(&current_exe)
            .creation_flags(0x00000010) // CREATE_NEW_CONSOLE
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new(&current_exe).spawn();
    }

    // Exit the current instance.
    std::process::exit(0);
}
