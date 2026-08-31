use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use push_notify::ServerChan;

pub(crate) const CREDENTIAL_NAME: &str = "serverchan-sendkey";

const SERVICE_NAME: &str = "thermal-cli.service";
const BINARY_PATH: &str = "/usr/local/bin/thermal-cli";
const SERVICE_PATH: &str = "/etc/systemd/system/thermal-cli.service";
const CREDENTIAL_PATH: &str = "/etc/thermal-cli/serverchan-sendkey";

pub(crate) struct Options<'a> {
    pub(crate) no_start: bool,
    pub(crate) sendkey_file: Option<&'a Path>,
}

pub(crate) fn run(options: Options<'_>) -> Result<()> {
    let paths = InstallPaths::system();
    let executable = std::env::current_exe().context("failed to locate current executable")?;
    let sendkey = load_sendkey(&options)?;

    install_binary(&executable, &paths.binary)?;
    if let Some(sendkey) = &sendkey {
        atomic_write(&paths.credential, sendkey.as_bytes(), 0o600)?;
    } else {
        remove_credential(&paths.credential)?;
    }
    let credential = sendkey.as_ref().map(|_| paths.credential.as_path());
    let unit = service_unit(&paths.binary, credential);
    atomic_write(&paths.service, unit.as_bytes(), 0o644)?;

    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", SERVICE_NAME])?;
    if !options.no_start {
        systemctl(&["restart", SERVICE_NAME])?;
    }

    println!("Installed {}", paths.binary.display());
    println!("Installed {}", paths.service.display());
    if sendkey.is_some() {
        println!("ServerChan notifications enabled");
    } else {
        println!("Notifications disabled");
    }
    if options.no_start {
        println!("Enabled {SERVICE_NAME}");
    } else {
        println!("Enabled and started {SERVICE_NAME}");
    }
    Ok(())
}

struct InstallPaths {
    binary: PathBuf,
    service: PathBuf,
    credential: PathBuf,
}

impl InstallPaths {
    fn system() -> Self {
        Self {
            binary: PathBuf::from(BINARY_PATH),
            service: PathBuf::from(SERVICE_PATH),
            credential: PathBuf::from(CREDENTIAL_PATH),
        }
    }
}

fn load_sendkey(options: &Options<'_>) -> Result<Option<String>> {
    load_sendkey_with(options, || {
        rpassword::prompt_password("ServerChan SendKey (empty disables notifications): ")
            .context("failed to read ServerChan SendKey from terminal")
    })
}

fn load_sendkey_with<F>(options: &Options<'_>, prompt: F) -> Result<Option<String>>
where
    F: FnOnce() -> Result<String>,
{
    let value = if let Some(path) = options.sendkey_file {
        fs::read_to_string(path)
            .with_context(|| format!("failed to read SendKey from {}", path.display()))?
    } else {
        prompt()?
    };

    let value = value.trim().to_owned();
    if value.is_empty() {
        return Ok(None);
    }
    ServerChan::new(&value).context("invalid ServerChan SendKey")?;
    Ok(Some(value))
}

fn remove_credential(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to remove credential {}; run install as root",
                path.display()
            )
        }),
    }
}

fn install_binary(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        return Ok(());
    }
    let parent = destination
        .parent()
        .context("binary destination has no parent directory")?;
    create_directory(parent)?;
    let temporary = temporary_path(destination);
    let result = (|| {
        fs::copy(source, &temporary).with_context(|| {
            format!(
                "failed to copy binary to {}; run install as root",
                temporary.display()
            )
        })?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
        fs::rename(&temporary, destination).with_context(|| {
            format!(
                "failed to install binary at {}; run install as root",
                destination.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn atomic_write(path: &Path, content: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .context("installation path has no parent directory")?;
    create_directory(parent)?;
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)
            .with_context(|| {
                format!(
                    "failed to create {}; run install as root",
                    temporary.display()
                )
            })?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path).with_context(|| {
            format!("failed to install {}; run install as root", path.display())
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| {
        format!(
            "failed to create directory {}; run install as root",
            path.display()
        )
    })
}

fn temporary_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("thermal-cli");
    destination.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

fn service_unit(binary: &Path, credential: Option<&Path>) -> String {
    let mut unit = format!(
        "[Unit]\n\
         Description=Hardware temperature monitor and notifier\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --daemon\n",
        binary.display()
    );
    if let Some(credential) = credential {
        unit.push_str(&format!(
            "LoadCredential={CREDENTIAL_NAME}:{}\n",
            credential.display()
        ));
    }
    unit.push_str(
        "Restart=on-failure\n\
         RestartSec=5s\n\
         NoNewPrivileges=yes\n\
         PrivateTmp=yes\n\
         ProtectSystem=strict\n\
         ProtectHome=yes\n\
         ProtectKernelTunables=yes\n\
         ProtectKernelModules=yes\n\
         ProtectControlGroups=yes\n\
         RestrictSUIDSGID=yes\n\
         LockPersonality=yes\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
    );
    unit
}

fn systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .args(arguments)
        .status()
        .with_context(|| format!("failed to execute systemctl {}", arguments.join(" ")))?;
    if !status.success() {
        bail!(
            "systemctl {} failed with status {}",
            arguments.join(" "),
            status
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "thermal-cli-install-test-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unit_runs_daemon_with_a_systemd_credential() {
        let unit = service_unit(
            Path::new("/usr/local/bin/thermal-cli"),
            Some(Path::new("/etc/thermal-cli/serverchan-sendkey")),
        );

        assert!(unit.contains("ExecStart=/usr/local/bin/thermal-cli --daemon"));
        assert!(
            unit.contains("LoadCredential=serverchan-sendkey:/etc/thermal-cli/serverchan-sendkey")
        );
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("NoNewPrivileges=yes"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn unit_omits_credential_when_notifications_are_disabled() {
        let unit = service_unit(Path::new("/usr/local/bin/thermal-cli"), None);

        assert!(unit.contains("ExecStart=/usr/local/bin/thermal-cli --daemon"));
        assert!(!unit.contains("LoadCredential="));
    }

    #[test]
    fn writes_credentials_with_owner_only_permissions() {
        let root = TestDir::new();
        let path = root.0.join("credential");

        atomic_write(&path, b"SCT123abc", 0o600).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "SCT123abc");
    }

    #[test]
    fn removes_existing_credential_when_notifications_are_disabled() {
        let root = TestDir::new();
        let path = root.0.join("credential");
        fs::write(&path, "SCTold123").unwrap();

        remove_credential(&path).unwrap();
        remove_credential(&path).unwrap();

        assert!(!path.exists());
    }

    #[test]
    fn loads_sendkey_from_file_without_prompting() {
        let root = TestDir::new();
        let source = root.0.join("source");
        fs::write(&source, "SCTfile123\n").unwrap();
        let options = Options {
            no_start: true,
            sendkey_file: Some(&source),
        };

        let value = load_sendkey_with(&options, || panic!("prompt should not be called")).unwrap();

        assert_eq!(value.as_deref(), Some("SCTfile123"));
    }

    #[test]
    fn empty_prompt_disables_notifications() {
        let options = Options {
            no_start: true,
            sendkey_file: None,
        };

        let value = load_sendkey_with(&options, || Ok("  \n".to_owned())).unwrap();

        assert_eq!(value, None);
    }
}
