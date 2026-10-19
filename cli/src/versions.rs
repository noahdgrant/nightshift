//! The versions of `ns` installed on this machine, one binary each. A repository is pinned to one
//! (`.nightshift/nightshift.toml`), and whichever `ns` is run hands over to it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::forge::Forge;
use crate::os::Os;

/// Where releases of `ns` are published.
const SOURCE: &str = "noahdgrant/nightshift";

/// Where installed versions live: `$NS_HOME`, or `~/.local/share/nightshift`.
pub fn home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("NS_HOME") {
        return Ok(PathBuf::from(home));
    }
    let user = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(Path::new(&user).join(".local/share/nightshift"))
}

fn check(version: &str) -> Result<()> {
    let ok = !version.is_empty()
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        && !version.starts_with('.');
    if !ok {
        bail!("{version:?} is not a version");
    }
    Ok(())
}

/// The binary of `version`, whether or not it is installed.
pub fn binary(home: &Path, version: &str) -> Result<PathBuf> {
    check(version)?;
    Ok(home.join("versions").join(version).join("ns"))
}

fn asset(version: &str) -> String {
    format!("ns-{version}-x86_64-unknown-linux-gnu.tar.gz")
}

/// Downloads `version` from its release, checks it against the checksum published with it and
/// installs it. Does nothing when it is installed.
pub fn install(home: &Path, version: &str, forge: &dyn Forge, os: &dyn Os) -> Result<PathBuf> {
    let target = binary(home, version)?;
    if target.is_file() {
        return Ok(target);
    }
    let tmp = home.join(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = fetch(&tmp, version, &target, forge, os);
    let _ = std::fs::remove_dir_all(&tmp);
    result.map(|()| target)
}

fn fetch(tmp: &Path, version: &str, target: &Path, forge: &dyn Forge, os: &dyn Os) -> Result<()> {
    let tarball = asset(version);
    let checksum = format!("{tarball}.sha256");
    forge
        .release_download(SOURCE, &format!("v{version}"), &[&tarball, &checksum], tmp)
        .with_context(|| format!("cannot download ns {version}"))?;
    let expected = std::fs::read_to_string(tmp.join(&checksum))?;
    let expected = expected.split_whitespace().next().unwrap_or_default();
    let digest = Sha256::digest(std::fs::read(tmp.join(&tarball))?);
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != expected {
        bail!("{tarball} does not match its checksum; it was not installed");
    }
    let mut untar = os.shell_command("tar -xzf \"$0\" -C \"$1\"");
    untar.arg(tmp.join(&tarball)).arg(tmp);
    if os
        .process_run(untar, b"", Duration::from_secs(120))?
        .success
        != Some(true)
    {
        bail!("cannot unpack {tarball}");
    }
    let unpacked = tmp.join(tarball.trim_end_matches(".tar.gz")).join("ns");
    std::fs::create_dir_all(target.parent().context("a binary has no directory")?)?;
    std::fs::rename(&unpacked, target)
        .with_context(|| format!("cannot install {}", target.display()))
}
