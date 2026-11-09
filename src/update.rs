//! Updates of Onde itself, from GitHub releases. Binaries are signed: without the
//! project's private key, nothing installs. Nothing happens silently either, since
//! restarting cuts the music: Onde tells, and the user chooses when to install.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use minisign_verify::{PublicKey, Signature};
use serde_json::Value;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "https://api.github.com/repos/dontleteuwatchyou/onde/releases/latest";
const PUBLIC_KEY: &str = include_str!("../assets/update.pub");
const MAX_BINARY: u64 = 256 * 1024 * 1024;
const MAX_SIGNATURE: u64 = 4 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Available {
    pub version: String,
    asset: String,
    binary_url: String,
    signature_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .user_agent(concat!("onde/", env!("CARGO_PKG_VERSION")))
            .build(),
    )
}

/// The file a release publishes for this system, beside its `.minisig`.
fn asset_name() -> String {
    format!(
        "onde-{}-{}{}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::EXE_SUFFIX
    )
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

/// A release without a signed binary for this system is not an update.
fn parse_release(release: &Value, asset: &str, current: &str) -> Option<Available> {
    let tag = release["tag_name"].as_str()?;
    if !is_newer(tag, current) {
        return None;
    }
    let url = |name: &str| {
        release["assets"].as_array()?.iter().find_map(|file| {
            (file["name"].as_str()? == name)
                .then(|| file["browser_download_url"].as_str().map(str::to_owned))?
        })
    };
    Some(Available {
        version: tag.strip_prefix('v').unwrap_or(tag).to_owned(),
        asset: asset.to_owned(),
        binary_url: url(asset)?,
        signature_url: url(&format!("{asset}.minisig"))?,
    })
}

pub fn check() -> Result<Option<Available>> {
    let address = std::env::var("ONDE_UPDATE_URL").unwrap_or_else(|_| LATEST.to_owned());
    let release: Value = agent()
        .get(&address)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("GitHub injoignable")?
        .body_mut()
        .read_json()
        .context("réponse de GitHub illisible")?;
    Ok(parse_release(&release, &asset_name(), CURRENT))
}

/// The trusted comment is signed too: it stops an old or foreign binary, even a
/// genuinely signed one, from passing for this version.
fn verify(
    binary: &[u8],
    signature: &str,
    public_key: &str,
    version: &str,
    asset: &str,
) -> Result<()> {
    let key =
        PublicKey::decode(public_key.trim()).map_err(|_| anyhow!("clé publique illisible"))?;
    let signature = Signature::decode(signature).map_err(|_| anyhow!("signature illisible"))?;
    key.verify(binary, &signature, false)
        .map_err(|_| anyhow!("signature invalide : mise à jour refusée"))?;
    if signature.trusted_comment() != format!("onde {version} {asset}") {
        bail!("signature d’une autre version : mise à jour refusée");
    }
    Ok(())
}

/// Why this copy of Onde can't replace itself, if so.
pub fn install_problem(exe: &Path) -> Option<&'static str> {
    if exe.components().any(|part| part.as_os_str() == "target") {
        return Some("Onde est compilé depuis les sources : mets-le à jour avec git pull.");
    }
    let probe = exe
        .parent()?
        .join(format!(".onde-update-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(_) => Some("Pas le droit de remplacer Onde : mets-le à jour comme tu l’as installé."),
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>> {
    agent()
        .get(url)
        .call()
        .context("téléchargement impossible")?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .context("téléchargement interrompu")
}

/// Downloads, verifies and swaps the binary; takes effect at the next launch.
/// Returns the path to launch, which `current_exe` no longer gives once replaced.
pub fn install(update: &Available) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("emplacement d’Onde introuvable")?;
    if let Some(problem) = install_problem(&exe) {
        bail!(problem);
    }
    let signature = String::from_utf8(download(&update.signature_url, MAX_SIGNATURE)?)
        .context("signature illisible")?;
    let binary = download(&update.binary_url, MAX_BINARY)?;
    verify(
        &binary,
        &signature,
        PUBLIC_KEY,
        &update.version,
        &update.asset,
    )?;
    let staged = exe.with_extension(format!("update-{}", std::process::id()));
    let result = stage(&staged, &binary).and_then(|()| {
        self_replace::self_replace(&staged).context("remplacement d’Onde impossible")
    });
    let _ = std::fs::remove_file(&staged);
    result.map(|()| exe)
}

fn stage(path: &Path, binary: &[u8]) -> Result<()> {
    let mut file = std::fs::File::create(path).context("écriture de la mise à jour")?;
    file.write_all(binary)
        .context("écriture de la mise à jour")?;
    file.sync_all().context("écriture de la mise à jour")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))
            .context("écriture de la mise à jour")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A throwaway key, unrelated to the one signing releases.
    const TEST_KEY: &str =
        "untrusted comment: test\nRWQyG9GTT69+3xsaN7WzYgYQCZC238Q7LR9i1rzaNsx6Anl/ntwdpqjh";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from rsign secret key
RUQyG9GTT69+38kdIlakQ+ttbzwxmdEAJm+4A3AlD2g/dxVhSS+pIHk7ZKriAVnGxYuPBo+3RNi5DS2ThWFcGlEU37I3rwAgyQ0=
trusted comment: onde 9.9.9 onde-linux-x86_64
W2N8I5+4Zf9QAkoI3vfGL4dogyd8bXt0epjR0gcOpfedS8EFRaWdHoSLukNc5pQ7IeQxgcSr63sDrQRxfbA3Dw==";

    fn release(tag: &str, names: &[&str]) -> Value {
        let assets: Vec<Value> = names
            .iter()
            .map(|name| json!({"name": name, "browser_download_url": format!("https://x/{name}")}))
            .collect();
        json!({"tag_name": tag, "assets": assets})
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("v0.1.0", "0.1.1"));
        assert!(!is_newer("nightly", "0.1.1"));
        assert!(!is_newer("v1.0.0-rc1", "0.1.1"));
    }

    #[test]
    fn a_release_needs_a_signed_binary_for_this_system() {
        let full = release(
            "v1.0.0",
            &["onde-linux-x86_64", "onde-linux-x86_64.minisig"],
        );
        let found = parse_release(&full, "onde-linux-x86_64", "0.1.1").unwrap();
        assert_eq!(found.version, "1.0.0");
        assert_eq!(found.binary_url, "https://x/onde-linux-x86_64");
        assert_eq!(found.signature_url, "https://x/onde-linux-x86_64.minisig");
        let unsigned = release("v1.0.0", &["onde-linux-x86_64"]);
        assert_eq!(parse_release(&unsigned, "onde-linux-x86_64", "0.1.1"), None);
        assert_eq!(
            parse_release(&full, "onde-windows-x86_64.exe", "0.1.1"),
            None
        );
        assert_eq!(parse_release(&full, "onde-linux-x86_64", "1.0.0"), None);
    }

    #[test]
    fn only_a_signature_for_this_binary_and_version_passes() {
        let check = |data: &[u8], version, asset| {
            verify(data, TEST_SIGNATURE, TEST_KEY, version, asset).map_err(|e| e.to_string())
        };
        assert!(check(b"onde test", "9.9.9", "onde-linux-x86_64").is_ok());
        assert!(
            check(b"onde piege", "9.9.9", "onde-linux-x86_64")
                .unwrap_err()
                .contains("invalide")
        );
        assert!(
            check(b"onde test", "9.9.8", "onde-linux-x86_64")
                .unwrap_err()
                .contains("autre version")
        );
        assert!(
            check(b"onde test", "9.9.9", "onde-windows-x86_64.exe")
                .unwrap_err()
                .contains("autre version")
        );
    }

    #[test]
    fn the_real_key_rejects_a_signature_from_another_key() {
        assert!(
            verify(
                b"onde test",
                TEST_SIGNATURE,
                PUBLIC_KEY,
                "9.9.9",
                "onde-linux-x86_64"
            )
            .is_err()
        );
    }

    #[test]
    fn a_build_from_the_sources_does_not_replace_itself() {
        assert!(install_problem(Path::new("/x/onde/target/release/onde")).is_some());
    }
}
