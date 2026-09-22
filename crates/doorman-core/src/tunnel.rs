//! Shared tunnel discovery and origin routing for the CLI and desktop app.
use std::{env, ffi::OsStr, os::unix::fs::PermissionsExt, path::PathBuf};

use crate::Route;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Auto,
    Cloudflare,
    Ngrok,
}

impl Provider {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "cloudflare" => Ok(Self::Cloudflare),
            "ngrok" => Ok(Self::Ngrok),
            _ => Err(format!("Unknown tunnel provider: {value}")),
        }
    }

    pub fn binary(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflared",
            Self::Ngrok => "ngrok",
            Self::Auto => unreachable!("resolve auto before building a command"),
        }
    }

    pub fn args(self, route: &Route) -> Vec<String> {
        // Never expose the host-routed proxy: public Host headers must not be
        // able to select other local routes.
        let origin = format!("http://127.0.0.1:{}", route.port);
        let host = route.hostname();
        match self {
            Self::Cloudflare => vec![
                "tunnel".into(),
                "--url".into(),
                origin,
                "--http-host-header".into(),
                host,
            ],
            Self::Ngrok => vec!["http".into(), origin, format!("--host-header={host}")],
            Self::Auto => unreachable!("resolve auto before building a command"),
        }
    }

    pub fn managed_args(self, route: &Route) -> Vec<String> {
        let mut args = self.args(route);
        match self {
            Self::Cloudflare => {
                // --output is a global option, so it must precede `tunnel`.
                args.splice(0..0, ["--output".into(), "json".into()]);
                args.push("--no-autoupdate".into());
            }
            Self::Ngrok => args.extend(["--log=stdout".into(), "--log-format=json".into()]),
            Self::Auto => unreachable!("resolve auto before building a command"),
        }
        args
    }
}

pub fn find_executable(binary: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|directory| directory.join(binary))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Finder-launched apps don't inherit login-shell PATH additions.
pub fn desktop_path() -> std::ffi::OsString {
    let mut paths: Vec<_> = env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect();
    paths.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/bin"));
    }
    env::join_paths(paths).unwrap_or_default()
}

pub fn select(
    requested: Provider,
    mut find: impl FnMut(&str) -> Option<PathBuf>,
) -> Result<(Provider, PathBuf), String> {
    let candidates: &[Provider] = match requested {
        Provider::Auto => &[Provider::Cloudflare, Provider::Ngrok],
        Provider::Cloudflare => &[Provider::Cloudflare],
        Provider::Ngrok => &[Provider::Ngrok],
    };
    for &provider in candidates {
        if let Some(path) = find(provider.binary()) {
            return Ok((provider, path));
        }
    }
    Err(if requested == Provider::Auto {
        "No tunnel provider found. Install cloudflared (https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/) or ngrok (https://ngrok.com/download).".into()
    } else {
        format!(
            "{} is not installed in the searched paths; install it or choose another provider.",
            requested.binary()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_logging_is_a_global_option() {
        let route = Route::new("shop", 4321, None, None).unwrap();
        assert_eq!(
            Provider::Cloudflare.managed_args(&route),
            [
                "--output",
                "json",
                "tunnel",
                "--url",
                "http://127.0.0.1:4321",
                "--http-host-header",
                "shop.localhost",
                "--no-autoupdate"
            ]
        );
    }

    #[test]
    #[ignore = "requires an installed cloudflared; only invokes help, never opens a tunnel"]
    fn installed_cloudflared_accepts_managed_args() {
        let executable =
            find_executable("cloudflared", &desktop_path()).expect("install cloudflared");
        let route = Route::new("shop", 4321, None, None).unwrap();
        let output = std::process::Command::new(executable)
            .args(Provider::Cloudflare.managed_args(&route))
            .arg("--help")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        // cloudflared can exit 0 even when the flags are invalid.
        assert!(!text.contains("flag provided but not defined"), "{text}");
        assert!(!text.contains("Incorrect Usage"), "{text}");
        assert!(text.contains("USAGE:"), "{text}");
    }

    #[test]
    fn selection_prefers_cloudflare_and_respects_explicit_choices() {
        assert_eq!(
            select(Provider::Auto, |b| Some(b.into())).unwrap().0,
            Provider::Cloudflare
        );
        let ngrok = |b: &str| (b == "ngrok").then(|| PathBuf::from(b));
        assert_eq!(select(Provider::Auto, ngrok).unwrap().0, Provider::Ngrok);
        assert!(select(Provider::Cloudflare, ngrok).is_err());
        assert!(select(Provider::Auto, |_| None).is_err());
        assert_eq!(
            select(Provider::Ngrok, |b| Some(b.into())).unwrap().0,
            Provider::Ngrok
        );
    }

    #[test]
    fn args_expose_only_the_app_with_a_fixed_host() {
        let route = Route::new("fix-ui.shop", 4321, None, None).unwrap();
        assert_eq!(
            Provider::Cloudflare.args(&route),
            [
                "tunnel",
                "--url",
                "http://127.0.0.1:4321",
                "--http-host-header",
                "fix-ui.shop.localhost",
            ]
        );
        assert_eq!(
            Provider::Ngrok.args(&route),
            [
                "http",
                "http://127.0.0.1:4321",
                "--host-header=fix-ui.shop.localhost",
            ]
        );
        assert!(
            Provider::Cloudflare
                .managed_args(&route)
                .contains(&"--no-autoupdate".into())
        );
        assert!(
            Provider::Ngrok
                .managed_args(&route)
                .contains(&"--log=stdout".into())
        );
    }

    #[test]
    fn lookup_requires_an_executable_file() {
        let exe = env::current_exe().unwrap();
        let parent = exe.parent().unwrap();
        assert_eq!(
            find_executable(
                exe.file_name().unwrap().to_str().unwrap(),
                parent.as_os_str()
            ),
            Some(exe.clone())
        );
        assert!(find_executable(".", parent.as_os_str()).is_none());
        assert!(find_executable("missing-doorman-provider", parent.as_os_str()).is_none());
    }
}
