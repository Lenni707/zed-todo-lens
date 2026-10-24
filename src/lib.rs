use std::fs;

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

const SERVER_NAME: &str = "todo-lens";
const BINARY_NAME: &str = "todo-lens-ls";
const GITHUB_REPO: &str = "Lenni707/zed-todo-lens";

struct TodoLensExtension {
    cached_binary_path: Option<String>,
}

impl TodoLensExtension {
    fn server_binary(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<String> {
        // 1. Explicit path from settings: lsp.todo-lens.binary.path
        if let Some(path) = LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|s| s.binary)
            .and_then(|b| b.path)
        {
            return Ok(path);
        }

        // 2. A binary on PATH (handy while developing).
        if let Some(path) = worktree.which(BINARY_NAME) {
            return Ok(path);
        }

        // 3. Previously downloaded binary.
        if let Some(path) = &self.cached_binary_path {
            if fs::metadata(path).is_ok_and(|m| m.is_file()) {
                return Ok(path.clone());
            }
        }

        // 4. Download from the GitHub release.
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = zed::latest_github_release(
            GITHUB_REPO,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let (os, arch) = zed::current_platform();
        let arch = match arch {
            zed::Architecture::Aarch64 => "aarch64",
            zed::Architecture::X8664 => "x86_64",
            zed::Architecture::X86 => return Err("x86 is not supported".into()),
        };
        let (os, ext, file_type) = match os {
            zed::Os::Mac => ("apple-darwin", "tar.gz", zed::DownloadedFileType::GzipTar),
            zed::Os::Linux => ("unknown-linux-gnu", "tar.gz", zed::DownloadedFileType::GzipTar),
            zed::Os::Windows => ("pc-windows-msvc", "zip", zed::DownloadedFileType::Zip),
        };
        let asset_name = format!("{BINARY_NAME}-{arch}-{os}.{ext}");
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == asset_name)
            .ok_or_else(|| format!("no release asset named {asset_name}"))?;

        let version_dir = format!("{BINARY_NAME}-{}", release.version);
        let exe = if matches!(zed::current_platform().0, zed::Os::Windows) {
            format!("{BINARY_NAME}.exe")
        } else {
            BINARY_NAME.to_string()
        };
        let binary_path = format!("{version_dir}/{exe}");

        if !fs::metadata(&binary_path).is_ok_and(|m| m.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::download_file(&asset.download_url, &version_dir, file_type)
                .map_err(|e| format!("failed to download {asset_name}: {e}"))?;
            zed::make_file_executable(&binary_path)?;

            // Remove older versions.
            if let Ok(entries) = fs::read_dir(".") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with(BINARY_NAME) && name != version_dir {
                        fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        self.cached_binary_path = Some(binary_path.clone());
        Ok(binary_path)
    }

    fn user_settings(worktree: &zed::Worktree) -> Option<zed::serde_json::Value> {
        LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|s| s.settings)
    }
}

impl zed::Extension for TodoLensExtension {
    fn new() -> Self {
        Self {
            cached_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER_NAME, worktree)
            .ok()
            .and_then(|s| s.binary);
        let args = binary
            .as_ref()
            .and_then(|b| b.arguments.clone())
            .unwrap_or_default();
        let env = binary
            .and_then(|b| b.env)
            .map(|env| env.into_iter().collect())
            .unwrap_or_default();

        Ok(zed::Command {
            command: self.server_binary(language_server_id, worktree)?,
            args,
            env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(Self::user_settings(worktree))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(Self::user_settings(worktree))
    }
}

zed::register_extension!(TodoLensExtension);
