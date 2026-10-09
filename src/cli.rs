use crate::{
    config::{Config, Locations},
    model::{now, DataState, Snapshot},
    scheduler, storage,
};
use std::io::IsTerminal;
pub fn render(snapshot: &Snapshot, compact: bool, metric: &str, color: bool) -> String {
    let mut lines = vec![];
    for p in &snapshot.providers {
        let name = match p.provider.as_str() {
            "codex" => "Codex",
            "opencode-go" => "Go",
            s => s,
        };
        let profile = if p.profile == "default" {
            String::new()
        } else {
            format!(" [{}]", p.profile)
        };
        let marker = if p.state == DataState::Fresh {
            ""
        } else {
            " ~"
        };
        if p.quotas.is_empty() {
            lines.push(if compact {
                format!("{name}{profile}: ?")
            } else {
                format!(
                    "{name}{profile}: unavailable [{:?}]{}",
                    p.state,
                    p.error
                        .as_ref()
                        .map(|e| format!("\n  error: {}", e.message))
                        .unwrap_or_default()
                )
            });
            continue;
        }
        for q in &p.quotas {
            let percent = if metric == "used" {
                q.used_percent
            } else {
                q.remaining_percent
            };
            let value = format!("{percent:.0}%");
            let value = if color {
                format!(
                    "\x1b[{}m{value}\x1b[0m",
                    if q.remaining_percent <= 10.0 {
                        31
                    } else if q.remaining_percent <= 20.0 {
                        33
                    } else {
                        32
                    }
                )
            } else {
                value
            };
            if compact {
                lines.push(format!("{name}{profile} {}: {value}{marker}", q.label));
            } else {
                lines.push(format!("{name}{profile} {}: {:.1}% remaining / {:.1}% used [{:?}]\n  reset: {} | last success: {}\n  source: {}{}",q.label,q.remaining_percent,q.used_percent,p.state,q.resets_at.map(|t|t.to_string()).unwrap_or_else(||"not reported".into()),p.last_success_at.map(|t|t.to_string()).unwrap_or_else(||"never".into()),p.source,p.error.as_ref().map(|e|format!("\n  error: {}",e.message)).unwrap_or_default()));
            }
        }
    }
    lines.join(if compact { " | " } else { "\n" })
}
const HELP:&str="AI Quota Monitor\n\nCommands:\n  status [--json|--compact] [--provider codex|opencode-go]\n  refresh [--provider PROVIDER]\n  daemon\n  doctor [--json]\n  history [--days N] [--provider PROVIDER]\n  export [--days N] [--provider PROVIDER]\n  migrate --from DIRECTORY --provider PROVIDER\n\nOptions: --config PATH, --state-dir PATH, --cache PATH, --color auto|always|never, --metric remaining|used\nstatus never performs network requests. '~' means stale; '?' means unavailable.\n";
pub fn run(args: Vec<String>) -> Result<i32, String> {
    let mut loc = Locations::default();
    let mut command = "status".to_string();
    let mut provider = None;
    let mut json = false;
    let mut compact = false;
    let mut color = None;
    let mut metric = None;
    let mut days = 7;
    let mut from = None;
    let mut iter = args.into_iter();
    let mut seen_command = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(0);
            }
            "--version" | "-V" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            "--json" => json = true,
            "--compact" => compact = true,
            "--config" => loc.config = iter.next().ok_or("Missing --config value")?.into(),
            "--state-dir" => loc.state = iter.next().ok_or("Missing --state-dir value")?.into(),
            "--cache" => loc.cache = iter.next().ok_or("Missing --cache value")?.into(),
            "--provider" => provider = Some(iter.next().ok_or("Missing provider")?),
            "--color" => color = Some(iter.next().ok_or("Missing color")?),
            "--metric" => metric = Some(iter.next().ok_or("Missing metric")?),
            "--days" => {
                days = iter
                    .next()
                    .ok_or("Missing days")?
                    .parse::<u32>()
                    .map_err(|_| "Invalid days")?
            }
            "--from" => {
                from = Some(std::path::PathBuf::from(
                    iter.next().ok_or("Missing source")?,
                ))
            }
            s if !s.starts_with('-') && !seen_command => {
                command = s.into();
                seen_command = true
            }
            _ => return Err("Unknown argument; use --help".into()),
        }
    }
    if provider
        .as_ref()
        .is_some_and(|p| !["codex", "opencode-go"].contains(&p.as_str()))
        || !(1..=3650).contains(&days)
    {
        return Err("Invalid provider or days".into());
    }
    let config = Config::load(&loc.config)?;
    let metric = metric.unwrap_or_else(|| config.compact_metric.clone());
    let color = color.unwrap_or_else(|| config.compact_color.clone());
    if !["remaining", "used"].contains(&metric.as_str())
        || !["always", "never", "auto"].contains(&color.as_str())
        || json && compact
    {
        return Err("Invalid output options".into());
    }
    let color = (color == "always" || color == "auto" && std::io::stdout().is_terminal())
        && std::env::var_os("NO_COLOR").is_none();
    let output = |mut snapshot: Snapshot| -> Result<(), String> {
        snapshot = snapshot.aged(now());
        if let Some(id) = &provider {
            snapshot.providers.retain(|p| p.provider == *id);
        }
        if json {
            println!(
                "{}",
                serde_json::to_string(&snapshot).map_err(|_| "Serialization failed")?
            );
        } else {
            println!("{}", render(&snapshot, compact, &metric, color));
        }
        Ok(())
    };
    match command.as_str() {
        "status" => {
            let snapshot = if loc.cache.exists() {
                storage::read_cache(&loc.cache)?
            } else {
                storage::empty(&config)
            };
            output(snapshot)?;
            Ok(0)
        }
        "refresh" => {
            let (snapshot, failed) = scheduler::refresh(&config, &loc, provider.as_deref(), true)?;
            let pending_error = snapshot
                .providers
                .iter()
                .any(|p| provider.as_ref().is_none_or(|id| p.provider == *id) && p.error.is_some());
            output(snapshot)?;
            Ok(if failed || pending_error { 1 } else { 0 })
        }
        "daemon" => {
            scheduler::daemon(&config, &loc)?;
            Ok(0)
        }
        "history" | "export" => {
            print!(
                "{}",
                storage::history(&loc, days, command == "export", provider.as_deref())?
            );
            Ok(0)
        }
        "doctor" => {
            let cache = storage::read_cache(&loc.cache).map(|s| s.aged(now()));
            let mut checks = vec![];
            checks.push(serde_json::json!({"check":"configuration","ok":true,"path":loc.config}));
            checks.push(serde_json::json!({"check":"cache","ok":cache.is_ok(),"path":loc.cache,"error":cache.as_ref().err()}));
            for p in config.providers.iter().filter(|p| p.enabled) {
                let ok = if p.provider == "opencode-go" {
                    crate::providers::go::credentials(p).is_ok()
                } else {
                    find_bin(&p.codex_bin)
                };
                checks.push(serde_json::json!({"check":"provider_prerequisite","provider":p.provider,"profile":p.profile,"ok":ok}));
            }
            let service = std::process::Command::new("systemctl")
                .args([
                    "--user",
                    "show",
                    "ai-quota-monitor.service",
                    "-p",
                    "ActiveState",
                    "-p",
                    "SubState",
                    "-p",
                    "NRestarts",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
            checks.push(serde_json::json!({"check":"service","details":service}));
            if json {
                println!(
                    "{}",
                    serde_json::json!({"schema_version":1,"checks":checks,"snapshot":cache.ok()})
                );
            } else {
                for c in checks {
                    println!("{c}");
                }
            }
            Ok(0)
        }
        "migrate" => {
            crate::storage::migrate(
                &config,
                &loc,
                &from.ok_or("migrate requires --from")?,
                provider.as_deref().ok_or("migrate requires --provider")?,
            )?;
            println!("Legacy history imported; source unchanged");
            Ok(0)
        }
        _ => Err("Unknown command; use --help".into()),
    }
}
fn find_bin(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &std::path::Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if path.components().count() > 1 {
        return executable(path);
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| executable(&dir.join(path))))
}
