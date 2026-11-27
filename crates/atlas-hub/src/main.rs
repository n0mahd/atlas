//! atlas-hub: the composition root for the hub.
//!
//! Reads its config (default `~/.config/atlas/hub.toml`, or the path in `ATLAS_HUB_CONFIG`),
//! then serves the UI on a Unix socket readable only by this user. `tailscale serve`
//! puts HTTPS and the tailnet identity in front of it.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use atlas_fs::{Sources, discover_projects};
use atlas_web::{Access, AppState};
use serde::Deserialize;
use tokio::net::UnixListener;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// The socket to listen on. Default: `$XDG_RUNTIME_DIR/atlas/hub.sock`.
    socket: Option<String>,
    /// Name shown in the UI. Default: this machine's hostname.
    hub_name: Option<String>,
    /// Tailscale logins allowed in, such as `user@github`.
    allowed_logins: Vec<String>,
    knowledge: String,
    public_skills: Option<String>,
    /// Folder whose git repos with agent files count as projects.
    projects_dir: Option<String>,
}

fn expand(home: &Path, p: &str) -> PathBuf {
    p.strip_prefix("~/")
        .map_or_else(|| PathBuf::from(p), |rest| home.join(rest))
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_owned())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "hub".to_owned())
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("atlas-hub: {msg}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| fail("HOME is not set")));
    let config_path = std::env::var_os("ATLAS_HUB_CONFIG")
        .map_or_else(|| home.join(".config/atlas/hub.toml"), PathBuf::from);
    let text = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| fail(format!("can't read {}: {e}", config_path.display())));
    let config: Config = toml::from_str(&text)
        .unwrap_or_else(|e| fail(format!("{} isn't valid: {e}", config_path.display())));
    if config.allowed_logins.is_empty() {
        fail("allowed_logins is empty, so nobody could get in");
    }

    let projects = config.projects_dir.as_deref().map_or_else(Vec::new, |dir| {
        let (projects, problems) = discover_projects(&expand(&home, dir));
        for p in problems {
            tracing::warn!("{}: {}", p.path, p.message);
        }
        projects
    });
    tracing::info!("{} projects registered", projects.len());
    let sources = Sources {
        knowledge: expand(&home, &config.knowledge),
        public_skills: config.public_skills.as_deref().map(|p| expand(&home, p)),
        projects,
        home: home.clone(),
    };
    let hub_name = config.hub_name.unwrap_or_else(hostname);
    let app = atlas_web::router(
        AppState::new(sources, &hub_name),
        Access::new(config.allowed_logins),
    );

    let socket = config.socket.map_or_else(
        || {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .unwrap_or_else(|| fail("XDG_RUNTIME_DIR is not set; give `socket` in the config"));
            PathBuf::from(runtime).join("atlas/hub.sock")
        },
        |s| expand(&home, &s),
    );
    if let Some(dir) = socket.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .unwrap_or_else(|e| fail(format!("can't create {}: {e}", dir.display())));
    }
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => fail(format!("can't replace {}: {e}", socket.display())),
    }
    let listener = UnixListener::bind(&socket)
        .unwrap_or_else(|e| fail(format!("can't listen on {}: {e}", socket.display())));
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|e| fail(format!("can't restrict {}: {e}", socket.display())));
    check_private(&socket);
    tracing::info!("listening on {}", socket.display());

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // systemd stops services with SIGTERM; Ctrl-C covers running by hand.
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .unwrap_or_else(|e| fail(format!("can't watch for SIGTERM: {e}")));
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        })
        .await
        .unwrap_or_else(|e| fail(e));
}

/// Refuses to serve if the socket's folder could let another user in: it must belong to
/// the same user as the socket and allow nobody else.
fn check_private(socket: &Path) {
    use std::os::unix::fs::MetadataExt;
    let Some(dir) = socket.parent() else { return };
    let (Ok(sock), Ok(folder)) = (std::fs::metadata(socket), std::fs::metadata(dir)) else {
        fail(format!("can't inspect {}", dir.display()));
    };
    if folder.uid() != sock.uid() || folder.mode() & 0o077 != 0 {
        fail(format!(
            "{} must belong to this user and be closed to others (chmod 700)",
            dir.display()
        ));
    }
}
