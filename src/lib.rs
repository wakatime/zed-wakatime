use std::{
    fs,
    path::{Path, PathBuf},
};

use zed_extension_api::{self as zed, Command, LanguageServerId, Result, Worktree};

struct WakatimeExtension {
    cached_ls_binary_path: Option<PathBuf>,
    cached_wakatime_cli_binary_path: Option<PathBuf>,
}

fn sanitize_path(path: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => path.trim_start_matches("/").to_string(),
        _ => path.to_string(),
    }
}

fn executable_name(binary: &str) -> String {
    match zed::current_platform() {
        (zed::Os::Windows, _) => format!("{}.exe", binary),
        _ => binary.to_string(),
    }
}

impl WakatimeExtension {
    fn target_triple(&self, binary: &str) -> Result<String, String> {
        let (platform, arch) = zed::current_platform();
        let (arch, os) = {
            let arch = match arch {
                zed::Architecture::Aarch64 if binary == "wakatime-cli" => "arm64",
                zed::Architecture::Aarch64 if binary == "wakatime-ls" => "aarch64",
                zed::Architecture::X8664 if binary == "wakatime-cli" => "amd64",
                zed::Architecture::X8664 if binary == "wakatime-ls" => "x86_64",
                _ => return Err(format!("unsupported architecture: {arch:?}")),
            };

            let os = match platform {
                zed::Os::Mac if binary == "wakatime-cli" => "darwin",
                zed::Os::Mac if binary == "wakatime-ls" => "apple-darwin",
                zed::Os::Linux if binary == "wakatime-cli" => "linux",
                zed::Os::Linux if binary == "wakatime-ls" => "unknown-linux-gnu",
                zed::Os::Windows if binary == "wakatime-cli" => "windows",
                zed::Os::Windows if binary == "wakatime-ls" => "pc-windows-msvc",
                _ => return Err("unsupported platform".to_string()),
            };

            (arch, os)
        };

        Ok(match binary {
            "wakatime-cli" => format!("{binary}-{os}-{arch}"),
            _ => format!("{binary}-{arch}-{os}"),
        })
    }

    fn download(
        &self,
        language_server_id: &LanguageServerId,
        binary: &str,
        repo: &str,
    ) -> Result<PathBuf> {
        let release = zed::latest_github_release(
            repo,
            zed::GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        )?;

        let target_triple = self.target_triple(binary)?;

        let asset_name = format!("{target_triple}.zip");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| format!("no asset found matching {:?}", asset_name))?;

        let version_dir = format!("{binary}-{}", release.version);
        let binary_path = if binary == "wakatime-cli" {
            Path::new(&version_dir).join(executable_name(&target_triple))
        } else {
            Path::new(&version_dir).join(executable_name(&binary))
        };

        if !fs::metadata(&binary_path).map_or(false, |stat| stat.is_file()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );

            zed::download_file(
                &asset.download_url,
                &version_dir,
                zed::DownloadedFileType::Zip,
            )
            .map_err(|err| format!("failed to download file: {err}"))?;

            let entries = fs::read_dir(".")
                .map_err(|err| format!("failed to list working directory {err}"))?;

            for entry in entries {
                let entry = entry.map_err(|err| format!("failed to load directory entry {err}"))?;
                if let Some(file_name) = entry.file_name().to_str() {
                    if file_name.starts_with(binary) && file_name != version_dir {
                        fs::remove_dir_all(entry.path()).ok();
                    }
                }
            }
        }

        zed::make_file_executable(binary_path.to_str().unwrap())?;

        Ok(binary_path)
    }

    fn language_server_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<PathBuf, String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        if let Some(path) = worktree.which(&executable_name("wakatime-ls")) {
            return Ok(path.into());
        }

        let target_triple = self.target_triple("wakatime-ls")?;
        if let Some(path) = worktree.which(&executable_name(&target_triple)) {
            return Ok(path.into());
        }

        if let Some(path) = &self.cached_ls_binary_path {
            if fs::metadata(path).map_or(false, |stat| stat.is_file()) {
                return Ok(path.into());
            }
        }

        let binary_path =
            self.download(language_server_id, "wakatime-ls", "wakatime/zed-wakatime")?;

        self.cached_ls_binary_path = Some(binary_path.clone());

        Ok(binary_path)
    }

    fn wakatime_cli_binary_path(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<PathBuf, String> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        if let Some(path) = worktree.which(&executable_name("wakatime-cli")) {
            return Ok(path.into());
        }

        if let Some(path) = &self.cached_wakatime_cli_binary_path {
            if fs::metadata(path).map_or(false, |stat| stat.is_file()) {
                return Ok(path.into());
            }
        }

        let binary_path =
            self.download(language_server_id, "wakatime-cli", "wakatime/wakatime-cli")?;

        self.cached_wakatime_cli_binary_path = Some(binary_path.clone());

        Ok(binary_path)
    }
}

impl zed::Extension for WakatimeExtension {
    fn new() -> Self {
        Self {
            cached_ls_binary_path: None,
            cached_wakatime_cli_binary_path: None,
        }
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Command> {
        let wakatime_cli_binary_path =
            self.wakatime_cli_binary_path(language_server_id, worktree)?;

        let ls_binary_path = self.language_server_binary_path(language_server_id, worktree)?;

        let args = vec!["--wakatime-cli".to_string(), {
            use std::env;
            let current = env::current_dir().unwrap();
            let waka_cli = current
                .join(wakatime_cli_binary_path)
                .to_str()
                .unwrap()
                .to_string();

            sanitize_path(waka_cli.as_str())
        }];

        Ok(Command {
            args,
            command: ls_binary_path.to_str().unwrap().to_owned(),
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(WakatimeExtension);
