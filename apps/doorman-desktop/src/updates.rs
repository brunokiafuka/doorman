use std::process::Command;

use anyhow::{Context, Result, bail};
use semver::Version;

const RELEASE_API: &str = "https://api.github.com/repos/brunokiafuka/doorman/releases/latest";
pub const RELEASE_PAGE: &str = "https://github.com/brunokiafuka/doorman/releases/latest";

pub struct Release {
    pub version: Version,
    pub newer: bool,
}

pub fn check() -> Result<Release> {
    // Run on a worker thread. Bound the request so offline checks always finish.
    let output = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            "15",
            "--max-filesize",
            "1048576",
            "--user-agent",
            "Doorman-update-check",
            "--header",
            "Accept: application/vnd.github+json",
            RELEASE_API,
        ])
        .output()
        .context("Could not start the update check")?;
    if !output.status.success() {
        bail!("Release service unavailable");
    }
    parse_release(&output.stdout, env!("CARGO_PKG_VERSION"))
}

fn parse_release(bytes: &[u8], installed: &str) -> Result<Release> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    if value["draft"].as_bool() != Some(false) || value["prerelease"].as_bool() != Some(false) {
        bail!("Expected a published stable release");
    }
    let tag = value["tag_name"]
        .as_str()
        .context("Missing release version")?;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))?;
    if !version.pre.is_empty() {
        bail!("Expected a stable version");
    }
    let installed = Version::parse(installed)?;
    // Build metadata does not affect update precedence.
    let newer = version.cmp_precedence(&installed).is_gt();
    Ok(Release { version, newer })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, installed: &str) -> Release {
        parse_release(
            format!(r#"{{"tag_name":"{tag}","draft":false,"prerelease":false}}"#).as_bytes(),
            installed,
        )
        .unwrap()
    }

    #[test]
    fn compares_versions_numerically_without_downgrades() {
        assert!(release("v0.10.0", "0.9.9").newer);
        assert!(!release("v0.3.1", "0.3.1").newer);
        assert!(!release("v0.3.1", "0.4.0").newer);
        assert!(!release("v0.3.1+build.2", "0.3.1+build.1").newer);
        assert!(release("v1.0.0", "1.0.0-beta.1").newer);
    }

    #[test]
    fn rejects_unpublished_or_malformed_releases() {
        for payload in [
            r#"{"tag_name":"v1.0.0","draft":true,"prerelease":false}"#,
            r#"{"tag_name":"v1.0.0","draft":false,"prerelease":true}"#,
            r#"{"tag_name":"v1.0.0-beta.1","draft":false,"prerelease":false}"#,
            r#"{"tag_name":"bad","draft":false,"prerelease":false}"#,
            r#"{"message":"API rate limit exceeded"}"#,
            "not json",
        ] {
            assert!(parse_release(payload.as_bytes(), "0.3.1").is_err());
        }
    }
}
