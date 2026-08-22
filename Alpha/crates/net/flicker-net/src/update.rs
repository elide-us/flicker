//! One-shot update check against GitHub Releases — the shipped half of the
//! ratified update architecture (in-app NOTIFY now; the launcher arrives with
//! the IdP work and owns actual patching; a self-updating game binary is
//! rejected outright).
//!
//! Same shape as the chat client: a background thread owns a current-thread
//! tokio runtime and the sync shell polls a `std::sync::mpsc` receiver. A game
//! must never block or error on connectivity, so EVERY failure path — offline,
//! rate-limited, API shape change, unparsable tag — simply never sends: the
//! menu shows no chip and the player is none the wiser.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde::Deserialize;

/// A newer published release: its bare version ("0.1.2") and release-page URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

/// The fields we read from `GET /repos/{owner}/{repo}/releases/latest`.
#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

/// Spawn the one-shot check. The receiver yields AT MOST one [`UpdateInfo`],
/// and only when the latest release's tag parses to a version NEWER than
/// `current` (the running `CARGO_PKG_VERSION`). Call once at boot; poll the
/// receiver per frame with `try_recv`.
pub fn check_github_latest(owner: &str, repo: &str, current: &str) -> Receiver<UpdateInfo> {
    let (tx, rx) = mpsc::channel();
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let Some(current) = parse_version(current) else {
        tracing::warn!("update check disabled: current version {current:?} did not parse");
        return rx;
    };
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async move {
            let Ok(client) = reqwest::Client::builder()
                // The GitHub API rejects requests without a User-Agent.
                .user_agent(concat!("prism-alpha/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(10))
                .build()
            else {
                return;
            };
            let Ok(resp) = client.get(&api).send().await else {
                tracing::debug!("update check: request failed (offline?)");
                return;
            };
            let Ok(latest) = resp.json::<LatestRelease>().await else {
                tracing::debug!("update check: unexpected response shape");
                return;
            };
            let Some(remote) = parse_version(&latest.tag_name) else {
                tracing::debug!("update check: tag {:?} did not parse", latest.tag_name);
                return;
            };
            if remote > current {
                tracing::info!(
                    "update available: v{}.{}.{} (running v{}.{}.{})",
                    remote.0,
                    remote.1,
                    remote.2,
                    current.0,
                    current.1,
                    current.2
                );
                let _ = tx.send(UpdateInfo {
                    version: format!("{}.{}.{}", remote.0, remote.1, remote.2),
                    url: latest.html_url,
                });
            }
        });
    });
    rx
}

/// Parse "0.1.2" / "v0.1.2" / "v0.1.2-alpha.1" into a comparable (major,
/// minor, patch) triple. Pre-release suffixes are ignored for ordering: our
/// tags are plain vX.Y.Z, and treating a newer triple's pre-release as newer
/// errs on the side of telling the player. Anything else → None (no chip).
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().strip_prefix('v').unwrap_or(s.trim());
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // four dotted parts is not a version we publish
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_version("0.1.1"), Some((0, 1, 1)));
        assert_eq!(parse_version("v0.1.1"), Some((0, 1, 1)));
        assert_eq!(parse_version("v1.2.3-alpha.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2.3+build7"), Some((1, 2, 3)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);

        // Tuple ordering IS semver ordering for plain triples.
        assert!(parse_version("v0.1.2") > parse_version("v0.1.1"));
        assert!(parse_version("v0.2.0") > parse_version("v0.1.9"));
        assert!(parse_version("v1.0.0") > parse_version("v0.99.99"));
        assert_eq!(parse_version("v0.1.1"), parse_version("0.1.1"));
    }

    /// An unparsable CURRENT version disables the check instead of panicking —
    /// the receiver just never yields.
    #[test]
    fn bad_current_version_yields_nothing() {
        let rx = check_github_latest("elide-us", "flicker", "not-a-version");
        assert!(rx.try_recv().is_err());
    }
}
