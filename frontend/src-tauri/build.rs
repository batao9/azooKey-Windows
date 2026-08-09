use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildInfo {
    version: String,
    channel: String,
    revision: String,
    installer_generation: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppVersion {
    version: String,
    installer_generation: u32,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> T {
    let data = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&data)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn main() {
    let crate_root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = crate_root.join("../..");
    let generated_path = repo_root.join("target/azookey-build/build-info.json");
    let source_path = repo_root.join("app-version.json");
    println!("cargo:rerun-if-changed={}", generated_path.display());
    println!("cargo:rerun-if-changed={}", source_path.display());

    let info = if generated_path.is_file() {
        read_json::<BuildInfo>(&generated_path)
    } else {
        let source = read_json::<AppVersion>(&source_path);
        BuildInfo {
            version: source.version,
            channel: "source".to_string(),
            revision: "unknown".to_string(),
            installer_generation: source.installer_generation,
        }
    };
    println!("cargo:rustc-env=AZOOKEY_BUILD_VERSION={}", info.version);
    println!("cargo:rustc-env=AZOOKEY_BUILD_CHANNEL={}", info.channel);
    println!("cargo:rustc-env=AZOOKEY_BUILD_REVISION={}", info.revision);
    println!(
        "cargo:rustc-env=AZOOKEY_INSTALLER_GENERATION={}",
        info.installer_generation
    );

    tauri_build::build()
}
