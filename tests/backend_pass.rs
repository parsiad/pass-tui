use assert_fs::prelude::*;
use assert_fs::TempDir;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn pass_cli_backend_invokes_pass_commands() -> anyhow::Result<()> {
    use pass_tui::backend::{Backend, PassCliBackend};

    // Create a fake pass in PATH that logs arguments
    let tmp = TempDir::new()?;
    let bin_dir = tmp.child("bin");
    bin_dir.create_dir_all()?;
    let log = tmp.child("log.txt");
    let pass_path = bin_dir.child("pass");
    pass_path.write_str(&format!(
        "#!/bin/sh\necho \"$@\" >> {}\nexit 0\n",
        log.path().display()
    ))?;
    let mut perms = pass_path.metadata()?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(pass_path.path(), perms)?;

    // Prepend our fake bin dir to PATH
    let orig_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", bin_dir.path().display(), orig_path);
    std::env::set_var("PATH", &new_path);

    let backend = PassCliBackend::default();
    backend.edit("foo/bar")?;
    backend.yank("foo/bar")?;
    backend.rm("foo/bar", false)?;

    log.assert(predicate::str::contains("edit foo/bar"));
    log.assert(predicate::str::contains("-c foo/bar"));
    log.assert(predicate::str::contains("rm -f foo/bar"));
    Ok(())
}
