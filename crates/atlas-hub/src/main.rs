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
    /// The user's other machines, shown as not paired until they pair (phase 3).
    #[serde(default)]
    machines: Vec<String>,
    /// The address people open, such as `https://hub.example.ts.net:7443`. Passkeys are
    /// tied to it; without it Atlas stays read-only.
    public_url: Option<String>,
    /// Passkeys and settings. Default: `$STATE_DIRECTORY`, else `~/.local/state/atlas-hub`.
    state_dir: Option<String>,
    /// Minutes after start during which the first passkey can be made from the browser by
    /// an allowed Tailscale login, while none exists. Default 30; 0 turns it off.
    first_run_minutes: Option<u64>,
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

fn load_config(home: &Path) -> Config {
    let config_path = std::env::var_os("ATLAS_HUB_CONFIG")
        .map_or_else(|| home.join(".config/atlas/hub.toml"), PathBuf::from);
    let text = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| fail(format!("can't read {}: {e}", config_path.display())));
    toml::from_str(&text)
        .unwrap_or_else(|e| fail(format!("{} isn't valid: {e}", config_path.display())))
}

fn state_dir(home: &Path, config: &Config) -> PathBuf {
    config.state_dir.as_deref().map_or_else(
        || {
            std::env::var_os("STATE_DIRECTORY")
                .map_or_else(|| home.join(".local/state/atlas-hub"), PathBuf::from)
        },
        |d| expand(home, d),
    )
}

const USAGE: &str = "usage: atlas-hub                         serve the hub
       atlas-hub setup                   print a one-time link (and QR code) to add a passkey
       atlas-hub pair-code               print a one-time command that installs and pairs atlasd
       atlas-hub add-machine <you@host>  install and pair atlasd on a machine over SSH
       atlas-hub token <name>            create a CLI token for managing machines remotely";

/// The hub's public origin from `public_url`.
fn public_origin(config: &Config) -> String {
    let Some(url) = config.public_url.as_deref() else {
        fail("public_url isn't set in the config, so machines can't pair with this hub");
    };
    atlas_signer::Url::parse(url)
        .unwrap_or_else(|e| fail(format!("public_url isn't a valid address: {e}")))
        .origin()
        .ascii_serialization()
}

/// The hub's origin and a fresh pairing code, for `pair-code` and `add-machine`.
fn pairing(home: &Path, config: &Config) -> (String, String) {
    let origin = public_origin(config);
    let code = atlas_signer::setup::issue_pairing(
        &state_dir(home, config),
        atlas_web::remote::PAIRING_TTL,
    )
    .unwrap_or_else(|e| fail(format!("can't write the pairing code: {e}")));
    (origin, code)
}

/// `atlas-hub token <name>`: creates a revocable CLI token for another workstation.
fn create_token(home: &Path, config: &Config, name: &str) {
    let origin = public_origin(config);
    let tokens = atlas_web::tokens::CliTokens::open(&state_dir(home, config));
    let (record, raw) = tokens.create(name).unwrap_or_else(|e| fail(e));
    println!(
        "Created CLI token \"{}\" (shown once; revoke anytime in Settings → CLI tokens).\n\nRun on that machine:\n\n  atlasd auth {origin} {raw}",
        record.name
    );
}

/// `atlas-hub pair-code`: the command to run on a machine, for copying by hand.
fn pair_code(home: &Path, config: &Config) {
    let (origin, code) = pairing(home, config);
    let cmd = atlas_web::remote::install_command(&origin, &code);
    println!(
        "On the machine, as the account Atlas should manage (works once, for 15 minutes):\n\n  {cmd}\n\nOr from another computer:\n\n  ssh you@machine '{cmd}'"
    );
}

/// `atlas-hub add-machine <target>`: installs and pairs atlasd there over SSH.
async fn add_machine(home: &Path, config: &Config, target: &str) {
    let target = atlas_web::remote::Target::parse(target).unwrap_or_else(|e| fail(e));
    let (origin, code) = pairing(home, config);
    eprintln!("Installing atlasd on {target} over SSH…");
    match atlas_web::remote::install_over_ssh(&origin, &code, &target).await {
        Ok(done) => {
            print!("{}", done.output);
            println!(
                "\nInstalled and paired on {target}. It shows on the Machines page once atlasd reports, within a minute."
            );
        }
        Err(e) => {
            if !e.output.is_empty() {
                eprintln!("{}\n", e.output);
            }
            fail(e.message);
        }
    }
}

/// `atlas-hub setup`: a one-time link to add a passkey, for first setup or lost passkeys.
/// Whoever can run this on the hub already controls it.
fn setup(home: &Path, config: &Config) {
    let Some(url) = config.public_url.as_deref() else {
        fail("public_url isn't set in the config, so Atlas is read-only and has no passkeys");
    };
    let ttl = std::time::Duration::from_mins(15);
    let token = atlas_signer::setup::issue(&state_dir(home, config), ttl)
        .unwrap_or_else(|e| fail(format!("can't write the setup link: {e}")));
    let link = format!("{}/setup/{token}", url.trim_end_matches('/'));
    println!("Open this link to add a passkey (works once, for 15 minutes):\n\n  {link}\n");
    if let Ok(code) = qrcode::QrCode::new(link.as_bytes()) {
        let qr = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build();
        println!("Or scan it with your phone:\n\n{qr}");
    }
}

/// Runs a subcommand, if one was given. Returns false to serve the hub.
async fn subcommand(home: &Path, args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        None => return false,
        Some("setup") => setup(home, &load_config(home)),
        Some("pair-code") => pair_code(home, &load_config(home)),
        Some("add-machine") => {
            let [_, target] = args else {
                fail(format!(
                    "add-machine takes one SSH address, such as you@laptop\n{USAGE}"
                ));
            };
            add_machine(home, &load_config(home), target).await;
        }
        Some("token") => {
            let [_, name] = args else {
                fail(format!("token takes a name, such as workstation\n{USAGE}"));
            };
            create_token(home, &load_config(home), name);
        }
        Some("-h" | "--help" | "help") => println!("{USAGE}"),
        Some(other) => fail(format!("unknown command {other:?}\n{USAGE}")),
    }
    true
}

#[tokio::main]
async fn main() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| fail("HOME is not set")));
    let args: Vec<String> = std::env::args().skip(1).collect();
    if subcommand(&home, &args).await {
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let config = load_config(&home);
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
    let hub_name = config.hub_name.clone().unwrap_or_else(hostname);
    let first_run = std::time::Duration::from_mins(config.first_run_minutes.unwrap_or(30));
    let writer = config.public_url.as_deref().map(|url| {
        atlas_web::Writer::open(url, &state_dir(&home, &config), first_run)
            .unwrap_or_else(|e| fail(format!("can't set up passkeys: {e}")))
    });
    match &writer {
        None => tracing::info!("no public_url in the config, so Atlas is read-only"),
        Some(w) if w.first_run_open() => tracing::warn!(
            "no passkey yet: open {}/signin within {} minutes to create one, or run `atlas-hub setup` for a link",
            w.origin(),
            first_run.as_secs() / 60
        ),
        Some(w) if !w.signer.has_keys() => {
            tracing::warn!("no passkey yet: run `atlas-hub setup` for a link to create one");
        }
        Some(_) => {}
    }
    let state = AppState::new(sources, &hub_name, config.machines, writer);
    let app = atlas_web::router(state, Access::new(config.allowed_logins));

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
