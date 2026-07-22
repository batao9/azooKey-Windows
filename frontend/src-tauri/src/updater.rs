use anyhow::{anyhow, Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(not(windows))]
use std::process::Command;

const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/batao9/azooKey-Windows/releases/latest";
const RELEASE_API_URL_ENV: &str = "AZOOKEY_UPDATE_RELEASE_API_URL";
const CURRENT_VERSION_ENV: &str = "AZOOKEY_UPDATE_CURRENT_VERSION";
const INSTALLER_ASSET_NAME: &str = "azookey-setup.exe";
const UPDATE_DOWNLOAD_STAGING_PREFIX: &str = "azookey-update-";
const SHA256SUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
const UPDATE_HELPER_EXE_NAME: &str = "azookey-updater-helper.exe";
const INSTALLER_LOCK_SHARE_MODE: u32 = 0x0000_0001;
const PROTECTED_UPDATE_STAGING_DIRECTORY_NAME: &str = ".azookey-updater-staging";
const UPDATE_RESULT_FILENAME: &str = "update-result.json";
const UPDATE_RESULT_REGISTRY_KEY: &str = r"Software\Azookey";
const UPDATE_RESULT_REGISTRY_VALUE: &str = "UpdateResultJson";
const APP_VERSION_JSON: &str = include_str!("../../../app-version.json");

#[cfg(windows)]
fn reject_reparse_point(path: &Path, label: &str) -> Result<()> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {label}: {}", path.display()))?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(anyhow!("{label} is a reparse point: {}", path.display()));
    }
    Ok(())
}

#[cfg(windows)]
fn protected_install_directory(executable: &Path) -> Result<PathBuf> {
    use windows::Win32::{
        Foundation::HANDLE,
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };

    let raw_program_files =
        unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, HANDLE::default()) }
            .context("failed to resolve Program Files")?;
    let program_files_string = unsafe { raw_program_files.to_string() };
    unsafe { CoTaskMemFree(Some(raw_program_files.0.cast())) };
    let expected =
        PathBuf::from(program_files_string.context("Program Files is not valid UTF-16")?)
            .join("Azookey");
    reject_reparse_point(&expected, "expected Azookey install directory")?;

    let install_dir = executable
        .parent()
        .ok_or_else(|| anyhow!("updater executable has no parent"))?;
    reject_reparse_point(install_dir, "updater install directory")?;

    let expected = fs::canonicalize(&expected).with_context(|| {
        format!(
            "failed to canonicalize expected Azookey install directory: {}",
            expected.display()
        )
    })?;
    let actual = fs::canonicalize(install_dir).with_context(|| {
        format!(
            "failed to canonicalize updater install directory: {}",
            install_dir.display()
        )
    })?;
    if !actual
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.to_string_lossy())
    {
        return Err(anyhow!(
            "updater executable is outside the protected install directory: {}",
            actual.display()
        ));
    }
    Ok(actual)
}

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

#[derive(Debug)]
struct ReleaseAssets {
    installer_url: String,
    sha256sums_url: String,
}

pub async fn check_for_updates() -> Result<UpdateCheckResponse> {
    let release = fetch_latest_release().await?;
    update_check_response(&release)
}

pub async fn download_and_launch_update() -> Result<UpdateStartResponse> {
    download_and_launch_update_impl(false).await
}

pub async fn download_and_launch_update_for_integration_test() -> Result<UpdateStartResponse> {
    download_and_launch_update_impl(true).await
}

async fn download_and_launch_update_impl(silent_installer: bool) -> Result<UpdateStartResponse> {
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
                cleanup_owned_update_staging(&installer_path);
                return Err(error);
            }
        };
    if !hashes_match(&expected_hash, &actual_hash) {
        cleanup_owned_update_staging(&installer_path);
        return Err(anyhow!(
            "installer hash mismatch: expected {}, actual {}",
            expected_hash,
            actual_hash
        ));
    }

    let launch_result = (|| -> Result<(PathBuf, PathBuf)> {
        let result_path = update_result_path()?;
        let install_log_path = staging_dir.join("azookey-update-install.log");
        delete_protected_update_result()?;
        delete_legacy_update_result(&result_path)?;
        launch_installer_helper(
            &installer_path,
            &expected_hash,
            &result_path,
            &install_log_path,
            silent_installer,
        )?;
        Ok((result_path, install_log_path))
    })();
    let (result_path, install_log_path) = match launch_result {
        Ok(paths) => paths,
        Err(error) => {
            cleanup_owned_update_staging(&installer_path);
            return Err(error);
        }
    };

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
    // The elevated helper writes the current result to HKCU. Prefer it over the
    // pre-#129 AppData file so a stale or malformed legacy file cannot shadow a
    // completed update forever.
    let Some((data, from_legacy_file)) =
        select_update_result_data(read_protected_update_result()?, &path)?
    else {
        return Ok(None);
    };
    let mut result: UpdateInstallResult = serde_json::from_str(&data)
        .with_context(|| format!("failed to parse update result: {}", path.display()))?;
    normalize_update_install_result(&mut result);
    if from_legacy_file {
        delete_legacy_update_result(&path)?;
    } else {
        delete_protected_update_result()?;
        // Best-effort cleanup of a stale pre-#129 result. It must never prevent
        // consuming the authenticated current result from the registry.
        let _ = delete_legacy_update_result(&path);
    }
    Ok(Some(result))
}

fn select_update_result_data(
    protected_result: Option<String>,
    legacy_path: &Path,
) -> Result<Option<(String, bool)>> {
    if let Some(data) = protected_result {
        return Ok(Some((data, false)));
    }
    if legacy_path.exists() {
        let data = fs::read_to_string(legacy_path)
            .with_context(|| format!("failed to read update result: {}", legacy_path.display()))?;
        return Ok(Some((data, true)));
    }
    Ok(None)
}

fn normalize_update_install_result(result: &mut UpdateInstallResult) {
    if result.exit_code == Some(3010) {
        result.status = "success".to_string();
        result.needs_restart = true;
    }

    if result.status == "success" {
        result.message = if result.needs_restart {
            "更新が完了しました。Windows の再起動が必要です。".to_string()
        } else {
            "更新が完了しました。".to_string()
        };
        return;
    }

    if result.status == "failed" {
        if let Some(exit_code) = result.exit_code {
            result.message = format!("更新に失敗しました。終了コード: {exit_code}");
        }
    }
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

fn is_owned_update_staging_installer(installer_path: &Path) -> bool {
    if installer_path.file_name() != Some(std::ffi::OsStr::new(INSTALLER_ASSET_NAME)) {
        return false;
    }
    let Some(staging_dir) = installer_path.parent() else {
        return false;
    };
    let Some(staging_name) = staging_dir.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(nonce) = staging_name.strip_prefix(UPDATE_DOWNLOAD_STAGING_PREFIX) else {
        return false;
    };
    if nonce.is_empty() || !nonce.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    staging_dir.parent() == Some(env::temp_dir().as_path())
}

fn cleanup_owned_update_staging(installer_path: &Path) {
    if !is_owned_update_staging_installer(installer_path) {
        return;
    }
    cleanup_download_paths(installer_path);
    let Some(staging_dir) = installer_path.parent() else {
        return;
    };
    let _ = fs::remove_file(staging_dir.join("azookey-update-install.log"));
    let _ = fs::remove_dir(staging_dir);
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

fn delete_legacy_update_result(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            fs::remove_file(path).with_context(|| {
                format!("failed to remove legacy update result: {}", path.display())
            })?;
        }
        Ok(_) => {
            return Err(anyhow!(
                "legacy update result is not a file: {}",
                path.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect legacy update result: {}", path.display())
            });
        }
    }

    if fs::symlink_metadata(path).is_ok() {
        return Err(anyhow!(
            "legacy update result still exists after deletion: {}",
            path.display()
        ));
    }
    Ok(())
}

fn updater_staging_dir() -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX epoch")?
        .as_secs();
    Ok(env::temp_dir().join(format!("{UPDATE_DOWNLOAD_STAGING_PREFIX}{nonce}")))
}

#[cfg(windows)]
fn launch_installer_helper(
    installer_path: &Path,
    expected_hash: &str,
    result_path: &Path,
    install_log_path: &Path,
    silent_installer: bool,
) -> Result<()> {
    use std::{ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE},
            UI::{
                Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
    };

    let executable = env::current_exe().context("failed to resolve updater executable")?;
    let install_dir = protected_install_directory(&executable)?;
    let helper_path = install_dir.join(UPDATE_HELPER_EXE_NAME);
    if !helper_path.is_file() {
        return Err(anyhow!(
            "protected updater helper is missing: {}",
            helper_path.display()
        ));
    }
    reject_reparse_point(&helper_path, "protected updater helper")?;

    let quoted = |value: &OsStr| -> Result<String> {
        let value = value.to_string_lossy();
        if value.contains('"') {
            return Err(anyhow!("updater helper argument contains a quote"));
        }
        Ok(format!("\"{value}\""))
    };
    let mut parameters = format!(
        "--azookey-apply-update --installer-path {} --expected-sha256 {} --result-path {} --install-log-path {}",
        quoted(installer_path.as_os_str())?,
        expected_hash,
        quoted(result_path.as_os_str())?,
        quoted(install_log_path.as_os_str())?
    );
    if silent_installer {
        parameters.push_str(" --silent");
    }

    let verb: Vec<u16> = OsStr::new("runas").encode_wide().chain(Some(0)).collect();
    let helper: Vec<u16> = helper_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let parameters: Vec<u16> = OsStr::new(&parameters)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let working_dir: Vec<u16> = install_dir
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut execute_info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(helper.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: PCWSTR(working_dir.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut execute_info) }
        .context("failed to launch elevated protected updater helper")?;
    if execute_info.hProcess == HANDLE::default() {
        return Err(anyhow!(
            "ShellExecuteEx returned no updater helper process handle"
        ));
    }
    let _ = unsafe { CloseHandle(execute_info.hProcess) };

    Ok(())
}

#[cfg(not(windows))]
fn launch_installer_helper(
    installer_path: &Path,
    expected_hash: &str,
    result_path: &Path,
    install_log_path: &Path,
    silent_installer: bool,
) -> Result<()> {
    let executable = env::current_exe().context("failed to resolve updater executable")?;
    let install_dir = executable
        .parent()
        .ok_or_else(|| anyhow!("updater executable has no parent"))?;
    let helper_path = install_dir.join(UPDATE_HELPER_EXE_NAME);
    let mut command = Command::new(&helper_path);
    command
        .arg("--azookey-apply-update")
        .arg("--installer-path")
        .arg(installer_path)
        .arg("--expected-sha256")
        .arg(expected_hash)
        .arg("--result-path")
        .arg(result_path)
        .arg("--install-log-path")
        .arg(install_log_path);
    if silent_installer {
        command.arg("--silent");
    }
    command
        .spawn()
        .context("failed to launch protected updater helper")?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct InstallerHelperArgs {
    installer_path: PathBuf,
    expected_sha256: String,
    result_path: PathBuf,
    install_log_path: PathBuf,
    silent: bool,
}

fn next_helper_arg(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<OsString> {
    args.next()
        .ok_or_else(|| anyhow!("missing value for updater helper option: {option}"))
}

fn parse_installer_helper_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<InstallerHelperArgs> {
    let mut args = args.into_iter();
    let mut installer_path = None;
    let mut expected_sha256 = None;
    let mut result_path = None;
    let mut install_log_path = None;
    let mut silent = false;

    while let Some(option) = args.next() {
        match option.to_string_lossy().as_ref() {
            "--installer-path" => {
                installer_path = Some(PathBuf::from(next_helper_arg(
                    &mut args,
                    "--installer-path",
                )?));
            }
            "--expected-sha256" => {
                expected_sha256 = Some(
                    next_helper_arg(&mut args, "--expected-sha256")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "--result-path" => {
                result_path = Some(PathBuf::from(next_helper_arg(&mut args, "--result-path")?));
            }
            "--install-log-path" => {
                install_log_path = Some(PathBuf::from(next_helper_arg(
                    &mut args,
                    "--install-log-path",
                )?));
            }
            "--silent" => silent = true,
            unexpected => return Err(anyhow!("unexpected updater helper option: {unexpected}")),
        }
    }

    let expected_sha256 = expected_sha256
        .filter(|value| is_sha256(value))
        .ok_or_else(|| anyhow!("missing or invalid expected installer SHA-256"))?;
    Ok(InstallerHelperArgs {
        installer_path: installer_path.ok_or_else(|| anyhow!("missing installer path"))?,
        expected_sha256,
        result_path: result_path.ok_or_else(|| anyhow!("missing result path"))?,
        install_log_path: install_log_path.ok_or_else(|| anyhow!("missing install log path"))?,
        silent,
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_from_reader(reader: &mut impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("failed to read installer")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format_sha256(&hasher.finalize()))
}

fn completed_at_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("unix:{seconds}")
}

#[cfg(not(windows))]
fn write_update_result(path: &Path, result: &UpdateInstallResult) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("update result path has no parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create update result dir: {}", parent.display()))?;
    let temporary = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(result).context("failed to serialize update result")?;
    fs::write(&temporary, data)
        .with_context(|| format!("failed to write update result: {}", temporary.display()))?;
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to replace update result: {}", path.display()))?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to publish update result: {}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn write_update_result(_path: &Path, result: &UpdateInstallResult) -> Result<()> {
    let data = serde_json::to_string_pretty(result).context("failed to serialize update result")?;
    let key = windows_registry::CURRENT_USER
        .create(UPDATE_RESULT_REGISTRY_KEY)
        .context("failed to create updater result registry key")?;
    key.set_string(UPDATE_RESULT_REGISTRY_VALUE, &data)
        .context("failed to write updater result registry value")?;
    Ok(())
}

#[cfg(windows)]
fn read_protected_update_result() -> Result<Option<String>> {
    let key = match windows_registry::CURRENT_USER.open(UPDATE_RESULT_REGISTRY_KEY) {
        Ok(key) => key,
        Err(_) => return Ok(None),
    };
    match key.get_string(UPDATE_RESULT_REGISTRY_VALUE) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(None),
    }
}

#[cfg(not(windows))]
fn read_protected_update_result() -> Result<Option<String>> {
    Ok(None)
}

#[cfg(windows)]
fn delete_protected_update_result() -> Result<()> {
    // `Key::open` is read-only. Use `create` to obtain KEY_WRITE as well, and
    // verify absence so a stale success cannot be mistaken for the next run.
    let key = windows_registry::CURRENT_USER
        .create(UPDATE_RESULT_REGISTRY_KEY)
        .context("failed to open updater result registry key for deletion")?;
    let _ = key.remove_value(UPDATE_RESULT_REGISTRY_VALUE);
    if key.get_string(UPDATE_RESULT_REGISTRY_VALUE).is_ok() {
        return Err(anyhow!("failed to remove updater result registry value"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn delete_protected_update_result() -> Result<()> {
    Ok(())
}

#[derive(Debug)]
struct InstallerExecution {
    exit_code: i32,
    install_log_path: PathBuf,
}

#[cfg(windows)]
fn execute_verified_installer(
    installer_path: &Path,
    expected_sha256: &str,
    _requested_install_log_path: &Path,
    silent: bool,
) -> Result<InstallerExecution> {
    use std::{
        mem::size_of,
        os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
        process,
    };
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
            System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
            UI::{
                Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
                WindowsAndMessaging::SW_SHOWNORMAL,
            },
        },
    };

    // The download directory is user-writable and may contain reparse points.
    // Pin the downloaded bytes with a deny-write/delete handle, then copy those
    // exact bytes into a Program Files child that an unprivileged process cannot
    // replace before execution.
    let mut installer = fs::OpenOptions::new()
        .read(true)
        .share_mode(INSTALLER_LOCK_SHARE_MODE)
        .open(installer_path)
        .with_context(|| format!("failed to lock installer: {}", installer_path.display()))?;
    let actual_hash = sha256_from_reader(&mut installer)?;
    if !hashes_match(expected_sha256, &actual_hash) {
        return Err(anyhow!(
            "installer changed before elevation: expected {}, actual {}",
            expected_sha256,
            actual_hash
        ));
    }

    let helper_executable = env::current_exe().context("failed to resolve updater helper")?;
    let install_dir = protected_install_directory(&helper_executable)?;
    reject_reparse_point(&helper_executable, "protected updater helper")?;

    let protected_staging_dir = install_dir.join(PROTECTED_UPDATE_STAGING_DIRECTORY_NAME);
    fs::create_dir_all(&protected_staging_dir).with_context(|| {
        format!(
            "failed to create protected updater staging dir: {}",
            protected_staging_dir.display()
        )
    })?;
    reject_reparse_point(
        &protected_staging_dir,
        "protected updater staging directory",
    )?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let protected_installer_path = protected_staging_dir.join(format!(
        ".azookey-setup-verified-{}-{nonce}.exe",
        process::id()
    ));
    let protected_install_log_path = protected_staging_dir.join(format!(
        "azookey-update-install-{}-{nonce}.log",
        process::id()
    ));
    struct ProtectedInstallerCleanup {
        file: PathBuf,
        directory: PathBuf,
    }
    impl Drop for ProtectedInstallerCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.file);
            let _ = fs::remove_dir(&self.directory);
        }
    }
    let _protected_cleanup = ProtectedInstallerCleanup {
        file: protected_installer_path.clone(),
        directory: protected_staging_dir.clone(),
    };
    let mut protected_installer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(INSTALLER_LOCK_SHARE_MODE)
        .open(&protected_installer_path)
        .with_context(|| {
            format!(
                "failed to create protected installer copy: {}",
                protected_installer_path.display()
            )
        })?;
    installer.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut installer, &mut protected_installer)
        .context("failed to copy verified installer into Program Files")?;
    protected_installer
        .sync_all()
        .context("failed to flush protected installer copy")?;
    protected_installer.seek(SeekFrom::Start(0))?;
    let protected_hash = sha256_from_reader(&mut protected_installer)?;
    if !hashes_match(expected_sha256, &protected_hash) {
        return Err(anyhow!(
            "protected installer copy hash mismatch: expected {}, actual {}",
            expected_sha256,
            protected_hash
        ));
    }

    // The destination is now an independently verified file under the
    // protected Program Files ACL. Close the write handle before CreateProcess:
    // keeping it open with FILE_SHARE_READ only prevents the Windows loader
    // from reopening the executable and can deadlock ShellExecuteEx.
    drop(protected_installer);
    drop(installer);

    // The protected helper was already started with `runas`. Using `runas`
    // again here can leave a second UAC prompt waiting in a non-interactive
    // session. `open` inherits the helper's elevated token; the installer's
    // own requestedExecutionLevel still prompts if the helper was invoked
    // directly without elevation.
    let verb: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let executable: Vec<u16> = protected_installer_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let parameters = if silent {
        format!(
            "/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RESTARTEXITCODE=3010 /LOG=\"{}\"",
            protected_install_log_path.display()
        )
    } else {
        format!(
            "/RESTARTEXITCODE=3010 /LOG=\"{}\"",
            protected_install_log_path.display()
        )
    };
    let parameters: Vec<u16> = std::ffi::OsStr::new(&parameters)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let working_dir: Vec<u16> = protected_staging_dir
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();

    let mut execute_info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(executable.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        lpDirectory: PCWSTR(working_dir.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let result = (|| -> Result<InstallerExecution> {
        unsafe { ShellExecuteExW(&mut execute_info) }
            .context("failed to create elevated installer process")?;
        if execute_info.hProcess == HANDLE::default() {
            return Err(anyhow!(
                "ShellExecuteEx returned no installer process handle"
            ));
        }
        let process = execute_info.hProcess;
        let wait = unsafe { WaitForSingleObject(process, INFINITE) };
        if wait != WAIT_OBJECT_0 {
            let _ = unsafe { CloseHandle(process) };
            return Err(anyhow!("failed while waiting for installer: {wait:?}"));
        }
        let mut exit_code = 0_u32;
        let exit_result = unsafe { GetExitCodeProcess(process, &mut exit_code) }
            .context("failed to read installer exit code");
        let _ = unsafe { CloseHandle(process) };
        exit_result?;
        Ok(InstallerExecution {
            exit_code: exit_code as i32,
            install_log_path: protected_install_log_path,
        })
    })();
    result
}

#[cfg(not(windows))]
fn execute_verified_installer(
    _installer_path: &Path,
    _expected_sha256: &str,
    _install_log_path: &Path,
    _silent: bool,
) -> Result<InstallerExecution> {
    Err(anyhow!("the Azookey updater helper is Windows-only"))
}

fn helper_result(args: &InstallerHelperArgs) -> UpdateInstallResult {
    let execution = execute_verified_installer(
        &args.installer_path,
        &args.expected_sha256,
        &args.install_log_path,
        args.silent,
    );
    // The helper has copied and re-verified the installer under Program Files
    // before it returns from execution. Remove only the exact staging shape
    // created by this updater; direct helper invocations must not be able to
    // turn an arbitrary --installer-path into an elevated delete primitive.
    cleanup_owned_update_staging(&args.installer_path);
    match execution {
        Ok(InstallerExecution {
            exit_code: exit_code @ (0 | 3010),
            install_log_path,
        }) => {
            let parent = install_log_path.parent().map(Path::to_path_buf);
            let _ = fs::remove_file(&install_log_path);
            if let Some(parent) = parent {
                let _ = fs::remove_dir(parent);
            }
            UpdateInstallResult {
                status: "success".to_string(),
                exit_code: Some(exit_code),
                needs_restart: exit_code == 3010,
                message: if exit_code == 3010 {
                    "更新が完了しました。Windows の再起動が必要です。".to_string()
                } else {
                    "更新が完了しました。".to_string()
                },
                completed_at: completed_at_string(),
                installer_path: Some(args.installer_path.display().to_string()),
                install_log_path: None,
            }
        }
        Ok(InstallerExecution {
            exit_code,
            install_log_path,
        }) => UpdateInstallResult {
            status: "failed".to_string(),
            exit_code: Some(exit_code),
            needs_restart: false,
            message: format!("更新に失敗しました。終了コード: {exit_code}"),
            completed_at: completed_at_string(),
            installer_path: Some(args.installer_path.display().to_string()),
            install_log_path: Some(install_log_path.display().to_string()),
        },
        Err(error) => UpdateInstallResult {
            status: "failed".to_string(),
            exit_code: None,
            needs_restart: false,
            message: error.to_string(),
            completed_at: completed_at_string(),
            installer_path: Some(args.installer_path.display().to_string()),
            install_log_path: None,
        },
    }
}

pub fn run_installer_helper_cli(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    shared::enable_redirection_guard().map_err(anyhow::Error::msg)?;
    let executable = env::current_exe().context("failed to resolve updater helper executable")?;
    let file_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !file_name.eq_ignore_ascii_case(UPDATE_HELPER_EXE_NAME) {
        return Err(anyhow!(
            "updater helper mode is only available from {UPDATE_HELPER_EXE_NAME}"
        ));
    }

    let args = parse_installer_helper_args(args)?;
    let result = helper_result(&args);
    write_update_result(&args.result_path, &result)?;
    if result.status == "success" {
        Ok(())
    } else {
        Err(anyhow!(result.message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, sync::MutexGuard};

    fn env_lock() -> MutexGuard<'static, ()> {
        crate::test_env_lock()
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
    fn cleanup_removes_only_the_owned_updater_staging_shape() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let staging = env::temp_dir().join(format!(
            "{UPDATE_DOWNLOAD_STAGING_PREFIX}{}{nonce}",
            std::process::id()
        ));
        fs::create_dir(&staging).unwrap();
        let installer = staging.join(INSTALLER_ASSET_NAME);
        fs::write(&installer, b"installer").unwrap();
        fs::write(partial_download_path(&installer), b"partial").unwrap();
        fs::write(staging.join("azookey-update-install.log"), b"log").unwrap();

        assert!(is_owned_update_staging_installer(&installer));
        cleanup_owned_update_staging(&installer);

        assert!(!staging.exists());
        assert!(!is_owned_update_staging_installer(
            &env::temp_dir().join("unrelated").join(INSTALLER_ASSET_NAME)
        ));
        assert!(!is_owned_update_staging_installer(
            &env::temp_dir()
                .join(format!("{UPDATE_DOWNLOAD_STAGING_PREFIX}123"))
                .join("unrelated.exe")
        ));
        assert!(!is_owned_update_staging_installer(
            &env::temp_dir()
                .join(format!("{UPDATE_DOWNLOAD_STAGING_PREFIX}not-a-nonce"))
                .join(INSTALLER_ASSET_NAME)
        ));
    }

    #[test]
    fn parses_protected_helper_arguments_without_losing_path_spaces() {
        let expected_hash = "a".repeat(64);
        let args = parse_installer_helper_args(
            [
                "--installer-path",
                r"C:\User Data\azookey-setup.exe",
                "--expected-sha256",
                &expected_hash,
                "--result-path",
                r"C:\User Data\update-result.json",
                "--install-log-path",
                r"C:\User Data\install.log",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();

        assert_eq!(
            args.installer_path,
            PathBuf::from(r"C:\User Data\azookey-setup.exe")
        );
        assert_eq!(args.expected_sha256, expected_hash);
        assert!(!args.silent);
        assert_eq!(
            args.result_path,
            PathBuf::from(r"C:\User Data\update-result.json")
        );
    }

    #[test]
    fn parses_silent_installer_helper_mode() {
        let expected_hash = "a".repeat(64);
        let args = parse_installer_helper_args(
            [
                "--installer-path",
                "installer.exe",
                "--expected-sha256",
                &expected_hash,
                "--result-path",
                "result.json",
                "--install-log-path",
                "install.log",
                "--silent",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap();

        assert!(args.silent);
    }

    #[test]
    fn rejects_invalid_helper_hash() {
        let error = parse_installer_helper_args(
            [
                "--installer-path",
                "installer.exe",
                "--expected-sha256",
                "not-a-hash",
                "--result-path",
                "result.json",
                "--install-log-path",
                "install.log",
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap_err();

        assert!(error.to_string().contains("SHA-256"));
    }

    #[test]
    fn hashes_the_bytes_read_from_the_locked_installer_handle() {
        let mut bytes = &b"verified installer bytes"[..];
        assert_eq!(
            sha256_from_reader(&mut bytes).unwrap(),
            "f12fa6b847f35162dbbacc2fe6870f73824c25140f77218e49b047beb434cfef"
        );
    }

    #[test]
    fn installer_lock_allows_reads_but_denies_write_and_delete_sharing() {
        assert_eq!(INSTALLER_LOCK_SHARE_MODE, 0x0000_0001);
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
        assert_eq!(
            result.message,
            "更新が完了しました。Windows の再起動が必要です。"
        );
        assert!(take_update_install_result().unwrap().is_none());
    }

    #[test]
    fn protected_update_result_takes_priority_over_malformed_legacy_file() {
        let temp = tempfile::tempdir().unwrap();
        let legacy_path = temp.path().join(UPDATE_RESULT_FILENAME);
        fs::write(&legacy_path, "not valid json").unwrap();
        let protected = r#"{
  "status": "success",
  "exit_code": 0,
  "needs_restart": false,
  "message": "updated",
  "completed_at": "2026-07-21T00:00:00Z",
  "installer_path": "protected-installer.exe",
  "install_log_path": null
}"#
        .to_string();

        let (selected, from_legacy) =
            select_update_result_data(Some(protected.clone()), &legacy_path)
                .unwrap()
                .unwrap();

        assert_eq!(selected, protected);
        assert!(!from_legacy);
    }

    #[test]
    fn update_result_recovers_legacy_failed_restart_exit_code() {
        let _env = EnvGuard::new();
        let temp = tempfile::tempdir().unwrap();
        unsafe {
            env::set_var("APPDATA", temp.path());
        }
        let path = update_result_path().unwrap();
        fs::write(
            &path,
            r#"{
  "status": "failed",
  "exit_code": 3010,
  "needs_restart": false,
  "message": "譖ｴ譁ｰ縺ｫ螟ｱ謨励＠縺ｾ縺励◆縲らｵゆｺ�繧ｳ繝ｼ繝�: 3010",
  "completed_at": "2026-05-27T00:00:00Z",
  "installer_path": "installer.exe",
  "install_log_path": "install.log"
}"#,
        )
        .unwrap();

        let result = take_update_install_result().unwrap().unwrap();

        assert_eq!(result.status, "success");
        assert_eq!(result.exit_code, Some(3010));
        assert!(result.needs_restart);
        assert_eq!(
            result.message,
            "更新が完了しました。Windows の再起動が必要です。"
        );
    }

    #[test]
    fn update_result_replaces_failed_exit_code_message() {
        let _env = EnvGuard::new();
        let temp = tempfile::tempdir().unwrap();
        unsafe {
            env::set_var("APPDATA", temp.path());
        }
        let path = update_result_path().unwrap();
        fs::write(
            &path,
            r#"{
  "status": "failed",
  "exit_code": 42,
  "needs_restart": false,
  "message": "譖ｴ譁ｰ縺ｫ螟ｱ謨励＠縺ｾ縺励◆縲らｵゆｺ�繧ｳ繝ｼ繝�: 42",
  "completed_at": "2026-05-27T00:00:00Z",
  "installer_path": "installer.exe",
  "install_log_path": "install.log"
}"#,
        )
        .unwrap();

        let result = take_update_install_result().unwrap().unwrap();

        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, Some(42));
        assert!(!result.needs_restart);
        assert_eq!(result.message, "更新に失敗しました。終了コード: 42");
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
