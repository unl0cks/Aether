//! Checks whether a newer Aether has been released, and replaces this one with it.
//!
//! The awkward part on Windows is that a running executable cannot be deleted or overwritten. It
//! can, however, be *renamed*: the open handle follows the file rather than the path. So an update
//! moves the running binary aside, moves the new one into its place, and deletes the leftover on
//! the next start.
//!
//! That leaves a window between the two renames where the program has no binary at its own path.
//! [`install_downloaded`] closes it by putting the old one back if the second rename fails, which
//! is the only part of this worth more tests than code.
//!
//! Nothing here interrupts a session on its own. A check that cannot reach GitHub, or reaches it
//! and cannot make sense of the reply, says nothing at all: being told mid-play that a web request
//! timed out is worse than not knowing there was an update.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where releases are published.
const RELEASES_API: &str = "https://api.github.com/repos/unl0cks/Aether/releases/latest";

/// The asset carrying the client, and the file listing its hash.
const CLIENT_ASSET_PREFIX: &str = "Aether-Portable-";
const CLIENT_ASSET_SUFFIX: &str = "-win-x64.zip";
const CHECKSUM_ASSET: &str = "SHA256SUMS.txt";

/// Suffix for the outgoing binary, left behind until the next start can delete it.
const BACKUP_SUFFIX: &str = "old";

/// A release worth offering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub archive_url: String,
    pub archive_name: String,
    pub checksums_url: String,
}

/// Read the release out of the API's reply.
///
/// Returns `None` for anything unexpected rather than failing loudly. This runs on every start, and
/// a GitHub outage or a change to their JSON must not be able to stop Aether from opening.
pub fn parse_latest_release(json: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;

    // A draft or a prerelease is not something to hand a player.
    if value.get("draft").and_then(serde_json::Value::as_bool) == Some(true)
        || value.get("prerelease").and_then(serde_json::Value::as_bool) == Some(true)
    {
        return None;
    }

    let version = normalise_version(value.get("tag_name")?.as_str()?);
    if version.is_empty() {
        return None;
    }

    let assets = value.get("assets")?.as_array()?;
    let find = |predicate: &dyn Fn(&str) -> bool| -> Option<(String, String)> {
        assets.iter().find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            predicate(name).then(|| {
                Some((
                    name.to_owned(),
                    asset.get("browser_download_url")?.as_str()?.to_owned(),
                ))
            })?
        })
    };

    let (archive_name, archive_url) =
        find(&|name| name.starts_with(CLIENT_ASSET_PREFIX) && name.ends_with(CLIENT_ASSET_SUFFIX))?;
    let (_, checksums_url) = find(&|name| name.eq_ignore_ascii_case(CHECKSUM_ASSET))?;

    Some(Release {
        version,
        archive_url,
        archive_name,
        checksums_url,
    })
}

/// Drop a leading `v`, so `v0.5.2` and `0.5.2` are one version.
fn normalise_version(version: &str) -> String {
    let trimmed = version.trim();
    trimmed
        .strip_prefix(['v', 'V'])
        .unwrap_or(trimmed)
        .to_owned()
}

/// Whether `candidate` is worth downloading over `installed`.
///
/// Compared as numbers per component, never as text. Sorted as strings, `0.5.10` falls below
/// `0.5.9`, which from the tenth patch onwards would leave everyone on the older build while the
/// update check went on appearing to work.
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let left = components(candidate);
    let right = components(installed);

    if left.is_empty() {
        return false;
    }
    if right.is_empty() {
        return true;
    }

    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

fn components(version: &str) -> Vec<u64> {
    let normalised = normalise_version(version);
    // Anything past a dash is a channel or build metadata, not an ordering to trust. Local builds
    // carry one, which is what keeps them from offering to "update" to the release they came from.
    let numeric = normalised.split('-').next().unwrap_or_default();

    let mut parts = Vec::new();
    for piece in numeric.split('.').filter(|piece| !piece.is_empty()) {
        match piece.parse::<u64>() {
            Ok(value) => parts.push(value),
            Err(_) => break,
        }
    }
    parts
}

/// Whether this build should check at all.
///
/// A build made here rather than released carries a channel suffix, and offering it an "update" to
/// the release it was built from would be noise during development.
pub fn version_is_released(version: &str) -> bool {
    !version.contains("-aether-local") && !components(version).is_empty()
}

/// Read a `sha256sum` listing into name against hash.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((hash, rest)) = line.split_once([' ', '\t']) else {
            continue;
        };
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        // A leading `*` marks binary mode in some tools and is not part of the name.
        let name = rest.trim_start_matches([' ', '\t', '*']).trim();
        if !name.is_empty() {
            map.insert(name.to_owned(), hash.to_ascii_lowercase());
        }
    }

    map
}

/// Whether a download is the file the release published.
///
/// An asset with no published hash is untrusted rather than assumed fine. Verification is the only
/// reason downloading and running an executable is acceptable at all, so a missing entry has to
/// fail closed.
pub fn checksum_matches(checksums: &HashMap<String, String>, name: &str, actual: &str) -> bool {
    checksums
        .get(name)
        .is_some_and(|expected| expected.eq_ignore_ascii_case(actual))
}

fn backup_path(current: &Path) -> PathBuf {
    let mut name = current.as_os_str().to_owned();
    name.push(".");
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Delete the binary left behind by a previous update, if it is still there.
///
/// Called at start, when nothing holds it open. Failure is ignored: a stale file wastes disk and
/// nothing else, and refusing to launch over it would be absurd.
pub fn clean_stale_backup(current: &Path) {
    let backup = backup_path(current);
    if backup.exists() {
        let _ = std::fs::remove_file(&backup);
    }
}

/// Put `staged` in place of `current`, keeping the old one aside.
///
/// The order matters and so does the recovery. Moving the running binary away first is what makes
/// room; if the second move then fails, this puts it back before returning the error, because the
/// alternative is a machine with no `aether.exe` at the path its shortcuts point at.
pub fn install_downloaded(current: &Path, staged: &Path) -> std::io::Result<()> {
    let backup = backup_path(current);

    // A leftover from an earlier update would block the rename.
    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }

    std::fs::rename(current, &backup)?;

    match std::fs::rename(staged, current) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Put it back. If even this fails there is nothing further to try, and the original
            // error is the more useful one to report.
            let _ = std::fs::rename(&backup, current);
            Err(error)
        }
    }
}

/// Ask GitHub what the newest release is.
pub async fn fetch_latest_release() -> Option<Release> {
    let client = reqwest::Client::builder()
        // GitHub rejects requests with no user agent.
        .user_agent(concat!("Aether/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let response = client.get(RELEASES_API).send().await.ok()?;
    if !response.status().is_success() {
        // Rate limiting and outages both land here. Neither is worth telling a player about.
        tracing::debug!("update check returned {}", response.status());
        return None;
    }

    parse_latest_release(&response.text().await.ok()?)
}

/// Fetch the release, check it, and put the new binary in place.
///
/// Returns the staged path on success so the caller can decide when to swap. Every failure is an
/// error rather than a silent pass: once a player has said yes, they are owed an answer.
pub async fn download_and_stage(release: &Release, current_exe: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::{Context as _, bail};
    use sha2::{Digest, Sha256};

    let client = reqwest::Client::builder()
        .user_agent(concat!("Aether/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let archive = client
        .get(&release.archive_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await
        .context("downloading the update")?;

    // Verified before anything is written to disk, let alone run.
    let actual = format!("{:x}", Sha256::digest(&archive));
    let listing = client
        .get(&release.checksums_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await
        .context("downloading the checksums")?;

    if !checksum_matches(&parse_checksums(&listing), &release.archive_name, &actual) {
        bail!(
            "the download did not match the checksum published for {}",
            release.archive_name
        );
    }

    // Extracted beside the current binary rather than into a temp directory, so the final move is
    // a rename within one volume rather than a copy across two.
    let staged = current_exe.with_extension("exe.new");
    let _ = std::fs::remove_file(&staged);

    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive.as_ref()))
        .context("reading the update archive")?;
    let mut entry = zip
        .by_name("aether.exe")
        .context("the update archive has no aether.exe in it")?;

    let mut file = std::fs::File::create(&staged).context("writing the new binary")?;
    std::io::copy(&mut entry, &mut file).context("writing the new binary")?;
    drop(file);

    Ok(staged)
}

/// Ask about, and then carry out, an update. Returns true when Aether should restart.
///
/// Called before the window opens, so the dialogs are the only thing on screen and there is no
/// half-drawn game behind them.
pub fn offer_update(release: &Release, runtime: &tokio::runtime::Runtime) -> bool {
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };

    let wants_it = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Aether")
        .set_description(format!(
            "Aether {} is available.

Would you like to update now?",
            release.version
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes;

    if !wants_it {
        return false;
    }

    let staged = match runtime.block_on(download_and_stage(release, &current_exe)) {
        Ok(staged) => staged,
        Err(error) => {
            tracing::warn!("update download failed: {error:#}");
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Warning)
                .set_title("Aether")
                .set_description(format!(
                    "The update could not be downloaded.

{error}

Aether will start normally."
                ))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            return false;
        }
    };

    if let Err(error) = install_downloaded(&current_exe, &staged) {
        let _ = std::fs::remove_file(&staged);
        tracing::warn!("update install failed: {error}");

        // Permission denied is the ordinary case for a Program Files install, and it deserves a
        // better answer than the raw error: the player did nothing wrong and there is a way out.
        let advice = if error.kind() == std::io::ErrorKind::PermissionDenied {
            "Aether does not have permission to replace itself where it is installed.              Running the installer again will update it."
        } else {
            "Aether has been left as it was."
        };
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Aether")
            .set_description(format!(
                "The update could not be applied.

{advice}"
            ))
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        return false;
    }

    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title("Aether")
        .set_description(format!(
            "Aether has been updated to {}.

It needs to restart to finish.",
            release.version
        ))
        .set_buttons(rfd::MessageButtons::OkCustom("Restart now".to_owned()))
        .show();

    true
}

/// Start the freshly installed binary and let this one exit.
pub fn restart() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST_JSON: &str = r#"{
        "tag_name": "v0.5.3",
        "draft": false,
        "prerelease": false,
        "assets": [
            {"name": "Aether-Setup-0.5.3-win-x64.exe", "browser_download_url": "https://example/s.exe"},
            {"name": "Aether-Portable-0.5.3-win-x64.zip", "browser_download_url": "https://example/p.zip"},
            {"name": "SHA256SUMS.txt", "browser_download_url": "https://example/sums.txt"}
        ]
    }"#;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aether-selfupdate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn the_client_archive_is_picked_out_of_the_release() {
        let release = parse_latest_release(LATEST_JSON).expect("a release");

        assert_eq!(release.version, "0.5.3");
        assert_eq!(release.archive_name, "Aether-Portable-0.5.3-win-x64.zip");
        assert_eq!(release.archive_url, "https://example/p.zip");
        assert_eq!(release.checksums_url, "https://example/sums.txt");
    }

    /// This runs on every start. Anything unreadable has to mean "no update", never a failure that
    /// could stop Aether opening.
    #[test]
    fn an_unreadable_reply_is_no_release_rather_than_a_failure() {
        for json in ["", "not json", "[]", "{}", r#"{"tag_name":""}"#] {
            assert_eq!(parse_latest_release(json), None, "{json}");
        }
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        for flag in ["draft", "prerelease"] {
            let json = format!(
                r#"{{"tag_name":"v9.9.9","{flag}":true,"assets":[
                    {{"name":"Aether-Portable-9.9.9-win-x64.zip","browser_download_url":"u"}},
                    {{"name":"SHA256SUMS.txt","browser_download_url":"u"}}]}}"#
            );
            assert_eq!(parse_latest_release(&json), None, "{flag}");
        }
    }

    /// A release with no client archive is not something to act on.
    #[test]
    fn a_release_without_the_archive_or_its_checksums_is_ignored() {
        let no_archive = r#"{"tag_name":"v1.0.0","assets":[
            {"name":"SHA256SUMS.txt","browser_download_url":"u"}]}"#;
        let no_sums = r#"{"tag_name":"v1.0.0","assets":[
            {"name":"Aether-Portable-1.0.0-win-x64.zip","browser_download_url":"u"}]}"#;

        assert_eq!(parse_latest_release(no_archive), None);
        assert_eq!(parse_latest_release(no_sums), None);
    }

    /// Compared as text, 0.5.10 sorts below 0.5.9 and every player stops being offered updates from
    /// the tenth patch on, with the check still appearing to run.
    #[test]
    fn versions_are_ordered_as_numbers() {
        assert!(is_newer("0.5.10", "0.5.9"));
        assert!(!is_newer("0.5.9", "0.5.10"));
        assert!(is_newer("0.6.0", "0.5.99"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("v0.5.3", "0.5.2"));
        assert!(!is_newer("0.5.2", "0.5.2"));
        assert!(!is_newer("0.5.2", "v0.5.3"));
    }

    #[test]
    fn a_local_build_does_not_offer_to_update_to_the_release_it_came_from() {
        assert!(!version_is_released("0.5.2-aether-local"));
        assert!(version_is_released("0.5.2"));

        // The suffix must not make it look older either, or it would nag every launch.
        assert!(!is_newer("0.5.2", "0.5.2-aether-local"));
    }

    #[test]
    fn checksums_are_read_from_the_published_listing() {
        let text = "2e14fad68a1dad9ef8122c652e728f968389f06dd980b381749a99da7ebd716c  Aether-Setup-0.5.2-win-x64.exe\n\
                    f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755  Aether-Portable-0.5.2-win-x64.zip\n";
        let checksums = parse_checksums(text);

        assert_eq!(checksums.len(), 2);
        assert!(checksum_matches(
            &checksums,
            "Aether-Portable-0.5.2-win-x64.zip",
            "F6955B9B86A6EDBCB6AAD8DE144D8D8D94E6650A0E8D24F5DCAAC02A6CAA4755",
        ));
    }

    /// Verification is the whole reason downloading an executable is acceptable, so an asset with
    /// no published hash has to fail closed rather than be assumed fine.
    #[test]
    fn an_unknown_or_mismatched_hash_is_rejected() {
        let checksums = parse_checksums(
            "f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755  archive.zip\n",
        );

        assert!(!checksum_matches(
            &checksums,
            "archive.zip",
            &"0".repeat(64)
        ));
        assert!(!checksum_matches(
            &checksums,
            "not-listed.zip",
            &"f".repeat(64)
        ));
    }

    #[test]
    fn junk_lines_in_the_checksum_listing_are_skipped() {
        let checksums = parse_checksums(
            "# comment\n\nnotahash  file.zip\n\
             f6955b9b86a6edbcb6aad8de144d8d8d94e6650a0e8d24f5dcaac02a6caa4755 *binary.zip\n",
        );

        assert_eq!(checksums.len(), 1);
        assert!(checksums.contains_key("binary.zip"));
    }

    #[test]
    fn an_update_replaces_the_binary_and_keeps_the_old_one_aside() {
        let dir = scratch_dir("swap");
        let current = dir.join("aether.exe");
        let staged = dir.join("aether.exe.new");
        std::fs::write(&current, b"old").expect("write");
        std::fs::write(&staged, b"new").expect("write");

        install_downloaded(&current, &staged).expect("swap");

        assert_eq!(std::fs::read(&current).expect("read"), b"new");
        assert_eq!(
            std::fs::read(backup_path(&current)).expect("read"),
            b"old",
            "the outgoing binary is kept until the next start can delete it"
        );
        assert!(!staged.exists());
    }

    /// The dangerous window: the running binary has been moved aside and the new one fails to take
    /// its place. Without the rollback the machine is left with no aether.exe at all.
    #[test]
    fn a_failed_swap_puts_the_original_back() {
        let dir = scratch_dir("rollback");
        let current = dir.join("aether.exe");
        std::fs::write(&current, b"old").expect("write");

        let missing = dir.join("never-downloaded.exe");
        let result = install_downloaded(&current, &missing);

        assert!(
            result.is_err(),
            "a missing staged file must not report success"
        );
        assert_eq!(
            std::fs::read(&current).expect("read"),
            b"old",
            "the original binary must be back at its own path"
        );
        assert!(!backup_path(&current).exists());
    }

    #[test]
    fn a_leftover_backup_does_not_block_the_next_update() {
        let dir = scratch_dir("leftover");
        let current = dir.join("aether.exe");
        let staged = dir.join("aether.exe.new");
        std::fs::write(&current, b"old").expect("write");
        std::fs::write(&staged, b"new").expect("write");
        std::fs::write(backup_path(&current), b"older still").expect("write");

        install_downloaded(&current, &staged).expect("swap over a leftover");

        assert_eq!(std::fs::read(&current).expect("read"), b"new");
    }

    #[test]
    fn the_stale_backup_is_cleaned_up_and_a_missing_one_is_not_an_error() {
        let dir = scratch_dir("clean");
        let current = dir.join("aether.exe");
        std::fs::write(&current, b"current").expect("write");
        std::fs::write(backup_path(&current), b"old").expect("write");

        clean_stale_backup(&current);
        assert!(!backup_path(&current).exists());

        // Nothing to clean is the normal case and must be silent.
        clean_stale_backup(&current);
        assert!(current.exists());
    }
}
