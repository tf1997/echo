use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateTarget {
    Portable,
    Installer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePackage {
    pub target: UpdateTarget,
    pub platform: String,
    pub arch: String,
    pub url: String,
    pub sha256: String,
    pub signature: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub channel: Option<String>,
    pub notes: Option<String>,
    pub force: Option<bool>,
    pub min_supported_version: Option<String>,
    pub packages: Vec<UpdatePackage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub available: bool,
    pub distribution: String,
    pub package: Option<UpdatePackage>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadUpdateResult {
    pub version: String,
    pub target: UpdateTarget,
    pub path: String,
    pub ready_to_restart: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortableVersionMarker {
    version: String,
    executable: String,
}

const BUILT_IN_UPDATE_MANIFEST_URL: &str =
    "http://127.0.0.1:9001/echo/updates/stable/latest.json";
const PORTABLE_MARKER: &str = "portable.json";
const CURRENT_MARKER: &str = "current.json";

pub fn relaunch_latest_portable_if_needed() {
    if std::env::var("ECHO_SKIP_PORTABLE_RELAUNCH").ok().as_deref() == Some("1") {
        return;
    }

    let Ok(current_exe) = std::env::current_exe() else {
        return;
    };
    let Some(root) = portable_root_from_exe(&current_exe) else {
        return;
    };
    let Ok(current_version) = Version::parse(env!("CARGO_PKG_VERSION")) else {
        return;
    };
    let Some((latest_version, latest_exe)) = find_latest_portable_executable(&root) else {
        return;
    };

    if latest_version <= current_version || same_file_best_effort(&current_exe, &latest_exe) {
        return;
    }

    let mut command = Command::new(latest_exe);
    command.env("ECHO_SKIP_PORTABLE_RELAUNCH", "1");
    if let Ok(data_dir) = std::env::var("ECHO_DATA_DIR") {
        command.env("ECHO_DATA_DIR", data_dir);
    }
    if command.spawn().is_ok() {
        std::process::exit(0);
    }
}

pub async fn check_for_updates() -> Result<UpdateCheckResult> {
    let manifest = fetch_manifest().await?;
    let current_version =
        Version::parse(env!("CARGO_PKG_VERSION")).context("invalid current app version")?;
    let latest_version = Version::parse(&manifest.version).context("invalid manifest version")?;
    let distribution = current_distribution();

    let package = if latest_version > current_version {
        select_package(&manifest, &distribution).cloned()
    } else {
        None
    };

    Ok(UpdateCheckResult {
        current_version: current_version.to_string(),
        latest_version: Some(latest_version.to_string()),
        available: package.is_some(),
        distribution,
        package,
        notes: manifest.notes,
    })
}

pub async fn download_update(app: AppHandle) -> Result<DownloadUpdateResult> {
    let check = check_for_updates().await?;
    let Some(package) = check.package else {
        return Ok(DownloadUpdateResult {
            version: check.current_version,
            target: UpdateTarget::Portable,
            path: String::new(),
            ready_to_restart: false,
            message: "当前已是最新版本".to_string(),
        });
    };
    let version = check
        .latest_version
        .clone()
        .ok_or_else(|| anyhow!("missing latest version"))?;

    match package.target {
        UpdateTarget::Portable => download_portable(app, &version, &package).await,
        UpdateTarget::Installer => download_installer(app, &version, &package).await,
    }
}

pub fn spawn_background_update_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            match check_for_updates().await {
                Ok(result) if result.available => {
                    let _ = app.emit_all("update-available", &result);
                }
                Ok(_) => {}
                Err(err) => {
                    log::debug!("Background update check skipped: {}", err);
                }
            }
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

async fn fetch_manifest() -> Result<UpdateManifest> {
    let manifest = reqwest::Client::new()
        .get(update_manifest_url())
        .send()
        .await
        .context("failed to request update manifest")?
        .error_for_status()
        .context("update manifest returned an error status")?
        .json::<UpdateManifest>()
        .await
        .context("failed to parse update manifest")?;
    Ok(manifest)
}

fn update_manifest_url() -> &'static str {
    option_env!("ECHO_BUILT_IN_UPDATE_MANIFEST_URL").unwrap_or(BUILT_IN_UPDATE_MANIFEST_URL)
}

fn select_package<'a>(
    manifest: &'a UpdateManifest,
    distribution: &str,
) -> Option<&'a UpdatePackage> {
    let platform = current_platform();
    let arch = current_arch();
    let target = if distribution == "portable" {
        UpdateTarget::Portable
    } else {
        UpdateTarget::Installer
    };

    manifest
        .packages
        .iter()
        .find(|pkg| pkg.target == target && pkg.platform == platform && pkg.arch == arch)
}

async fn download_portable(
    app: AppHandle,
    version: &str,
    package: &UpdatePackage,
) -> Result<DownloadUpdateResult> {
    let root = portable_root()?;
    let downloads_dir = root.join(".updates").join("downloading").join(version);
    let version_dir = root.join("versions").join(version);
    fs::create_dir_all(&downloads_dir).context("failed to create update download directory")?;
    fs::create_dir_all(&version_dir).context("failed to create portable version directory")?;

    let archive_path = downloads_dir.join(package_filename(package));
    download_file(&app, package, &archive_path).await?;
    verify_sha256(&archive_path, &package.sha256)?;
    extract_zip(&archive_path, &version_dir)?;

    let executable = find_executable_in_dir(&version_dir)
        .ok_or_else(|| anyhow!("未在更新包中找到可执行文件"))?;
    let marker = PortableVersionMarker {
        version: version.to_string(),
        executable: path_relative_to(&executable, &version_dir)
            .unwrap_or(executable.clone())
            .to_string_lossy()
            .to_string(),
    };
    write_json_atomic(&version_dir.join(CURRENT_MARKER), &marker)?;
    write_json_atomic(&root.join(CURRENT_MARKER), &marker)?;

    Ok(DownloadUpdateResult {
        version: version.to_string(),
        target: UpdateTarget::Portable,
        path: version_dir.to_string_lossy().to_string(),
        ready_to_restart: true,
        message: "新版已下载，重启后生效".to_string(),
    })
}

async fn download_installer(
    app: AppHandle,
    version: &str,
    package: &UpdatePackage,
) -> Result<DownloadUpdateResult> {
    let base = app
        .path_resolver()
        .app_data_dir()
        .ok_or_else(|| anyhow!("failed to resolve app data dir"))?;
    let installers_dir = base.join("updates").join("installers").join(version);
    fs::create_dir_all(&installers_dir).context("failed to create installer update directory")?;

    let installer_path = installers_dir.join(package_filename(package));
    download_file(&app, package, &installer_path).await?;
    verify_sha256(&installer_path, &package.sha256)?;

    Ok(DownloadUpdateResult {
        version: version.to_string(),
        target: UpdateTarget::Installer,
        path: installer_path.to_string_lossy().to_string(),
        ready_to_restart: false,
        message: "安装包已下载，请运行安装器完成更新".to_string(),
    })
}

async fn download_file(app: &AppHandle, package: &UpdatePackage, dest: &Path) -> Result<()> {
    let response = reqwest::Client::new()
        .get(&package.url)
        .send()
        .await
        .context("failed to request update package")?
        .error_for_status()
        .context("update package returned an error status")?;

    let total = response.content_length().or(package.size);
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(dest)
        .await
        .context("failed to create update package file")?;
    let mut downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("failed while downloading update package")?;
        file.write_all(&chunk)
            .await
            .context("failed to write update package")?;
        downloaded += chunk.len() as u64;
        let _ = app.emit_all(
            "update-download-progress",
            serde_json::json!({ "downloaded": downloaded, "total": total }),
        );
    }
    file.flush()
        .await
        .context("failed to flush update package")?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = fs::File::open(path).context("failed to open downloaded update")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .context("failed to read downloaded update")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(anyhow!("更新包校验失败"));
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, dest: &Path) -> Result<()> {
    let bytes = fs::read(archive_path).context("failed to read update archive")?;
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).context("failed to open zip update archive")?;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .context("failed to read zip entry")?;
        let Some(enclosed) = file.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let out_path = dest.join(enclosed);
        if file.is_dir() {
            fs::create_dir_all(&out_path).context("failed to create update directory")?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).context("failed to create update parent directory")?;
            }
            let mut out_file =
                fs::File::create(&out_path).context("failed to create update file")?;
            std::io::copy(&mut file, &mut out_file).context("failed to extract update file")?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = file.unix_mode() {
                    fs::set_permissions(&out_path, fs::Permissions::from_mode(mode)).ok();
                }
            }
        }
    }
    Ok(())
}

fn current_distribution() -> String {
    if std::env::var("ECHO_DISTRIBUTION").ok().as_deref() == Some("installer") {
        return "installer".to_string();
    }
    if portable_root().is_ok() {
        "portable".to_string()
    } else {
        "installer".to_string()
    }
}

fn portable_root() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to resolve current executable")?;
    portable_root_from_exe(&exe).ok_or_else(|| anyhow!("当前版本不是便携式分发"))
}

fn portable_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    if exe_dir.join(PORTABLE_MARKER).exists() || exe_dir.join("versions").exists() {
        return Some(exe_dir.to_path_buf());
    }
    let versions_dir = exe_dir.parent()?;
    if versions_dir.file_name().and_then(|v| v.to_str()) == Some("versions") {
        return versions_dir.parent().map(|root| root.to_path_buf());
    }
    None
}

fn find_latest_portable_executable(root: &Path) -> Option<(Version, PathBuf)> {
    let versions_dir = root.join("versions");
    let entries = fs::read_dir(versions_dir).ok()?;
    let mut best: Option<(Version, PathBuf)> = None;

    for entry in entries.flatten() {
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let version = Version::parse(entry.file_name().to_string_lossy().as_ref()).ok()?;
        let dir = entry.path();
        let exe = read_marker_executable(&dir).or_else(|| find_executable_in_dir(&dir))?;
        if best
            .as_ref()
            .map(|(best_version, _)| &version > best_version)
            .unwrap_or(true)
        {
            best = Some((version, exe));
        }
    }
    best
}

fn read_marker_executable(dir: &Path) -> Option<PathBuf> {
    let marker = fs::read_to_string(dir.join(CURRENT_MARKER)).ok()?;
    let marker: PortableVersionMarker = serde_json::from_str(&marker).ok()?;
    Some(dir.join(marker.executable))
}

fn find_executable_in_dir(dir: &Path) -> Option<PathBuf> {
    let current_name = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_os_string()))?;
    find_file_named(dir, &current_name)
}

fn find_file_named(dir: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name() == Some(name) {
            return Some(path);
        }
        if entry.file_type().ok()?.is_dir() {
            if let Some(found) = find_file_named(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

fn package_filename(package: &UpdatePackage) -> String {
    package
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("update.zip")
        .split('?')
        .next()
        .unwrap_or("update.zip")
        .to_string()
}

fn current_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        other => other,
    }
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => other,
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let json = serde_json::to_vec_pretty(value).context("failed to serialize update marker")?;
    {
        let mut file = fs::File::create(&tmp).context("failed to create update marker")?;
        file.write_all(&json)
            .context("failed to write update marker")?;
        file.sync_all().ok();
    }
    fs::rename(tmp, path).context("failed to commit update marker")?;
    Ok(())
}

fn path_relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    path.strip_prefix(base).ok().map(|p| p.to_path_buf())
}

fn same_file_best_effort(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[tauri::command]
pub async fn check_for_updates_command() -> Result<UpdateCheckResult, String> {
    check_for_updates().await.map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn download_update_command(app: AppHandle) -> Result<DownloadUpdateResult, String> {
    download_update(app).await.map_err(|err| err.to_string())
}
