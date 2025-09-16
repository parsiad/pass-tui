use anyhow::Result;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

pub trait Backend: Send {
    fn edit(&self, entry: &str) -> Result<()>;
    fn yank(&self, entry: &str) -> Result<()>;
    fn add(&self, entry: &str) -> Result<()> {
        self.edit(entry)
    }
    fn rm(&self, target: &str, recursive: bool) -> Result<()>;
    fn show(&self, entry: &str) -> Result<String>;
    fn show_qr(&self, entry: &str) -> Result<String>;
    fn mv(&self, from: &str, to: &str) -> Result<()>;
    fn unlock(&self, _entry: &str, _qr: bool) -> Result<()> {
        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct PassCliBackend {
    pub store_dir: Option<PathBuf>,
}

impl PassCliBackend {
    pub fn new(store_dir: Option<PathBuf>) -> Self {
        Self { store_dir }
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new("pass");
        if let Some(dir) = &self.store_dir {
            cmd.env("PASSWORD_STORE_DIR", dir);
        }
        cmd
    }

    fn store_root(&self) -> PathBuf {
        self.store_dir.clone().unwrap_or_else(|| {
            dirs_next::home_dir()
                .unwrap_or_default()
                .join(".password-store")
        })
    }

    fn capture(&self, args: &[&str]) -> std::io::Result<std::process::Output> {
        let mut cmd = self.cmd();
        cmd.args(args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());
        cmd.output()
    }

    fn status_interactive(&self, args: &[&str]) -> std::io::Result<ExitStatus> {
        let mut cmd = self.cmd();
        cmd.args(args);
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::inherit());
        cmd.status()
    }

    fn capture_string(&self, args: &[&str], context: &'static str) -> Result<String> {
        let output = self.capture(args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(PassStatusError {
                context,
                status: output.status,
            }
            .into())
        }
    }
}

fn resolve_source(store: &Path, key: &str) -> Result<(PathBuf, bool)> {
    let dir = store.join(key);
    if dir.is_dir() {
        return Ok((dir, true));
    }

    let file = store.join(format!("{}.gpg", key));
    if file.is_file() {
        return Ok((file, false));
    }

    anyhow::bail!("source not found: {}", key)
}

fn destination_path(store: &Path, key: &str, is_dir: bool) -> PathBuf {
    if is_dir {
        store.join(key)
    } else {
        store.join(format!("{}.gpg", key))
    }
}

#[derive(Debug, Clone)]
pub struct PassStatusError {
    pub context: &'static str,
    pub status: ExitStatus,
}

impl fmt::Display for PassStatusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed: {}", self.context, self.status)
    }
}

impl std::error::Error for PassStatusError {}

impl Backend for PassCliBackend {
    fn edit(&self, entry: &str) -> Result<()> {
        // interactive; caller should suspend TUI before calling
        let status = self.cmd().arg("edit").arg(entry).status()?;
        if status.success() {
            return Ok(());
        }
        // pass edit returns exit code 1 when nothing changed; treat that as success
        if status.code() == Some(1) {
            return Ok(());
        }
        anyhow::bail!("pass edit failed: {status}")
    }

    fn yank(&self, entry: &str) -> Result<()> {
        // suppress pass output in TUI
        let status = self
            .cmd()
            .arg("-c")
            .arg(entry)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("pass -c failed: {status}")
        }
    }

    fn rm(&self, target: &str, recursive: bool) -> Result<()> {
        let mut cmd = self.cmd();
        cmd.arg("rm");
        if recursive {
            cmd.arg("-r");
        }
        cmd.arg("-f"); // confirm in TUI, force in pass
        let status = cmd
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("pass rm failed: {status}")
        }
    }

    fn show(&self, entry: &str) -> Result<String> {
        // Use plain `pass <entry>` to print raw contents
        let args = [entry];
        self.capture_string(&args, "pass show")
    }

    fn show_qr(&self, entry: &str) -> Result<String> {
        // Use `pass show -q <entry>` to produce QR (if supported by user's pass setup)
        let args = ["show", "-q", entry];
        self.capture_string(&args, "pass show -q")
    }

    fn mv(&self, from: &str, to: &str) -> Result<()> {
        let store = self.store_root();
        let (src, is_dir) = resolve_source(&store, from)?;
        let dst = destination_path(&store, to, is_dir);

        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        // Prevent overwriting existing destination
        if dst.exists() {
            anyhow::bail!("destination exists: {}", to);
        }
        fs::rename(&src, &dst)?;
        Ok(())
    }

    fn unlock(&self, entry: &str, qr: bool) -> Result<()> {
        let (args, context): (Vec<&str>, &str) = if qr {
            (vec!["show", "-q", entry], "pass show -q")
        } else {
            (vec![entry], "pass show")
        };
        let status = self.status_interactive(&args)?;
        if status.success() {
            Ok(())
        } else {
            Err(PassStatusError { context, status }.into())
        }
    }
}
