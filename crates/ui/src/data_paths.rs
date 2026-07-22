use std::{env, ffi::OsStr, path::PathBuf};

const APP_DIRECTORY_NAME: &str = "Azookey";
const UI_WEBVIEW_DIRECTORY_NAME: &str = "ui-webview";

pub(crate) fn ui_webview_data_directory() -> PathBuf {
    resolve_ui_webview_data_directory(
        env::var_os("LOCALAPPDATA").as_deref(),
        env::var_os("APPDATA").as_deref(),
        env::temp_dir(),
    )
}

fn resolve_ui_webview_data_directory(
    local_app_data: Option<&OsStr>,
    app_data: Option<&OsStr>,
    temporary_directory: PathBuf,
) -> PathBuf {
    let root = [local_app_data, app_data]
        .into_iter()
        .flatten()
        .find(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or(temporary_directory);

    root.join(APP_DIRECTORY_NAME)
        .join(UI_WEBVIEW_DIRECTORY_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_data_prefers_local_app_data() {
        let path = resolve_ui_webview_data_directory(
            Some(OsStr::new("/users/test/local")),
            Some(OsStr::new("/users/test/roaming")),
            PathBuf::from("/tmp"),
        );

        assert_eq!(path, PathBuf::from("/users/test/local/Azookey/ui-webview"));
    }

    #[test]
    fn webview_data_falls_back_to_roaming_app_data() {
        let path = resolve_ui_webview_data_directory(
            None,
            Some(OsStr::new("/users/test/roaming")),
            PathBuf::from("/tmp"),
        );

        assert_eq!(
            path,
            PathBuf::from("/users/test/roaming/Azookey/ui-webview")
        );
    }

    #[test]
    fn webview_data_falls_back_to_temporary_directory() {
        let install_directory = PathBuf::from("/program-files/Azookey");
        let path = resolve_ui_webview_data_directory(
            Some(OsStr::new("")),
            None,
            PathBuf::from("/users/test/temp"),
        );

        assert_eq!(path, PathBuf::from("/users/test/temp/Azookey/ui-webview"));
        assert!(!path.starts_with(install_directory));
    }
}
