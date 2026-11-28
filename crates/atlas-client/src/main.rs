//! atlasd: keeps one login's harnesses in step with the owner's signed releases.
//!
//! Runs as the login (a systemd user timer, or a launchd agent on macOS), never as root,
//! and never runs anything it receives. What it prints calls releases "updates" and
//! harnesses "agents", as the hub's pages do.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use atlas_client::config::{CliConfig, Config, Paths, State};
use atlas_client::pass::{self, Fetched};
use atlas_client::{http, release, ssh};
use atlas_core::distribution::PairRequest;

const USAGE: &str = "\
usage:
  atlasd pair <hub-url> <code> [--machine NAME] [--adopt DIR]... [--memory-store DIR]
                                                  pair this login with the hub; --adopt lets
                                                  atlasd take over links an older script made
                                                  into DIR (such as ~/Knowledge);
                                                  --memory-store names your own checkout of
                                                  the knowledge repo for memory links
  atlasd run [--dry-run | --summary]              apply the latest signed update; --dry-run
                                                  shows what would change, --summary says
                                                  what changed in two or three lines
  atlasd status                                   show the pairing and the pending plan
  atlasd auth <hub-url> <token>                   save a CLI token from Settings → CLI tokens
  atlasd pair-code                                get a one-time install command from the hub
  atlasd machines [list]                          list machines on the hub
  atlasd machines add <you@host>                  install and pair atlasd over SSH from here
  atlasd machines track <name>                    add an unpaired machine name to the hub
  atlasd machines remove <name-or-id>             remove a machine or unpair a client id";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = home().and_then(|home| {
        let paths = Paths::new(&home);
        match args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice()
        {
            ["pair", hub, code, rest @ ..] => {
                let mut machine = None;
                let mut adopt = Vec::new();
                let mut memory_store = None;
                let mut opts = rest.iter();
                loop {
                    match (opts.next(), opts.next()) {
                        (None, _) => break,
                        (Some(&"--machine"), Some(name)) => machine = Some(*name),
                        (Some(&"--adopt"), Some(dir)) => adopt.push((*dir).to_owned()),
                        (Some(&"--memory-store"), Some(dir)) => {
                            memory_store = Some((*dir).to_owned());
                        }
                        _ => return Err(USAGE.to_owned()),
                    }
                }
                pair(&paths, hub, code, machine, adopt, memory_store)
            }
            ["run"] => run(&paths, Mode::Full),
            ["run", "--dry-run"] => run(&paths, Mode::DryRun),
            ["run", "--summary"] => run(&paths, Mode::Summary),
            ["status"] => status(&paths),
            ["auth", hub, token] => auth(&paths, hub, token),
            ["pair-code"] => pair_code(&paths),
            ["machines"] | ["machines", "list"] => machines_list(&paths),
            ["machines", "add", target] => machines_add(&paths, target),
            ["machines", "track", name] => machines_track(&paths, name),
            ["machines", "remove", target] => machines_remove(&paths, target),
            ["-h" | "--help" | "help"] => {
                println!("{USAGE}");
                Ok(ExitCode::SUCCESS)
            }
            _ => Err(USAGE.to_owned()),
        }
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("atlasd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| "HOME isn't set to an absolute path".to_owned())
}

/// This machine's short name. Linux keeps it in a file; macOS has neither file, so ask
/// `hostname -s` and lowercase it (`MacBook-Air` is `macbook-air` on the tailnet).
fn hostname() -> String {
    ["/proc/sys/kernel/hostname", "/etc/hostname"]
        .iter()
        .find_map(|f| std::fs::read_to_string(f).ok())
        .or_else(|| {
            std::process::Command::new("hostname")
                .arg("-s")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.to_lowercase())
        })
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn pair(
    paths: &Paths,
    hub: &str,
    code: &str,
    machine: Option<&str>,
    adopt_dirs: Vec<String>,
    memory_store: Option<String>,
) -> Result<ExitCode, String> {
    let (origin, _) = release::origin_and_rp(hub)?;
    let request = PairRequest {
        code: code.to_owned(),
        hub: origin.clone(),
        machine: machine.map_or_else(hostname, str::to_owned),
        user: std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned()),
    };
    let resp = http::pair(&origin, &request)?;
    if release::origin_and_rp(&resp.hub)?.0 != origin {
        return Err(format!(
            "the hub calls itself {}, not {origin}; pair using that address",
            resp.hub
        ));
    }
    if resp.owner_keys.is_empty() {
        return Err("the hub has no owner passkey yet, so no update could be trusted".to_owned());
    }
    // Folders to take over: those given now, plus any from an earlier pairing, whose
    // project roots and memory store are kept too.
    let earlier = Config::load(&paths.config).ok();
    let mut adopt = earlier.as_ref().map_or_else(Vec::new, |c| c.adopt.clone());
    for dir in adopt_dirs {
        if !adopt.contains(&dir) {
            adopt.push(dir);
        }
    }
    Config {
        hub: resp.hub,
        client_id: resp.client_id,
        token: resp.token,
        machine: request.machine.clone(),
        adopt,
        project_roots: earlier
            .as_ref()
            .map_or_else(atlas_client::config::default_project_roots, |c| {
                c.project_roots.clone()
            }),
        memory_store: memory_store.or_else(|| earlier.and_then(|c| c.memory_store)),
    }
    .save(&paths.config)?;
    State {
        owner_keys: resp.owner_keys,
        last_applied: None,
        last_digest: None,
    }
    .save(&paths.state)?;
    println!(
        "Paired {} ({}) with {origin}.",
        request.machine, request.user
    );
    Ok(ExitCode::SUCCESS)
}

fn fetch(config: &Config) -> Fetched {
    match http::release(&config.hub, &config.token) {
        Ok(Some(r)) => Fetched::Release(Box::new(r)),
        Ok(None) => Fetched::Nothing,
        Err(http::ReleaseError::Unpaired) => Fetched::Unpaired,
        Err(http::ReleaseError::Failed(e)) => Fetched::Unreachable(e),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Full,
    DryRun,
    /// Applies like `Full` but prints the pass's two or three summary lines, and leaves
    /// out the things it left alone (`atlasd status` lists them).
    Summary,
}

fn run(paths: &Paths, mode: Mode) -> Result<ExitCode, String> {
    let dry_run = mode == Mode::DryRun;
    let config = Config::load(&paths.config)?;
    let state = State::load(&paths.state)?;
    let fetched = fetch(&config);
    let reachable = !matches!(fetched, Fetched::Unreachable(_) | Fetched::Unpaired);
    let pass = pass::run(paths, &config, &state, fetched, dry_run, now());
    let summary = mode == Mode::Summary;
    let lines = if summary { &pass.summary } else { &pass.lines };
    for line in lines {
        println!("{line}");
    }
    for p in &pass.problems {
        if !(summary && pass.left_alone.contains(p)) {
            eprintln!("problem: {p}");
        }
    }
    if !dry_run {
        if pass.state != state {
            pass.state.save(&paths.state)?;
        }
        if reachable && let Err(e) = http::report(&config.hub, &config.token, &pass.report) {
            eprintln!("problem: couldn't send the report: {e}");
        }
    }
    Ok(if pass.failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn status(paths: &Paths) -> Result<ExitCode, String> {
    let config = Config::load(&paths.config)?;
    let state = State::load(&paths.state)?;
    println!("Hub: {}", config.hub);
    println!("Machine: {}", config.machine);
    println!(
        "Last applied update: {}",
        state
            .last_applied
            .map_or_else(|| "none".to_owned(), |n| n.to_string())
    );
    let names: Vec<&str> = state.owner_keys.iter().map(|k| k.name.as_str()).collect();
    println!("Trusted passkeys: {}", names.join(", "));
    println!("Plan for the hub's latest update:");
    let pass = pass::run(paths, &config, &state, fetch(&config), true, now());
    for line in &pass.lines {
        println!("  {line}");
    }
    for p in &pass.problems {
        println!("  problem: {p}");
    }
    Ok(ExitCode::SUCCESS)
}

fn auth(paths: &Paths, hub: &str, token: &str) -> Result<ExitCode, String> {
    let (origin, _) = release::origin_and_rp(hub)?;
    let token = token.trim();
    let list = http::admin_machines(&origin, token)?;
    CliConfig {
        hub: origin.clone(),
        token: token.to_owned(),
    }
    .save(&paths.cli)?;
    println!(
        "Saved CLI token for {origin} (hub: {}, {} machines).",
        list.hub,
        list.machines.len()
    );
    Ok(ExitCode::SUCCESS)
}

fn pair_code(paths: &Paths) -> Result<ExitCode, String> {
    let cli = CliConfig::load(&paths.cli)?;
    let resp = http::admin_pair_code(&cli.hub, &cli.token)?;
    println!(
        "On the machine, as the account Atlas should manage (works once, for {} minutes):\n\n  {}\n\nOr from here over SSH:\n\n  ssh you@machine '{}'",
        resp.ttl_seconds / 60,
        resp.command,
        resp.command
    );
    Ok(ExitCode::SUCCESS)
}

fn machines_list(paths: &Paths) -> Result<ExitCode, String> {
    let cli = CliConfig::load(&paths.cli)?;
    let resp = http::admin_machines(&cli.hub, &cli.token)?;
    let latest = resp.latest_release;
    for m in &resp.machines {
        if m.hub {
            println!("{}  [hub]", m.name);
        } else if m.clients.is_empty() {
            println!("{}  (not paired yet)", m.name);
        } else {
            println!("{}  ({} paired)", m.name, m.clients.len());
        }
        for c in &m.clients {
            let status = match (c.release, latest) {
                (_, None) => "no update yet".to_owned(),
                (Some(a), Some(l)) if a >= l => format!("update {a}, up to date"),
                (Some(a), Some(l)) => format!("update {a}, {} behind", l - a),
                (None, Some(_)) => "hasn't applied an update".to_owned(),
            };
            println!("  {}  id={}  {}", c.user, c.id, status);
            for p in &c.problems {
                println!("    problem: {p}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn machines_add(paths: &Paths, target: &str) -> Result<ExitCode, String> {
    let cli = CliConfig::load(&paths.cli)?;
    let target = ssh::Target::parse(target)?;
    let resp = http::admin_pair_code(&cli.hub, &cli.token)?;
    let (resp_origin, _) = release::origin_and_rp(&resp.origin)?;
    if resp_origin != cli.hub {
        return Err(format!(
            "the hub answered with origin {resp_origin}, not {}; refusing to run install",
            cli.hub
        ));
    }
    eprintln!("Installing atlasd on {target} over SSH…");
    ssh::install_over_ssh(&cli.hub, &resp.code, &target)?;
    println!(
        "\nInstalled and paired on {target}. Run `atlasd machines` to check its status once it reports."
    );
    Ok(ExitCode::SUCCESS)
}

fn machines_track(paths: &Paths, name: &str) -> Result<ExitCode, String> {
    let cli = CliConfig::load(&paths.cli)?;
    let added = http::admin_add_machine(&cli.hub, &cli.token, name)?;
    println!("Added {added} to the hub's machine list.");
    Ok(ExitCode::SUCCESS)
}

fn machines_remove(paths: &Paths, target: &str) -> Result<ExitCode, String> {
    let cli = CliConfig::load(&paths.cli)?;
    let res = http::admin_remove_machine(&cli.hub, &cli.token, target)?;
    if res.removed_machine && res.removed_clients > 0 {
        println!(
            "Removed {target} and unpaired {} client(s). Their tokens no longer work.",
            res.removed_clients
        );
    } else if res.removed_machine {
        println!("Removed {target} from the hub's machine list.");
    } else {
        println!(
            "Unpaired {} client(s) matching {target}. Their tokens no longer work.",
            res.removed_clients
        );
    }
    Ok(ExitCode::SUCCESS)
}
