use anyhow::{anyhow, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/batao9/azooKey-Windows/releases/latest";
const RELEASE_API_URL_ENV: &str = "AZOOKEY_UPDATE_RELEASE_API_URL";
const CURRENT_VERSION_ENV: &str = "AZOOKEY_UPDATE_CURRENT_VERSION";
const INSTALLER_ASSET_NAME: &str = "azookey-setup.exe";
const SHA256SUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
const UPDATE_RESULT_FILENAME: &str = "update-result.json";
const APP_VERSION_JSON: &str = include_str!("../../../app-version.json");

#[derive(Debug, Deserialize)]
struct AppVersionConfig {
    version: String,
}

#[derive(Debug, Deserialize, Clone)]
struct ReleaseAsset {
    name: String,
    #[serde(default)]
    browser_download_url: String,
}

#[derive(Debug, Deserialize, Clone)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    html_url: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct UpdateCheckResponse {
    pub current_version: String,
    pub latest_version: String,
    pub latest_tag: String,
    pub release_name: String,
    pub release_url: String,
    pub update_available: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct UpdateStartResponse {
    pub latest_version: String,
    pub installer_path: String,
    pub result_path: String,
    pub install_log_path: String,
    pub launched: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct UpdateInstallResult {
    pub status: String,
    pub exit_code: Option<i32>,
    pub needs_restart: bool,
    pub message: String,
    pub completed_at: String,
    pub installer_path: Option<String>,
    pub install_log_path: Option<String>,
}

struct ReleaseAssets {
    installer_url: String,
    sha256sums_url: String,
}

pub async fn check_for_updates() -> Result<UpdateCheckResponse> {
    let release = fetch_latest_release().await?;
    update_check_response(&release)
}

pub async fn download_and_launch_update() -> Result<UpdateStartResponse> {
    let release = fetch_latest_release().await?;
    let check = update_check_response(&release)?;
    if !check.update_available {
        return Err(anyhow!("利用可能な更新はありません"));
    }

    let assets = select_release_assets(&release.assets)?;
    let client = http_client()?;
    let sha256sums = download_text(&client, &assets.sha256sums_url).await?;
    let expected_hash = parse_sha256sum(&sha256sums, INSTALLER_ASSET_NAME)?;

    let staging_dir = updater_staging_dir()?;
    fs::create_dir_all(&staging_dir).with_context(|| {
        format!(
            "failed to create update staging dir: {}",
            staging_dir.display()
        )
    })?;
    let installer_path = staging_dir.join(INSTALLER_ASSET_NAME);
    let actual_hash =
        match download_file_with_sha256(&client, &assets.installer_url, &installer_path).await {
            Ok(hash) => hash,
            Err(error) => {
                cleanup_download_paths(&installer_path);
                return Err(error);
            }
        };
    if !hashes_match(&expected_hash, &actual_hash) {
        cleanup_download_paths(&installer_path);
        return Err(anyhow!(
            "installer hash mismatch: expected {}, actual {}",
            expected_hash,
            actual_hash
        ));
    }

    let result_path = update_result_path()?;
    let install_log_path = staging_dir.join("azookey-update-install.log");
    launch_installer_helper(&installer_path, &result_path, &install_log_path)?;

    Ok(UpdateStartResponse {
        latest_version: check.latest_version,
        installer_path: installer_path.display().to_string(),
        result_path: result_path.display().to_string(),
        install_log_path: install_log_path.display().to_string(),
        launched: true,
    })
}

pub fn take_update_install_result() -> Result<Option<UpdateInstallResult>> {
    let path = update_result_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read update result: {}", path.display()))?;
    let result = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse update result: {}", path.display()))?;
    fs::remove_file(&path)
        .with_context(|| format!("failed to remove update result: {}", path.display()))?;
    Ok(Some(result))
}

async fn fetch_latest_release() -> Result<GithubRelease> {
    let url = release_api_url();
    let client = http_client()?;
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to request latest release: {url}"))?
        .error_for_status()
        .with_context(|| format!("latest release request failed: {url}"))?;

    response
        .json::<GithubRelease>()
        .await
        .context("failed to parse latest release response")
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("azookey-windows-updater")
        .build()
        .context("failed to build HTTP client")
}

fn release_api_url() -> String {
    env::var(RELEASE_API_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_API_URL.to_string())
}

fn current_version_string() -> Result<String> {
    if let Ok(version) = env::var(CURRENT_VERSION_ENV) {
        let trimmed = version.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let config: AppVersionConfig =
        serde_json::from_str(APP_VERSION_JSON).context("failed to parse app-version.json")?;
    Ok(config.version)
}

fn update_check_response(release: &GithubRelease) -> Result<UpdateCheckResponse> {
    let current_version = current_version_string()?;
    let latest_version = normalize_version(&release.tag_name)?;
    let current = parse_version(&current_version)?;
    let latest = parse_version(&latest_version)?;

    Ok(UpdateCheckResponse {
        current_version,
        latest_version,
        latest_tag: release.tag_name.clone(),
        release_name: release.name.clone(),
        release_url: release.html_url.clone(),
        update_available: latest > current,
    })
}

fn normalize_version(value: &str) -> Result<String> {
    let trimmed = value.trim();
    let without_prefix = trimmed.strip_prefix('v').unwrap_or(trimmed);
    parse_version(without_prefix)?;
    Ok(without_prefix.to_string())
}

fn parse_version(value: &str) -> Result<Version> {
    Version::parse(value.trim().strip_prefix('v').unwrap_or(value.trim()))
        .with_context(|| format!("invalid version: {value}"))
}

fn select_release_assets(assets: &[ReleaseAsset]) -> Result<ReleaseAssets> {
    let installer = find_asset_download_url(assets, INSTALLER_ASSET_NAME)?;
    let sha256sums = find_asset_download_url(assets, SHA256SUMS_ASSET_NAME)?;
    Ok(ReleaseAssets {
        installer_url: installer.to_string(),
        sha256sums_url: sha256sums.to_string(),
    })
}

fn find_asset_download_url<'a>(assets: &'a [ReleaseAsset], name: &str) -> Result<&'a str> {
    let asset = assets
        .iter()
        .find(|asset| asset.name == name)
        .ok_or_else(|| anyhow!("release asset not found: {name}"))?;
    if asset.browser_download_url.trim().is_empty() {
        return Err(anyhow!("release asset has no download URL: {name}"));
    }
    Ok(&asset.browser_download_url)
}

async fn download_text(client: &reqwest::Client, url: &str) -> Result<String> {
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed: {url}"))?
        .text()
        .await
        .with_context(|| format!("failed to read text response: {url}"))
}

async fn download_file_with_sha256(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
) -> Result<String> {
    cleanup_download_paths(destination);
    let mut response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed: {url}"))?;
    let partial = partial_download_path(destination);
    let mut file = fs::File::create(&partial)
        .with_context(|| format!("failed to create installer: {}", partial.display()))?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("failed to read binary response: {url}"))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .with_context(|| format!("failed to write installer: {}", partial.display()))?;
    }
    file.flush()
        .with_context(|| format!("failed to flush installer: {}", partial.display()))?;
    drop(file);

    let digest = hasher.finalize();
    let hash = format_sha256(&digest);
    fs::rename(&partial, destination).with_context(|| {
        format!(
            "failed to move installer into place: {}",
            destination.display()
        )
    })?;
    Ok(hash)
}

fn cleanup_download_paths(destination: &Path) {
    let _ = fs::remove_file(destination);
    let _ = fs::remove_file(partial_download_path(destination));
}

fn partial_download_path(destination: &Path) -> PathBuf {
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "download".into());
    destination.with_file_name(format!("{file_name}.part"))
}

fn parse_sha256sum(contents: &str, filename: &str) -> Result<String> {
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut parts = trimmed.split_whitespace();
        let Some(hash) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name.trim_start_matches('*') == filename {
            if hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Ok(hash.to_ascii_lowercase());
            }
            return Err(anyhow!("invalid SHA-256 hash for {filename}"));
        }
    }

    Err(anyhow!("SHA-256 hash not found for {filename}"))
}

fn hashes_match(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
}

fn format_sha256(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn app_data_dir() -> Result<PathBuf> {
    let appdata = env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA is not set"))?;
    Ok(PathBuf::from(appdata).join("Azookey"))
}

fn update_result_path() -> Result<PathBuf> {
    let dir = app_data_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create app data dir: {}", dir.display()))?;
    Ok(dir.join(UPDATE_RESULT_FILENAME))
}

fn updater_staging_dir() -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX epoch")?
        .as_secs();
    Ok(env::temp_dir().join(format!("azookey-update-{nonce}")))
}

fn launch_installer_helper(
    installer_path: &Path,
    result_path: &Path,
    install_log_path: &Path,
) -> Result<()> {
    let script_path = installer_path
        .parent()
        .ok_or_else(|| anyhow!("installer path has no parent"))?
        .join("azookey-update-helper.ps1");
    write_installer_helper_script(&script_path)?;

    let status = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-File")
        .arg(&script_path)
        .arg("-InstallerPath")
        .arg(installer_path)
        .arg("-ResultPath")
        .arg(result_path)
        .arg("-InstallLogPath")
        .arg(install_log_path)
        .spawn()
        .context("failed to launch updater helper")?;

    drop(status);
    Ok(())
}

fn write_installer_helper_script(script_path: &Path) -> Result<()> {
    let mut file = fs::File::create(script_path)
        .with_context(|| format!("failed to create updater helper: {}", script_path.display()))?;
    file.write_all(INSTALLER_HELPER_PS1.as_bytes())
        .with_context(|| format!("failed to write updater helper: {}", script_path.display()))?;
    Ok(())
}

const INSTALLER_HELPER_PS1: &str = r#"
param(
  [Parameter(Mandatory = $true)][string]$InstallerPath,
  [Parameter(Mandatory = $true)][string]$ResultPath,
  [Parameter(Mandatory = $true)][string]$InstallLogPath
)

$ErrorActionPreference = "Stop"

function Write-UpdateResult {
  param(
    [Parameter(Mandatory = $true)][string]$Status,
    [object]$ExitCode,
    [Parameter(Mandatory = $true)][bool]$NeedsRestart,
    [Parameter(Mandatory = $true)][string]$Message
  )

  $resultDir = Split-Path -Parent $ResultPath
  New-Item -ItemType Directory -Force -Path $resultDir | Out-Null
  $json = [PSCustomObject]@{
    status = $Status
    exit_code = $ExitCode
    needs_restart = $NeedsRestart
    message = $Message
    completed_at = (Get-Date).ToUniversalTime().ToString("o")
    installer_path = $InstallerPath
    install_log_path = $InstallLogPath
  } | ConvertTo-Json -Depth 3
  $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
  [System.IO.File]::WriteAllText($ResultPath, $json, $utf8NoBom)
}

try {
  $installerArgs = @(
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/RESTARTEXITCODE=3010",
    "/LOG=$InstallLogPath"
  )
  $proc = Start-Process -FilePath $InstallerPath -ArgumentList $installerArgs -Wait -PassThru
  if ($proc.ExitCode -eq 0) {
    Write-UpdateResult -Status "success" -ExitCode $proc.ExitCode -NeedsRestart $false -Message "更新が完了しました。"
    exit 0
  }
  if ($proc.ExitCode -eq 3010) {
    Write-UpdateResult -Status "success" -ExitCode $proc.ExitCode -NeedsRestart $true -Message "更新が完了しました。Windows の再起動が必要です。"
    exit 0
  }

  Write-UpdateResult -Status "failed" -ExitCode $proc.ExitCode -NeedsRestart $false -Message "更新に失敗しました。終了コード: $($proc.ExitCode)"
  exit 1
} catch {
  Write-UpdateResult -Status "failed" -ExitCode $null -NeedsRestart $false -Message $_.Exception.Message
  exit 1
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsString,
        sync::{Mutex, MutexGuard, OnceLock},
    };

    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct EnvGuard {
        _guard: MutexGuard<'static, ()>,
        release_api_url: Option<OsString>,
        current_version: Option<OsString>,
        appdata: Option<OsString>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let guard = env_lock();
            Self {
                _guard: guard,
                release_api_url: env::var_os(RELEASE_API_URL_ENV),
                current_version: env::var_os(CURRENT_VERSION_ENV),
                appdata: env::var_os("APPDATA"),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.release_api_url {
                    Some(value) => env::set_var(RELEASE_API_URL_ENV, value),
                    None => env::remove_var(RELEASE_API_URL_ENV),
                }
                match &self.current_version {
                    Some(value) => env::set_var(CURRENT_VERSION_ENV, value),
                    None => env::remove_var(CURRENT_VERSION_ENV),
                }
                match &self.appdata {
                    Some(value) => env::set_var("APPDATA", value),
                    None => env::remove_var("APPDATA"),
                }
            }
        }
    }

    fn release(tag_name: &str) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.to_string(),
            name: format!("Release {tag_name}"),
            html_url: "https://example.test/release".to_string(),
            assets: vec![
                ReleaseAsset {
                    name: INSTALLER_ASSET_NAME.to_string(),
                    browser_download_url: "https://example.test/azookey-setup.exe".to_string(),
                },
                ReleaseAsset {
                    name: SHA256SUMS_ASSET_NAME.to_string(),
                    browser_download_url: "https://example.test/SHA256SUMS.txt".to_string(),
                },
            ],
        }
    }

    #[test]
    fn compares_versions_with_v_prefix() {
        let _env = EnvGuard::new();
        unsafe {
            env::set_var(CURRENT_VERSION_ENV, "0.1.0-batao.2");
        }

        let response = update_check_response(&release("v0.1.0-batao.3")).unwrap();

        assert!(response.update_available);
        assert_eq!(response.latest_version, "0.1.0-batao.3");
    }

    #[test]
    fn reports_no_update_for_same_version() {
        let _env = EnvGuard::new();
        unsafe {
            env::set_var(CURRENT_VERSION_ENV, "0.1.0-batao.3");
        }

        let response = update_check_response(&release("v0.1.0-batao.3")).unwrap();

        assert!(!response.update_available);
    }

    #[test]
    fn compares_prerelease_suffixes_as_semver() {
        let _env = EnvGuard::new();
        unsafe {
            env::set_var(CURRENT_VERSION_ENV, "0.1.0-batao.2");
        }

        let response = update_check_response(&release("v0.1.0-batao.10")).unwrap();

        assert!(response.update_available);
    }

    #[test]
    fn selects_required_release_assets() {
        let assets = select_release_assets(&release("v0.1.0").assets).unwrap();

        assert_eq!(
            assets.installer_url,
            "https://example.test/azookey-setup.exe"
        );
        assert_eq!(assets.sha256sums_url, "https://example.test/SHA256SUMS.txt");
    }

    #[test]
    fn rejects_missing_release_asset() {
        let err = select_release_assets(&[]).unwrap_err();

        assert!(err.to_string().contains(INSTALLER_ASSET_NAME));
    }

    #[test]
    fn rejects_missing_hash_asset() {
        let assets = vec![ReleaseAsset {
            name: INSTALLER_ASSET_NAME.to_string(),
            browser_download_url: "https://example.test/azookey-setup.exe".to_string(),
        }];

        let err = select_release_assets(&assets).unwrap_err();

        assert!(err.to_string().contains(SHA256SUMS_ASSET_NAME));
    }

    #[test]
    fn rejects_similar_installer_asset_name() {
        let assets = vec![
            ReleaseAsset {
                name: "azookey-setup-old.exe".to_string(),
                browser_download_url: "https://example.test/azookey-setup-old.exe".to_string(),
            },
            ReleaseAsset {
                name: SHA256SUMS_ASSET_NAME.to_string(),
                browser_download_url: "https://example.test/SHA256SUMS.txt".to_string(),
            },
        ];

        let err = select_release_assets(&assets).unwrap_err();

        assert!(err.to_string().contains(INSTALLER_ASSET_NAME));
    }

    #[test]
    fn parses_sha256sum_for_installer() {
        let hash = "F36FCAE86160DBEA7FD605CCD7355E3DAFE51F04BE10C2FA95E25AA01F60C475";
        let contents = format!("{hash}  {INSTALLER_ASSET_NAME}\n");

        let parsed = parse_sha256sum(&contents, INSTALLER_ASSET_NAME).unwrap();

        assert_eq!(parsed, hash.to_ascii_lowercase());
    }

    #[test]
    fn rejects_hash_mismatch() {
        assert!(!hashes_match(
            "a".repeat(64).as_str(),
            "b".repeat(64).as_str()
        ));
    }

    #[test]
    fn partial_download_path_stays_out_of_final_installer_name() {
        let path = PathBuf::from(r"C:\Temp\azookey-setup.exe");

        let partial = partial_download_path(&path);

        assert_eq!(
            partial.file_name().and_then(|name| name.to_str()),
            Some("azookey-setup.exe.part")
        );
    }

    #[test]
    fn cleanup_removes_final_and_partial_downloads() {
        let temp = tempfile::tempdir().unwrap();
        let installer = temp.path().join(INSTALLER_ASSET_NAME);
        let partial = partial_download_path(&installer);
        fs::write(&installer, b"final").unwrap();
        fs::write(&partial, b"partial").unwrap();

        cleanup_download_paths(&installer);

        assert!(!installer.exists());
        assert!(!partial.exists());
    }

    #[test]
    fn helper_requests_restart_exit_code() {
        assert!(INSTALLER_HELPER_PS1.contains(r#""/NORESTART""#));
        assert!(INSTALLER_HELPER_PS1.contains(r#""/RESTARTEXITCODE=3010""#));
    }

    #[test]
    fn env_overrides_are_used() {
        let _env = EnvGuard::new();
        unsafe {
            env::set_var(RELEASE_API_URL_ENV, "http://127.0.0.1:7777/latest.json");
            env::set_var(CURRENT_VERSION_ENV, "0.0.1");
        }

        assert_eq!(release_api_url(), "http://127.0.0.1:7777/latest.json");
        assert_eq!(current_version_string().unwrap(), "0.0.1");
    }

    #[test]
    fn update_result_is_taken_once() {
        let _env = EnvGuard::new();
        let temp = tempfile::tempdir().unwrap();
        unsafe {
            env::set_var("APPDATA", temp.path());
        }
        let path = update_result_path().unwrap();
        fs::write(
            &path,
            r#"{
  "status": "success",
  "exit_code": 3010,
  "needs_restart": true,
  "message": "restart required",
  "completed_at": "2026-05-27T00:00:00Z",
  "installer_path": "installer.exe",
  "install_log_path": "install.log"
}"#,
        )
        .unwrap();

        let result = take_update_install_result().unwrap().unwrap();

        assert_eq!(result.exit_code, Some(3010));
        assert!(result.needs_restart);
        assert!(take_update_install_result().unwrap().is_none());
    }

    #[test]
    fn update_result_preserves_success_exit_zero() {
        let result: UpdateInstallResult = serde_json::from_str(
            r#"{
  "status": "success",
  "exit_code": 0,
  "needs_restart": false,
  "message": "updated",
  "completed_at": "2026-05-27T00:00:00Z",
  "installer_path": "installer.exe",
  "install_log_path": "install.log"
}"#,
        )
        .unwrap();

        assert_eq!(result.status, "success");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.needs_restart);
    }

    #[test]
    fn update_result_preserves_failed_exit_code() {
        let result: UpdateInstallResult = serde_json::from_str(
            r#"{
  "status": "failed",
  "exit_code": 42,
  "needs_restart": false,
  "message": "failed",
  "completed_at": "2026-05-27T00:00:00Z",
  "installer_path": "installer.exe",
  "install_log_path": "install.log"
}"#,
        )
        .unwrap();

        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, Some(42));
        assert!(!result.needs_restart);
    }
}
