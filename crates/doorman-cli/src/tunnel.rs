use std::{
    env,
    os::unix::process::CommandExt,
    process::{Command, ExitCode},
};

use anyhow::{Context, Result};
use clap::ValueEnum;
use doorman_core::{Request, Response, tunnel};

use crate::client;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Provider {
    Auto,
    Cloudflare,
    Ngrok,
}

impl From<Provider> for tunnel::Provider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Auto => Self::Auto,
            Provider::Cloudflare => Self::Cloudflare,
            Provider::Ngrok => Self::Ngrok,
        }
    }
}

pub fn run(name: &str, requested: Provider) -> Result<ExitCode> {
    let response = client::call(&Request::List)?;
    let Response::Routes { routes, .. } = response else {
        return Err(client::unexpected(response));
    };
    let route = routes
        .iter()
        .find(|route| route.name == name)
        .with_context(|| {
            format!("route '{name}' not found; use `doorman list` to see running routes")
        })?;
    let path = env::var_os("PATH").unwrap_or_default();
    let (provider, executable) = tunnel::select(requested.into(), |binary| {
        tunnel::find_executable(binary, &path)
    })
    .map_err(anyhow::Error::msg)?;
    eprintln!(
        "Publicly exposing '{name}' (port {}) with {}. Anyone with the public URL can access this app.\n\
         Stop with Ctrl-C. The provider will print the public URL below.",
        route.port,
        provider.binary()
    );
    if provider == tunnel::Provider::Ngrok {
        eprintln!(
            "ngrok requires an account and a configured authtoken (`ngrok config add-authtoken`)."
        );
    }
    // Replace the CLI, preserving stdio, exit status, and signal delivery.
    Err(Command::new(&executable).args(provider.args(route)).exec())
        .with_context(|| format!("could not start {}", executable.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_cli_parses_default_and_explicit_provider() {
        use clap::Parser;
        for (args, expected) in [
            (vec!["doorman", "tunnel", "shop"], Provider::Auto),
            (
                vec!["doorman", "tunnel", "shop", "--provider", "ngrok"],
                Provider::Ngrok,
            ),
        ] {
            let cli = crate::Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                Some(crate::Command::Tunnel { name, provider })
                    if name == "shop" && provider == expected
            ));
        }
        assert!(crate::Cli::try_parse_from(["doorman", "tunnel"]).is_err());
        assert!(
            crate::Cli::try_parse_from(["doorman", "tunnel", "shop", "--provider", "unknown"])
                .is_err()
        );
    }
}
