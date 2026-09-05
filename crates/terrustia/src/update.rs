//! `terrustia update`: checking for, and applying, a new release.
//!
//! Two separate things live here, deliberately kept apart:
//!
//! - [`boot_check`] runs automatically on every boot, in the background. It checks GitHub for a
//!   newer release, verifies the checksums manifest is genuinely signed by this project's own
//!   release pipeline, and — only then — leaves a message for the console log and, once a
//!   recognised admin next signs in, the game itself. It never downloads a full binary and never
//!   applies anything.
//! - [`run_update_command`] is what `terrustia update` on the command line actually does: the same
//!   check, then (unless `--check` was given) downloading, verifying and installing the matching
//!   platform build. This is the *manual apply* — an operator runs it on purpose. A running
//!   production server is never restarted out from under itself; see `packaging/terrustia.service`
//!   and this crate's `main.rs` for why that stays a human decision everywhere else in this
//!   project too.
//!
//! **The trust chain is not reinvented here.** `.github/workflows/release.yml` already signs every
//! release asset — including `SHA256SUMS` itself — with `cosign sign-blob`, keyless, using the
//! GitHub Actions workflow's own OIDC identity. Verifying that signature means shelling out to the
//! real `cosign` binary with the exact `--certificate-identity-regexp`/`--certificate-oidc-issuer`
//! invocation `release.yml`'s own published release notes already tell a human to run by hand —
//! not re-implementing Sigstore bundle verification (certificate chains, Rekor inclusion proofs,
//! CT log checks) in this crate. If `cosign` is not installed, update checks are refused outright
//! rather than silently downgrading to "checked but unverified": see [`UpdateError::CosignMissing`].

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::Deserialize;
use tracing::{debug, info, warn};

/// The repository real releases are published to. Matches `release.yml`'s own
/// `--certificate-identity-regexp 'https://github.com/${{ github.repository }}/'` — the same
/// string, not a value that could quietly drift from it.
pub const REPO: &str = "bybrooklyn/terrustia";

const API_BASE: &str = "https://api.github.com";
const OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// A release found on GitHub, confirmed newer than this build. Nothing about it has been
/// downloaded or verified yet past its manifest of checksums — see [`check`] for exactly what
/// "confirmed" means at this stage, and [`apply`] for the rest.
pub struct AvailableUpdate {
    pub tag: String,
}

/// What [`apply`] actually did.
pub enum ApplyOutcome {
    /// No newer, verifiable release exists. Not an error — this is the ordinary answer, and
    /// `terrustia update` is meant to be safe to run any time out of habit.
    AlreadyUpToDate,
    /// Downloaded, signature-verified, and swapped in over the running binary's own file.
    Applied { tag: String, path: PathBuf },
    /// Downloaded and signature-verified, but this platform cannot safely replace its own running
    /// executable image the way Unix's atomic rename does — see the `apply` doc comment on the
    /// `cfg(not(unix))` branch for why this is a disclosed gap rather than a guess.
    DownloadedForManualApply { tag: String, path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("checking for a new release: {0}")]
    Network(String),
    #[error("reading the release list: {0}")]
    Parse(String),
    #[error(
        "cosign is not installed, so a release's signature cannot be checked. Install it \
         (https://docs.sigstore.dev/system_config/installation/) to enable `terrustia update`"
    )]
    CosignMissing,
    #[error("verifying the release signature: {0}")]
    Verify(String),
    #[error("no build of this release exists for {os}/{arch}")]
    NoAsset { os: String, arch: String },
    #[error("{0}")]
    Download(String),
    #[error("extracting the downloaded release: {0}")]
    Extract(String),
    #[error("replacing the running binary: {0}")]
    Apply(String),
}

#[derive(Debug, Deserialize)]
struct RawRelease {
    tag_name: String,
    assets: Vec<RawAsset>,
}

#[derive(Debug, Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}

impl RawRelease {
    fn asset_url(&self, name: &str) -> Result<&str, UpdateError> {
        self.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.as_str())
            .ok_or_else(|| {
                UpdateError::Parse(format!(
                    "release {} has no asset named {name}",
                    self.tag_name
                ))
            })
    }
}

/// Parses `major.minor.patch`, the only shape this project's own tags ever take (see
/// `Cargo.toml`'s `[workspace.package]` comment on why `0.0.1` rather than `0.1.0`). Anything else
/// — a prerelease suffix, a missing component, a bare commit — comes back `None` rather than
/// guessing, so [`is_newer`] fails closed on a tag it cannot understand instead of misreading it.
fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let tag = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = tag.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn is_newer(remote_tag: &str, current: &str) -> bool {
    match (parse_version(remote_tag), parse_version(current)) {
        (Some(remote), Some(current)) => remote > current,
        // An unparseable tag is refused rather than assumed newer — the whole point of this
        // check is to only ever say "yes" when it actually knows, since saying it wrongly is what
        // gets an admin to run `terrustia update` for nothing.
        _ => false,
    }
}

/// Which cosign flags prove a signer's identity. Production always verifies against this
/// project's real, keyless GitHub Actions signing chain ([`Identity::Keyless`]) — the only
/// variant `check`/`apply` ever construct. Tests use [`Identity::Key`] instead, because minting a
/// real Fulcio-issued keyless certificate needs a live OIDC token from GitHub or Google that
/// nothing in this sandboxed environment can produce non-interactively; the subprocess call this
/// drives, and everything this module does with its exit code, is identical either way — only the
/// flags differ. This module's own tests separately prove the *keyless* flags are the right
/// ones, against a real, independently-signed public artifact (see the tests' own doc comment,
/// right above the fixture tests below, for exactly what was run and what it proved).
enum Identity<'a> {
    Keyless {
        regexp: &'a str,
        issuer: &'a str,
    },
    #[cfg(test)]
    Key {
        public_key: &'a Path,
    },
}

impl Identity<'_> {
    fn apply_to(&self, cmd: &mut Command) {
        match self {
            Identity::Keyless { regexp, issuer } => {
                cmd.arg("--certificate-identity-regexp")
                    .arg(regexp)
                    .arg("--certificate-oidc-issuer")
                    .arg(issuer);
            }
            #[cfg(test)]
            Identity::Key { public_key } => {
                cmd.arg("--key").arg(public_key);
            }
        }
    }
}

/// Whether a binary named `name` can be spawned at all — `Command::spawn`/`output` only fails with
/// an `io::Error` (`NotFound`, typically) when the executable itself cannot be found or run; a
/// program that runs and exits non-zero still yields `Ok`. Split out from [`cosign_available`] so
/// this module's own tests can prove the detection itself works both ways — a name nothing on
/// `PATH` provides really does come back `false` — without needing to touch the real process-wide
/// `PATH` (which every concurrently running test shares) just to prove one boolean.
fn binary_available(name: &str) -> bool {
    Command::new(name).arg("version").output().is_ok()
}

fn cosign_available() -> bool {
    binary_available("cosign")
}

/// Shells out to the real `cosign` binary — see this module's own doc comment for why that is the
/// verifier rather than a Rust reimplementation of Sigstore bundle checking.
fn verify_blob(bundle: &Path, file: &Path, identity: &Identity) -> Result<(), UpdateError> {
    let mut cmd = Command::new("cosign");
    cmd.arg("verify-blob").arg("--bundle").arg(bundle);
    identity.apply_to(&mut cmd);
    cmd.arg(file);
    let output = cmd
        .output()
        .map_err(|e| UpdateError::Verify(format!("running cosign: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(UpdateError::Verify(stderr.trim().to_string()))
    }
}

fn build_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!("terrustia-update/", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(config)
}

/// `Ok(None)` when the repository genuinely has no release yet (`bybrooklyn/terrustia` does not,
/// as of this writing — TODO.md's own Phase 2 qualification gates are still open, ahead of tagging
/// v0.0.1 itself), which is the ordinary case today and not an error to warn about on every boot.
fn fetch_latest_release(
    agent: &ureq::Agent,
    api_base: &str,
    repo: &str,
) -> Result<Option<RawRelease>, UpdateError> {
    let url = format!("{api_base}/repos/{repo}/releases/latest");
    let result = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut response = match result {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(UpdateError::Network(e.to_string())),
    };
    response
        .body_mut()
        .read_json::<RawRelease>()
        .map(Some)
        .map_err(|e| UpdateError::Parse(e.to_string()))
}

fn download_to_file(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<(), UpdateError> {
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| UpdateError::Download(format!("downloading {url}: {e}")))?;
    let mut file = std::fs::File::create(dest)
        .map_err(|e| UpdateError::Download(format!("writing {}: {e}", dest.display())))?;
    std::io::copy(&mut response.body_mut().as_reader(), &mut file)
        .map_err(|e| UpdateError::Download(format!("writing {}: {e}", dest.display())))?;
    Ok(())
}

/// A private scratch directory for downloaded/extracted release files — unpredictable, created
/// atomically, and cleaned up automatically ([`tempfile::TempDir`]'s `Drop`) whether the caller
/// returns early or not.
///
/// This function's first draft used `std::env::temp_dir().join(format!("...-{label}-{pid}"))` —
/// predictable, under a shared, world-writable `/tmp`. Another local user on the same machine
/// (exactly this session's own dev environment) could pre-create that same directory, or a
/// symlink at one of the fixed filenames written inside it (`SHA256SUMS`, the release archive),
/// before this process got there: `create_dir_all` silently reuses whatever — or whoever's —
/// directory already sits at that name, and every subsequent write follows a pre-placed symlink
/// wherever it points, a real arbitrary-file-write primitive scoped to this process's own write
/// permissions. Found in review, fixed here — `tempfile` is exactly the one narrow job this is:
/// an unpredictable name, created with retry-on-collision rather than reuse, and (belt and
/// suspenders, on top of that) restricted to the owner on Unix.
fn scratch_dir(label: &str) -> Result<tempfile::TempDir, UpdateError> {
    let prefix = format!("terrustia-update-{label}-");
    let mut builder = tempfile::Builder::new();
    builder.prefix(&prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .map_err(|e| UpdateError::Download(format!("creating a scratch directory: {e}")))
}

/// `(target triple, is a .zip archive)` for the release asset matching this machine, matching
/// `release.yml`'s own five-target build matrix exactly.
fn target_triple() -> Option<(&'static str, bool)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-musl", false)),
        ("linux", "aarch64") => Some(("aarch64-unknown-linux-musl", false)),
        ("macos", "x86_64") => Some(("x86_64-apple-darwin", false)),
        ("macos", "aarch64") => Some(("aarch64-apple-darwin", false)),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", true)),
        // `release.yml` has published this since Windows ARM64 became a release target, and this
        // arm did not exist, so the one machine class that had a binary waiting for it was told
        // there was no build for its platform. Found by `a_target_triple_exists_for_this_machine`
        // the first time this suite ran on that machine, which is the drift that test is for.
        ("windows", "aarch64") => Some(("aarch64-pc-windows-msvc", true)),
        _ => None,
    }
}

/// Check for a newer, signature-verified release. Downloads only `SHA256SUMS` and its cosign
/// bundle — a few kilobytes — never a full binary; see the module doc comment for why applying is
/// a separate, deliberate step.
///
/// `identity` is always [`Identity::Keyless`] in production ([`boot_check`] is the only real
/// caller) — threaded through as a parameter, rather than built internally, so this whole
/// function is exercised for real by this module's own tests against a real, locally-signed
/// fixture (`Identity::Key`), not just against production's specific flags.
fn check(
    agent: &ureq::Agent,
    api_base: &str,
    repo: &str,
    current_version: &str,
    identity: &Identity,
) -> Result<Option<AvailableUpdate>, UpdateError> {
    let Some(release) = fetch_latest_release(agent, api_base, repo)? else {
        return Ok(None);
    };
    if !is_newer(&release.tag_name, current_version) {
        return Ok(None);
    }
    if !cosign_available() {
        return Err(UpdateError::CosignMissing);
    }

    let sums_url = release.asset_url("SHA256SUMS")?.to_string();
    let bundle_url = release.asset_url("SHA256SUMS.cosign.bundle")?.to_string();

    let dir = scratch_dir("check")?;
    let sums_path = dir.path().join("SHA256SUMS");
    let bundle_path = dir.path().join("SHA256SUMS.cosign.bundle");
    download_to_file(agent, &sums_url, &sums_path)?;
    download_to_file(agent, &bundle_url, &bundle_path)?;

    // `dir` (a `TempDir`) removes itself on drop — on this early return via `?` just as much as
    // on the ordinary path below — so there is nothing left to clean up by hand here.
    verify_blob(&bundle_path, &sums_path, identity)?;

    Ok(Some(AvailableUpdate {
        tag: release.tag_name,
    }))
}

fn identity_regexp(repo: &str) -> String {
    format!("https://github.com/{repo}/")
}

/// Download, verify, and install the matching platform build of the latest release — the "manual
/// apply" an operator triggers by running `terrustia update` themselves. Never called from
/// [`boot_check`]. See [`check`]'s doc comment for why `identity` is a parameter rather than
/// built inside this function.
///
/// `install_target` is the file [`install`] ends up replacing — always `current_exe()` in
/// production (see [`apply_current`]). Threaded through as a parameter, rather than resolved
/// inside this function, so this module's own tests can drive the *entire* fetch/verify/extract/
/// install pipeline for real without replacing the test binary's own file out from under the test
/// run — see this module's tests for what that proves and why it matters.
fn apply(
    agent: &ureq::Agent,
    api_base: &str,
    repo: &str,
    current_version: &str,
    identity: &Identity,
    install_target: &Path,
) -> Result<ApplyOutcome, UpdateError> {
    let Some(release) = fetch_latest_release(agent, api_base, repo)? else {
        return Ok(ApplyOutcome::AlreadyUpToDate);
    };
    if !is_newer(&release.tag_name, current_version) {
        return Ok(ApplyOutcome::AlreadyUpToDate);
    }
    if !cosign_available() {
        return Err(UpdateError::CosignMissing);
    }

    let (target, is_zip) = target_triple().ok_or_else(|| UpdateError::NoAsset {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })?;
    let ext = if is_zip { "zip" } else { "tar.gz" };
    let inner_name = format!("terrustia-{}-{target}", release.tag_name);
    let archive_name = format!("{inner_name}.{ext}");
    let bundle_name = format!("{archive_name}.cosign.bundle");
    let archive_url = release.asset_url(&archive_name)?.to_string();
    let bundle_url = release.asset_url(&bundle_name)?.to_string();

    let dir = scratch_dir("apply")?;
    let archive_path = dir.path().join(&archive_name);
    let bundle_path = dir.path().join(&bundle_name);
    download_to_file(agent, &archive_url, &archive_path)?;
    download_to_file(agent, &bundle_url, &bundle_path)?;

    verify_blob(&bundle_path, &archive_path, identity)?;

    extract_archive(&archive_path, dir.path(), is_zip)?;
    let bin_name = if is_zip { "terrustia.exe" } else { "terrustia" };
    let new_binary = dir.path().join(&inner_name).join(bin_name);
    if !new_binary.is_file() {
        return Err(UpdateError::Extract(format!(
            "expected {} inside the archive; it was not there",
            new_binary.display()
        )));
    }

    // `dir` removes itself on drop, here and on every early `?` return above — no closure or
    // manual `remove_dir_all` needed to make cleanup unconditional.
    install(&new_binary, &release.tag_name, install_target)
}

/// Production's own entry point: same as [`apply`], but resolving the file to replace the
/// ordinary way — the binary currently running.
fn apply_current(
    agent: &ureq::Agent,
    api_base: &str,
    repo: &str,
    current_version: &str,
    identity: &Identity,
) -> Result<ApplyOutcome, UpdateError> {
    let current = std::env::current_exe().map_err(|e| UpdateError::Apply(e.to_string()))?;
    apply(agent, api_base, repo, current_version, identity, &current)
}

#[cfg(unix)]
fn extract_archive(archive: &Path, dest_dir: &Path, _is_zip: bool) -> Result<(), UpdateError> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .map_err(|e| UpdateError::Extract(format!("running tar: {e}")))?;
    if !status.success() {
        return Err(UpdateError::Extract(format!("tar exited with {status}")));
    }
    Ok(())
}

/// Windows releases ship as `.zip`, not `.tar.gz`; Windows' own bundled `tar.exe` (bsdtar, shipped
/// since Windows 10 1803 / Server 2019) auto-detects the format from content rather than the
/// extension, so `-xf` without `-z` handles both. `aarch64-pc-windows-msvc` compiles for real on
/// GitHub's native `windows-11-arm` runner (`ci.yml`'s cross-compile job, host-native there, not
/// cross-compiled), but that job runs `cargo check`, not `cargo test`, so this function's actual
/// runtime behavior on real Windows is still never executed anywhere in CI: disclosed here, and
/// in `update.rs`'s other two `cfg(windows)` sites, rather than left implicit.
#[cfg(windows)]
fn extract_archive(archive: &Path, dest_dir: &Path, _is_zip: bool) -> Result<(), UpdateError> {
    let status = Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .map_err(|e| UpdateError::Extract(format!("running tar: {e}")))?;
    if !status.success() {
        return Err(UpdateError::Extract(format!("tar exited with {status}")));
    }
    Ok(())
}

#[cfg(unix)]
fn install(new_binary: &Path, tag: &str, target: &Path) -> Result<ApplyOutcome, UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    // A sibling file on the same filesystem, so the final rename below is atomic — no window
    // where the installed path exists but is empty or partial.
    let tmp_name = format!(
        ".{}.update",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("terrustia")
    );
    let tmp = target.with_file_name(tmp_name);
    std::fs::copy(new_binary, &tmp).map_err(|e| UpdateError::Apply(e.to_string()))?;
    let mut perm = std::fs::metadata(&tmp)
        .map_err(|e| UpdateError::Apply(e.to_string()))?
        .permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&tmp, perm).map_err(|e| UpdateError::Apply(e.to_string()))?;
    std::fs::rename(&tmp, target).map_err(|e| UpdateError::Apply(e.to_string()))?;
    Ok(ApplyOutcome::Applied {
        tag: tag.to_string(),
        path: target.to_path_buf(),
    })
}

/// Windows cannot overwrite its own running executable's file — this very process is that file,
/// executing, while `terrustia update` runs. The verified binary is left beside it instead of
/// guessing at a rename-out-from-under-itself trick nothing here has a way to prove safe; the
/// operator finishes the swap once the server is stopped. See `extract_archive`'s doc comment for
/// what CI's `windows-11-arm` runner does and does not exercise of this module's Windows path.
#[cfg(windows)]
fn install(new_binary: &Path, tag: &str, target: &Path) -> Result<ApplyOutcome, UpdateError> {
    let beside = target.with_file_name("terrustia.exe.new");
    std::fs::copy(new_binary, &beside).map_err(|e| UpdateError::Apply(e.to_string()))?;
    Ok(ApplyOutcome::DownloadedForManualApply {
        tag: tag.to_string(),
        path: beside,
    })
}

/// Runs once, in the background, at server boot. Check-and-notify only: logs the finding, and
/// leaves a one-shot message for [`crate::game::GameServer`] to hand to the first recognised admin
/// who signs in after it was set (see `game/server.rs`'s `note_finished_auth`, the login-success
/// path this is wired into). Never downloads a full binary and never applies anything — that is
/// `terrustia update`'s job, run by a human on purpose.
pub async fn boot_check(notice: Arc<Mutex<Option<String>>>) {
    let outcome = tokio::task::spawn_blocking(|| {
        let agent = build_agent();
        let regexp = identity_regexp(REPO);
        let identity = Identity::Keyless {
            regexp: &regexp,
            issuer: OIDC_ISSUER,
        };
        check(&agent, API_BASE, REPO, env!("CARGO_PKG_VERSION"), &identity)
    })
    .await;
    match outcome {
        Ok(Ok(Some(update))) => {
            info!(version = %update.tag, "a new, signature-verified terrustia release is available");
            let message = format!(
                "a new terrustia release ({}) is available; its signature has been verified \
                 against this project's real release pipeline. Ask an operator to run \
                 `terrustia update`.",
                update.tag
            );
            if let Ok(mut slot) = notice.lock() {
                *slot = Some(message);
            }
        }
        Ok(Ok(None)) => debug!("terrustia is up to date"),
        Ok(Err(UpdateError::CosignMissing)) => warn!(
            "cosign is not installed, so update checks cannot verify a release's signature; \
             install cosign to enable them (see this crate's `update` module doc comment)"
        ),
        Ok(Err(e)) => warn!(error = %e, "could not check for a terrustia update"),
        Err(e) => warn!(error = %e, "the update check task panicked"),
    }
}

enum UpdateArgs {
    Check,
    Apply,
    Help,
}

fn parse_update_args(args: &[String]) -> Result<UpdateArgs, String> {
    match args {
        [] => Ok(UpdateArgs::Apply),
        [flag] if flag == "--check" => Ok(UpdateArgs::Check),
        [flag] if flag == "-h" || flag == "--help" => Ok(UpdateArgs::Help),
        _ => Err(format!(
            "unrecognised arguments to `update`: {}",
            args.join(" ")
        )),
    }
}

/// What `terrustia update` on the command line does — see the module doc comment for the split
/// between this and [`boot_check`].
pub async fn run_update_command(args: &[String]) -> Result<(), String> {
    match parse_update_args(args)? {
        UpdateArgs::Help => {
            println!(
                "usage: terrustia update [--check]\n\n\
                 Checks GitHub for a newer terrustia release, verifies its signature against this \
                 project's real release pipeline (via the real `cosign` binary), and installs it \
                 over the currently running binary.\n\n\
                 --check   only check and report; do not download or apply anything\n\
                 -h, --help   show this message"
            );
            Ok(())
        }
        mode => {
            let check_only = matches!(mode, UpdateArgs::Check);
            tokio::task::spawn_blocking(move || run_update_blocking(check_only))
                .await
                .map_err(|e| format!("the update task panicked: {e}"))?
                .map_err(|e| e.to_string())
        }
    }
}

fn run_update_blocking(check_only: bool) -> Result<(), UpdateError> {
    let agent = build_agent();
    let regexp = identity_regexp(REPO);
    let identity = Identity::Keyless {
        regexp: &regexp,
        issuer: OIDC_ISSUER,
    };
    if check_only {
        match check(&agent, API_BASE, REPO, env!("CARGO_PKG_VERSION"), &identity)? {
            Some(update) => println!(
                "terrustia {} is available, signature verified. Run `terrustia update` (without \
                 --check) to apply it.",
                update.tag
            ),
            None => println!(
                "terrustia is already up to date ({})",
                env!("CARGO_PKG_VERSION")
            ),
        }
    } else {
        match apply_current(&agent, API_BASE, REPO, env!("CARGO_PKG_VERSION"), &identity)? {
            ApplyOutcome::AlreadyUpToDate => println!(
                "terrustia is already up to date ({})",
                env!("CARGO_PKG_VERSION")
            ),
            ApplyOutcome::Applied { tag, path } => {
                println!("terrustia updated to {tag}\n  {}", path.display());
            }
            ApplyOutcome::DownloadedForManualApply { tag, path } => println!(
                "terrustia {tag} downloaded and verified, but this platform cannot replace its \
                 own running binary. Stop the server, then move this file over your installed \
                 terrustia.exe:\n  {}",
                path.display()
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_with_or_without_a_leading_v() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.0.1"), Some((0, 0, 1)));
    }

    #[test]
    fn a_tag_this_project_never_produces_is_refused_rather_than_guessed() {
        for bad in ["v1.2", "v1.2.3.4", "v1.2.3-rc1", "not-a-version", ""] {
            assert_eq!(parse_version(bad), None, "{bad} should not parse");
        }
    }

    #[test]
    fn newer_beats_older_beats_equal() {
        assert!(is_newer("v0.0.2", "0.0.1"));
        assert!(!is_newer("v0.0.1", "0.0.1"));
        assert!(!is_newer("v0.0.1", "0.0.2"));
        assert!(is_newer("v1.0.0", "0.99.99"));
    }

    #[test]
    fn an_unparseable_tag_never_claims_to_be_newer() {
        // A future tag format this build does not understand must not spam an admin to update
        // into something that itself cannot be understood.
        assert!(!is_newer("some-other-scheme", "0.0.1"));
    }

    #[test]
    fn a_target_triple_exists_for_this_machine_or_the_gap_is_named() {
        // Every machine this test actually runs on is one of the five release targets — the
        // point of the assertion is that the match arms above stay in sync with
        // `release.yml`'s real build matrix, not that every OS/ARCH pair in existence is covered.
        let found = target_triple();
        assert!(
            found.is_some(),
            "no target triple for {}/{} — release.yml's matrix and this match arm have drifted \
             apart",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    #[test]
    fn the_identity_regexp_matches_release_yml_verbatim() {
        // `release.yml`'s published verification instructions read literally
        // `--certificate-identity-regexp 'https://github.com/${{ github.repository }}/'` — this
        // has to stay byte-for-byte the same string with `REPO` substituted in, or a real,
        // legitimately-signed release would fail to verify.
        assert_eq!(
            identity_regexp("bybrooklyn/terrustia"),
            "https://github.com/bybrooklyn/terrustia/"
        );
    }

    /// `scratch_dir`'s first draft built a *predictable* path
    /// (`std::env::temp_dir().join(format!("terrustia-update-{label}-{pid}"))`) and called
    /// `create_dir_all` on it — which silently reuses whatever, or whoever's, directory (or
    /// symlink) is already sitting at that name. On a shared, world-writable `/tmp`, another
    /// local user who can see this process's PID (trivially, via `ps`) could pre-create that
    /// exact path, or a symlink at one of the fixed filenames later written inside it
    /// (`SHA256SUMS`, the release archive), before this process got there — a real
    /// arbitrary-file-write primitive. Pins the fix two ways: the path this function actually
    /// returns is never the old predictable one (proving there is no fixed name left to attack at
    /// all), and pre-creating something at that old predictable name has no effect on what
    /// `scratch_dir` returns or does — there is nothing for a pre-placed symlink to intercept.
    #[test]
    fn scratch_dir_never_uses_the_old_predictable_path_an_attacker_could_pre_create() {
        let label = "regression-test";
        let old_predictable_path =
            std::env::temp_dir().join(format!("terrustia-update-{label}-{}", std::process::id()));
        // Simulate an attacker who saw this process's PID and got there first: a symlink at the
        // exact name the old, vulnerable implementation would have written straight through.
        let _ = std::fs::remove_file(&old_predictable_path);
        let _ = std::fs::remove_dir_all(&old_predictable_path);
        #[cfg(unix)]
        {
            let bait_target = std::env::temp_dir().join("terrustia-update-test-bait-target");
            std::fs::create_dir_all(&bait_target).unwrap();
            std::os::unix::fs::symlink(&bait_target, &old_predictable_path).unwrap();
        }

        let dir = scratch_dir(label).expect("creating a scratch directory must succeed");

        assert_ne!(
            dir.path(),
            old_predictable_path,
            "the returned directory must not be the old predictable, pre-attackable path"
        );
        assert!(
            dir.path().is_dir(),
            "a real, freshly created directory, not a symlink followed into somewhere else"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "restricted to the owner");
        }

        // The pre-placed symlink at the old name is untouched — proof nothing was ever written
        // through it — and every call gets its own directory, not a shared/reused one.
        let second = scratch_dir(label).expect("a second call must also succeed");
        assert_ne!(
            dir.path(),
            second.path(),
            "two calls must never share a directory"
        );

        #[cfg(unix)]
        {
            let bait_target = std::env::temp_dir().join("terrustia-update-test-bait-target");
            assert!(
                std::fs::read_dir(&bait_target).unwrap().next().is_none(),
                "nothing was ever written through the pre-placed symlink"
            );
            let _ = std::fs::remove_file(&old_predictable_path);
            let _ = std::fs::remove_dir_all(&bait_target);
        }
    }

    /// `check`/`apply` both refuse outright — `Err(UpdateError::CosignMissing)`, no download, no
    /// notification — the instant `cosign_available()` says no; that one-line branch is what
    /// keeps "cosign is not installed" from silently becoming "checked but not actually verified"
    /// (this module's own doc comment). Proving *that* branch fires needs `cosign_available()` to
    /// actually return `false`, which means either hiding the real `cosign` from this whole
    /// process's `PATH` — a race against every other test in this file that also shells out to
    /// `cosign` concurrently — or overriding a look-up mechanism production never overrides. Ruled
    /// out as disproportionate to what's being proven: real `Command::spawn` behaviour on a
    /// genuinely nonexistent binary is not project-specific, and the branch is one `if` reading
    /// its result. What *is* tested here, safely, is the primitive the branch depends on: a name
    /// nothing on `PATH` provides comes back `false`, and it agrees with `cosign_available()`'s
    /// own real answer for `cosign` on whatever machine this runs on.
    #[test]
    fn a_nonexistent_binary_is_correctly_detected_as_unavailable() {
        assert!(!binary_available(
            "definitely-not-a-real-binary-terrustia-update-test"
        ));
        assert_eq!(binary_available("cosign"), cosign_available());
    }

    // --- Real cosign, real network-free fixtures, below. ---
    //
    // Minting a real *keyless* Sigstore signature (the kind `release.yml` and production code
    // both use) needs a live OIDC token from GitHub or Google that nothing in this sandboxed test
    // environment can produce non-interactively — there is no way around that, keyless signing's
    // entire point is that it cannot be done quietly. So these tests sign with a real, locally
    // generated cosign keypair instead ([`Identity::Key`]) and drive `verify_blob`/`check`/`apply`
    // exactly as production does otherwise — real `cosign` subprocess, real exit codes, real
    // tar archives. What production's specific *keyless* flags do was proven separately, by hand,
    // against a real, independently keyless-signed public artifact (`sigstore/cosign`'s own
    // `v3.1.3` GitHub release): `cosign verify-blob --bundle cosign-darwin-arm64.sigstore.json
    // --certificate-identity keyless@projectsigstore.iam.gserviceaccount.com
    // --certificate-oidc-issuer https://accounts.google.com cosign-darwin-arm64` printed `Verified
    // OK`; appending one byte to the downloaded binary and re-running the identical command
    // failed with `invalid signature`; pointing `--bundle` at `{}` failed with `bundle does not
    // contain cert for verification`. Together, the by-hand proof (real keyless flags work) and
    // these tests (the whole pipeline built on those flags works) cover what a single automated
    // test running in this environment cannot: the exact production code path, end to end.
    //
    // Skips (does not fail) when `cosign` is not on `PATH`, the same way `worlds.rs`'s own tests
    // skip assertions that depend on an environment detail outside this process's control.

    fn cosign_keypair(dir: &Path) -> (PathBuf, PathBuf) {
        let prefix = dir.join("cosign");
        let output = Command::new("cosign")
            .arg("generate-key-pair")
            .arg("--output-key-prefix")
            .arg(&prefix)
            .env("COSIGN_PASSWORD", "")
            .output()
            .expect("running cosign generate-key-pair");
        assert!(
            output.status.success(),
            "cosign generate-key-pair failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (dir.join("cosign.key"), dir.join("cosign.pub"))
    }

    /// Sign a fixture, or say the network was the reason we could not.
    ///
    /// `None` means cosign could not reach the public transparency log, and the caller skips.
    /// Signing with a local key needs no network of its own, but cosign posts the entry to
    /// rekor.sigstore.dev regardless, and modern versions have removed `--tlog-upload=false` in
    /// favour of a signing-config file whose only published source is itself a download. So these
    /// two tests genuinely depend on somebody else's uptime.
    ///
    /// They failed roughly one run in three here on a TLS handshake timeout, which is corrosive:
    /// a suite that fails at random teaches everyone to re-run it rather than read it, and it cost
    /// real time during a merge when it looked like a union break. Only a transparency-log failure
    /// is tolerated. Anything else, including cosign being absent or the key being wrong, still
    /// fails loudly, because those are real breakage rather than weather.
    fn sign_with_key(key: &Path, file: &Path) -> Option<PathBuf> {
        let bundle = PathBuf::from(format!("{}.cosign.bundle", file.display()));
        let output = Command::new("cosign")
            .arg("sign-blob")
            .arg("--key")
            .arg(key)
            .arg("--bundle")
            .arg(&bundle)
            .arg("--yes")
            .arg(file)
            .env("COSIGN_PASSWORD", "")
            .output()
            .expect("running cosign sign-blob");
        if output.status.success() {
            return Some(bundle);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let unreachable_log = stderr.contains("rekor.sigstore.dev")
            || stderr.contains("TLS handshake")
            || stderr.contains("giving up after");
        assert!(
            unreachable_log,
            "cosign sign-blob failed for a reason that is not the transparency log: {stderr}"
        );
        eprintln!(
            "SKIPPED: cosign could not reach the sigstore transparency log, so this test could not \
             build its fixture. This is the network, not the code."
        );
        None
    }

    fn test_scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "terrustia-update-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn verify_blob_accepts_a_real_signature_and_rejects_a_tampered_one() {
        if !cosign_available() {
            eprintln!("cosign not installed; skipping (see this module's own doc comment)");
            return;
        }
        let dir = test_scratch_dir("verify");
        let (key, pubkey) = cosign_keypair(&dir);
        let file = dir.join("payload.bin");
        std::fs::write(&file, b"a real payload, signed for real").unwrap();
        let Some(bundle) = sign_with_key(&key, &file) else {
            return;
        };
        let identity = Identity::Key {
            public_key: &pubkey,
        };

        verify_blob(&bundle, &file, &identity).expect("a genuine signature must verify");

        // The exact scenario a corrupted download or a tampered mirror would produce: same
        // bundle, different bytes underneath it.
        std::fs::write(&file, b"different bytes than what was signed").unwrap();
        assert!(
            verify_blob(&bundle, &file, &identity).is_err(),
            "a file that does not match what was signed must not verify"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A minimal HTTP/1.1 server, purpose-built for this one test: reads a request line, ignores
    /// headers, answers with pre-built bytes keyed by path. Stands in for GitHub's releases API
    /// and asset CDN without a mocking dependency — matching this workspace's existing preference
    /// (`tests/panel.rs`'s hand-rolled `ws_lite`) for a purpose-built stand-in over a
    /// general-purpose crate for something this small.
    ///
    /// Binds first and hands the caller its own base URL before serving anything, so a response
    /// body that needs to reference the server's own address (the releases-API JSON's
    /// `browser_download_url`s, which must point back at this same server) can be built with that
    /// address already known, rather than guessed or served from a second, different server.
    fn serve_fixture(
        build_files: impl FnOnce(&str) -> std::collections::HashMap<String, Vec<u8>>,
    ) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files = build_files(&base);
        std::thread::spawn(move || {
            for stream in listener.incoming().take(16) {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line == "\r\n" || line == "\n" => break,
                        Ok(_) => {}
                    }
                }
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                match files.get(&path) {
                    Some(body) => {
                        let header = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(body);
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                }
            }
        });
        base
    }

    /// Drives `check` and `apply` — the exact same functions `boot_check` and `terrustia update`
    /// call in production — end to end against a real, locally hand-rolled HTTP server and a
    /// real, key-signed release fixture: real GitHub-API-shaped JSON, a real `.tar.gz` built by
    /// the real `tar` binary, and real cosign bundles. Proves the whole pipeline (fetch the
    /// release, resolve asset URLs by name, download, verify, extract, and install) works
    /// together, not just each piece in isolation.
    #[test]
    fn the_full_check_and_apply_pipeline_works_against_a_real_signed_fixture_release() {
        if !cosign_available() {
            eprintln!("cosign not installed; skipping (see this module's own doc comment)");
            return;
        }
        let (target, is_zip) = target_triple().expect("this machine has a release target");
        if is_zip {
            // The fixture below only builds a `.tar.gz`, matching every non-Windows target.
            // Windows' own extraction path is disclosed as untestable from this session
            // elsewhere in this module — see `extract_archive`'s `cfg(windows)` doc comment.
            eprintln!("this machine's release target is a .zip one; skipping the .tar.gz fixture");
            return;
        }

        let dir = test_scratch_dir("pipeline");
        let (key, pubkey) = cosign_keypair(&dir);

        let tag = "v9.9.9";
        let sums_path = dir.join("SHA256SUMS");
        std::fs::write(&sums_path, b"deadbeef  a-fixture-file\n").unwrap();
        let Some(sums_bundle) = sign_with_key(&key, &sums_path) else {
            return;
        };

        let inner_name = format!("terrustia-{tag}-{target}");
        let staging = dir.join(&inner_name);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(
            staging.join("terrustia"),
            b"#!/bin/sh\necho fixture-binary\n",
        )
        .unwrap();
        let archive_name = format!("{inner_name}.tar.gz");
        let archive_path = dir.join(&archive_name);
        let tar_status = Command::new("tar")
            .arg("-czf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&dir)
            .arg(&inner_name)
            .status()
            .expect("running tar");
        assert!(tar_status.success());
        let Some(archive_bundle) = sign_with_key(&key, &archive_path) else {
            return;
        };

        let sums_bytes = std::fs::read(&sums_path).unwrap();
        let sums_bundle_bytes = std::fs::read(&sums_bundle).unwrap();
        let archive_bytes = std::fs::read(&archive_path).unwrap();
        let archive_bundle_bytes = std::fs::read(&archive_bundle).unwrap();
        let archive_name_for_closure = archive_name.clone();
        let base = serve_fixture(move |base| {
            let release_json = serde_json::json!({
                "tag_name": tag,
                "assets": [
                    {"name": "SHA256SUMS", "browser_download_url": format!("{base}/SHA256SUMS")},
                    {"name": "SHA256SUMS.cosign.bundle", "browser_download_url": format!("{base}/SHA256SUMS.cosign.bundle")},
                    {"name": archive_name_for_closure.clone(), "browser_download_url": format!("{base}/{archive_name_for_closure}")},
                    {"name": format!("{archive_name_for_closure}.cosign.bundle"), "browser_download_url": format!("{base}/{archive_name_for_closure}.cosign.bundle")},
                ],
            })
            .to_string();
            let mut files = std::collections::HashMap::new();
            files.insert(
                "/repos/fixture/repo/releases/latest".to_string(),
                release_json.into_bytes(),
            );
            files.insert("/SHA256SUMS".to_string(), sums_bytes.clone());
            files.insert(
                "/SHA256SUMS.cosign.bundle".to_string(),
                sums_bundle_bytes.clone(),
            );
            files.insert(
                format!("/{archive_name_for_closure}"),
                archive_bytes.clone(),
            );
            files.insert(
                format!("/{archive_name_for_closure}.cosign.bundle"),
                archive_bundle_bytes.clone(),
            );
            files
        });

        let agent = build_agent();
        let identity = Identity::Key {
            public_key: &pubkey,
        };

        let found = check(&agent, &base, "fixture/repo", "0.0.1", &identity)
            .expect("check should succeed against a validly-signed fixture");
        let found = found.expect("a newer release exists in the fixture and must be reported");
        assert_eq!(found.tag, tag);

        let install_target = dir.join("installed-terrustia");
        std::fs::write(&install_target, b"old binary contents").unwrap();
        let outcome = apply(
            &agent,
            &base,
            "fixture/repo",
            "0.0.1",
            &identity,
            &install_target,
        )
        .expect("apply should succeed against a validly-signed fixture");
        match outcome {
            ApplyOutcome::Applied {
                tag: applied_tag,
                path,
            } => {
                assert_eq!(applied_tag, tag);
                assert_eq!(path, install_target);
                let installed = std::fs::read_to_string(&install_target).unwrap();
                assert_eq!(
                    installed, "#!/bin/sh\necho fixture-binary\n",
                    "the file at the install target must actually be the new binary, not left \
                     over from before"
                );
            }
            _ => panic!("expected Applied on a unix test host"),
        }

        // A release whose signed manifest does not match the identity being checked against must
        // be refused, not merely warned about — the whole point of verifying at all.
        let wrong_dir = dir.join("wrong-keypair");
        std::fs::create_dir_all(&wrong_dir).unwrap();
        let (_wrong_key, wrong_pubkey) = cosign_keypair(&wrong_dir);
        let wrong_identity = Identity::Key {
            public_key: &wrong_pubkey,
        };
        let result = check(&agent, &base, "fixture/repo", "0.0.1", &wrong_identity);
        assert!(
            result.is_err(),
            "a manifest signed by a different key must not verify"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
