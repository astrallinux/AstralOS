use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ast_parser::{parse, AstBlock, AstValue, State};
use clap::{Parser, Subcommand};

const SNAPSHOT_DIR: &str = "/var/lib/astralos/snapshots";
const MAX_SNAPSHOTS: usize = 10;
const CLEAN_MAX_SNAPSHOTS: usize = 5;

#[derive(Parser)]
#[command(name = "astctl", version, about = "AST configuration tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Diff declared packages against system state
    Diff {
        /// Path to the .ast file to diff
        #[arg(value_name = "FILE")]
        file_path: String,
    },
    /// Apply declared package states to the system
    Apply {
        /// Path to the .ast file to apply
        #[arg(value_name = "FILE")]
        file_path: String,
        /// Print commands that would be run without executing them
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
    /// Sync system state to .ast file (add missing installed packages)
    Sync {
        /// Path to the .ast file to sync
        #[arg(value_name = "FILE")]
        file_path: String,
        /// Print the new .ast content without overwriting the file
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Suppress all non-error output
        #[arg(long, short = 'q')]
        quiet: bool,
    },
    /// Rollback to the most recent snapshot
    Rollback {
        /// Path to the .ast file to rollback
        #[arg(value_name = "FILE")]
        file_path: String,
        /// Print commands that would be run without executing them
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
    /// Clean up old snapshots and orphaned packages
    Clean {
        /// Print commands that would be run without executing them
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Diff { file_path } => {
            if let Err(e) = diff_file(&file_path) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Apply { file_path, dry_run } => {
            if let Err(e) = apply_file(&file_path, dry_run) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Sync { file_path, dry_run, quiet } => {
            if let Err(e) = sync_file(&file_path, dry_run, quiet) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Rollback { file_path, dry_run } => {
            if let Err(e) = rollback_file(&file_path, dry_run) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
        Commands::Clean { dry_run } => {
            if let Err(e) = clean_file(dry_run) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }
}

fn diff_file(file_path: &str) -> Result<(), String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let blocks = parse(&content)?;
    let packages = extract_packages(&blocks)?;
    let (to_install, to_remove, correct, missing) = compute_diff_from_packages(&packages)?;

    // If nothing to do, print short message
    if to_install.is_empty() && to_remove.is_empty() && missing.is_empty() {
        println!("\x1b[32m✓ System is up to date. No changes needed.\x1b[0m");
        return Ok(());
    }

    print_diff_summary(&to_install, &to_remove, &correct, &missing);
    Ok(())
}

fn apply_file(file_path: &str, dry_run: bool) -> Result<(), String> {
    // Read and parse the entire file
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let blocks = parse(&content)?;

    // Extract packages for the existing diff logic
    let packages = extract_packages(&blocks)?;
    let (to_install, to_remove, correct, missing) = compute_diff_from_packages(&packages)?;

    // Check for Unknown packages - we can't proceed if system state is unknown
    if !missing.is_empty() {
        print_diff_summary(&to_install, &to_remove, &correct, &missing);
        eprintln!("\n\x1b[31mError: Cannot apply changes - the following packages have unknown system state:\x1b[0m");
        for pkg in &missing {
            eprintln!("  \x1b[31m{}\x1b[0m", pkg);
        }
        eprintln!("\n\x1b[31mPlease ensure dpkg-query is available and try again.\x1b[0m");
        return Err("Unknown package state prevents safe apply".to_string());
    }

    // Collect all commands to run
    let mut all_commands: Vec<Vec<String>> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 1. Handle SYSTEM block
    let system_commands = handle_system_block(&blocks, dry_run)?;
    all_commands.extend(system_commands);

    // 2. Handle USERS block
    let (user_commands, user_warnings) = handle_users_block(&blocks, dry_run)?;
    all_commands.extend(user_commands);
    warnings.extend(user_warnings);

    // 3. Handle SERVICES block
    let (service_commands, service_warnings) = handle_services_block(&blocks, dry_run)?;
    all_commands.extend(service_commands);
    warnings.extend(service_warnings);

    // 4. Handle PACKAGES block (with version = "latest" support)
    let (package_commands, source_builds) = handle_packages_block(&blocks, &to_install, &to_remove, dry_run)?;
    all_commands.extend(package_commands);

    // Print summary
    if to_install.is_empty() && to_remove.is_empty() && all_commands.is_empty() && warnings.is_empty() {
        println!("\x1b[32m✓ System is up to date. No changes needed.\x1b[0m");
        return Ok(());
    }

    print_diff_summary(&to_install, &to_remove, &correct, &missing);

    // Print warnings
    for warning in &warnings {
        println!("\x1b[33mWarning: {}\x1b[0m", warning);
    }

    if dry_run {
        println!("\n\x1b[33m[DRY RUN] The following commands would be executed:\x1b[0m");
        for cmd in &all_commands {
            println!("  {}", cmd.join(" "));
        }
        println!("\n\x1b[33m[DRY RUN] No changes were made.\x1b[0m");
        return Ok(());
    }

    print!("\nProceed with these changes? [y/N] ");
    io::stdout().flush().map_err(|e| format!("Failed to flush stdout: {}", e))?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| format!("Failed to read input: {}", e))?;

    let confirmed = input.trim().eq_ignore_ascii_case("y");

    if !confirmed {
        println!("Aborted.");
        return Ok(());
    }

    // Create snapshot before making changes
    create_snapshot(file_path)?;

    // Execute all commands
    for cmd in &all_commands {
        let status = Command::new(&cmd[0])
            .args(&cmd[1..])
            .status()
            .map_err(|e| format!("Failed to run '{}': {}", cmd.join(" "), e))?;

        if !status.success() {
            return Err(format!("Command failed: {}", cmd.join(" ")));
        }
    }

    // Execute source builds if any
    if !source_builds.is_empty() {
        execute_source_builds(&source_builds)?;
    }

    println!("\n\x1b[32m✓ Successfully applied changes.\x1b[0m");

    Ok(())
}

fn create_snapshot(file_path: &str) -> Result<(), String> {
    // Create snapshot directory
    fs::create_dir_all(SNAPSHOT_DIR)
        .map_err(|e| format!("Failed to create snapshot directory '{}': {}", SNAPSHOT_DIR, e))?;

    // Generate timestamp
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Failed to get current time: {}", e))?;
    let timestamp = now.as_secs();
    let datetime = chrono::DateTime::<chrono::Local>::from(chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp as i64, 0).unwrap());
    let timestamp_str = datetime.format("%Y-%m-%d-%H-%M-%S").to_string();

    let snapshot_path = format!("{}/{}.ast", SNAPSHOT_DIR, timestamp_str);

    // Copy the current .ast file to snapshot
    fs::copy(file_path, &snapshot_path)
        .map_err(|e| format!("Failed to create snapshot '{}': {}", snapshot_path, e))?;

    // Clean up old snapshots (keep only last 10)
    cleanup_old_snapshots()?;

    println!("\n\x1b[34m[SNAPSHOT] Created: {}\x1b[0m", snapshot_path);

    Ok(())
}

fn cleanup_old_snapshots() -> Result<(), String> {
    let mut snapshots: Vec<_> = fs::read_dir(SNAPSHOT_DIR)
        .map_err(|e| format!("Failed to read snapshot directory: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ast"))
        .collect();

    // Sort by modification time (newest first)
    snapshots.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        let b_time = b.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    // Remove old snapshots beyond MAX_SNAPSHOTS
    for snapshot in snapshots.iter().skip(MAX_SNAPSHOTS) {
        let _ = fs::remove_file(snapshot.path());
    }

    Ok(())
}

fn rollback_file(file_path: &str, dry_run: bool) -> Result<(), String> {
    // Find the most recent snapshot
    let mut snapshots: Vec<_> = fs::read_dir(SNAPSHOT_DIR)
        .map_err(|e| format!("Failed to read snapshot directory '{}': {}", SNAPSHOT_DIR, e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ast"))
        .collect();

    if snapshots.is_empty() {
        return Err("No snapshots found in /var/lib/astralos/snapshots/".to_string());
    }

    // Sort by modification time (newest first)
    snapshots.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        let b_time = b.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
        b_time.cmp(&a_time)
    });

    let latest_snapshot = &snapshots[0];
    let snapshot_path = latest_snapshot.path();

    println!("\x1b[34m[ROLLBACK] Using snapshot: {}\x1b[0m", snapshot_path.display());

    if dry_run {
        println!("\n\x1b[33m[DRY RUN] The following actions would be performed:\x1b[0m");
        println!("  cp {} {}", snapshot_path.display(), file_path);
        println!("  astctl apply {} --dry-run", file_path);
        println!("\n\x1b[33m[DRY RUN] No changes were made.\x1b[0m");
        return Ok(());
    }

    // Copy snapshot back to original file
    fs::copy(&snapshot_path, file_path)
        .map_err(|e| format!("Failed to restore snapshot: {}", e))?;

    println!("\x1b[32m✓ Restored {} from snapshot\x1b[0m", file_path);

    // Apply the restored file
    println!("\n\x1b[34m[ROLLBACK] Applying restored configuration...\x1b[0m");
    apply_file(file_path, false)?;

    Ok(())
}

fn clean_file(dry_run: bool) -> Result<(), String> {
    println!("\x1b[34m[CLEAN] Starting cleanup...\x1b[0m");

    // 1. Clean up old snapshots (keep only 5 most recent)
    let mut snapshots: Vec<_> = fs::read_dir(SNAPSHOT_DIR)
        .map_err(|e| format!("Failed to read snapshot directory '{}': {}", SNAPSHOT_DIR, e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("ast"))
        .collect();

    if snapshots.is_empty() {
        println!("  No snapshots found in {}", SNAPSHOT_DIR);
    } else {
        // Sort by modification time (newest first)
        snapshots.sort_by(|a, b| {
            let a_time = a.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let b_time = b.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            b_time.cmp(&a_time)
        });

        let total_snapshots = snapshots.len();
        let to_delete = if total_snapshots > CLEAN_MAX_SNAPSHOTS {
            total_snapshots - CLEAN_MAX_SNAPSHOTS
        } else {
            0
        };

        if to_delete > 0 {
            if dry_run {
                println!("\x1b[33m[DRY RUN] Would delete {} old snapshot(s):\x1b[0m", to_delete);
                for snapshot in snapshots.iter().skip(CLEAN_MAX_SNAPSHOTS) {
                    println!("  {}", snapshot.path().display());
                }
            } else {
                let mut deleted = 0;
                for snapshot in snapshots.iter().skip(CLEAN_MAX_SNAPSHOTS) {
                    if fs::remove_file(snapshot.path()).is_ok() {
                        deleted += 1;
                    }
                }
                println!("\x1b[32m✓ Deleted {} old snapshot(s), {} remaining\x1b[0m", deleted, CLEAN_MAX_SNAPSHOTS);
            }
        } else {
            println!("\x1b[32m✓ No old snapshots to delete ({} of {} max)\x1b[0m", total_snapshots, CLEAN_MAX_SNAPSHOTS);
        }
    }

    // 2. Clean up orphaned packages
    if dry_run {
        println!("\x1b[33m[DRY RUN] Would run: apt-get autoremove -y\x1b[0m");
    } else {
        println!("\x1b[34m[CLEAN] Running apt-get autoremove -y...\x1b[0m");
        let status = Command::new("apt-get")
            .args(["autoremove", "-y"])
            .status()
            .map_err(|e| format!("Failed to run apt-get autoremove: {}", e))?;

        if status.success() {
            println!("\x1b[32m✓ Package cleanup completed\x1b[0m");
        } else {
            eprintln!("\x1b[31mWarning: apt-get autoremove exited with non-zero status\x1b[0m");
        }
    }

    println!("\x1b[32m✓ Cleanup completed\x1b[0m");
    Ok(())
}

fn handle_system_block(blocks: &[AstBlock], _dry_run: bool) -> Result<Vec<Vec<String>>, String> {
    let mut commands = Vec::new();

    for block in blocks {
        if block.name.to_uppercase() == "SYSTEM" {
            // Handle hostname
            if let Some(AstValue::String(hostname)) = block.get("hostname") {
                // Try hostnamectl first, fallback to /etc/hostname
                if command_exists("hostnamectl") {
                    commands.push(vec!["hostnamectl".to_string(), "set-hostname".to_string(), hostname.clone()]);
                } else {
                    commands.push(vec!["sh".to_string(), "-c".to_string(), format!("echo {} > /etc/hostname", hostname)]);
                }
            }

            // Handle timezone
            if let Some(AstValue::String(timezone)) = block.get("timezone") {
                if command_exists("timedatectl") {
                    commands.push(vec!["timedatectl".to_string(), "set-timezone".to_string(), timezone.clone()]);
                }
            }
        }
    }

    Ok(commands)
}

fn handle_users_block(blocks: &[AstBlock], _dry_run: bool) -> Result<(Vec<Vec<String>>, Vec<String>), String> {
    let mut commands = Vec::new();
    let warnings = Vec::new();

    for block in blocks {
        if block.name.to_uppercase() == "USERS" {
            for (username, value) in &block.entries {
                if let AstValue::Block(user_block) = value {
                    // Create user if doesn't exist
                    commands.push(vec!["useradd".to_string(), "-m".to_string(), username.clone()]);

                    // Handle groups
                    if let Some(AstValue::Array(groups)) = user_block.get("groups") {
                        for group in groups {
                            commands.push(vec!["usermod".to_string(), "-aG".to_string(), group.clone(), username.clone()]);
                        }
                    }
                }
            }
        }
    }

    Ok((commands, warnings))
}

fn handle_services_block(blocks: &[AstBlock], _dry_run: bool) -> Result<(Vec<Vec<String>>, Vec<String>), String> {
    let mut commands = Vec::new();
    let mut warnings = Vec::new();

    // Discover current system services
    let system_services = discover_services()?;

    for block in blocks {
        if block.name.to_uppercase() == "SERVICES" {
            for (service_name, value) in &block.entries {
                // Check if service exists on system
                if !system_services.contains_key(service_name) {
                    warnings.push(format!("Service '{}' not found on system", service_name));
                    continue;
                }

                let is_running = system_services[service_name];

                match value {
                    AstValue::State(State::Enabled) => {
                        if !is_running {
                            // Enable and start service
                            commands.push(vec!["rc-update".to_string(), "add".to_string(), service_name.clone(), "default".to_string()]);
                            commands.push(vec!["service".to_string(), service_name.clone(), "start".to_string()]);
                        }
                    }
                    AstValue::State(State::Disabled) => {
                        if is_running {
                            // Stop and disable service
                            commands.push(vec!["service".to_string(), service_name.clone(), "stop".to_string()]);
                            commands.push(vec!["rc-update".to_string(), "del".to_string(), service_name.clone(), "default".to_string()]);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok((commands, warnings))
}

fn discover_services() -> Result<HashMap<String, bool>, String> {
    let mut services = HashMap::new();

    // Try rc-status first (OpenRC)
    if let Ok(output) = Command::new("rc-status").args(["--all", "--format", "{{name}} {{status}}"]).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let name = parts[0].to_string();
                    let status = parts[1].to_string();
                    services.insert(name, status == "started");
                }
            }
            return Ok(services);
        }
    }

    // Try service --status-all (SysV)
    if let Ok(output) = Command::new("service").args(["--status-all"]).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                // Format: [ + ]  service_name
                if line.contains("[ + ]") || line.contains("[ - ]") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        let name = parts[2].to_string();
                        let running = line.contains("[ + ]");
                        services.insert(name, running);
                    }
                }
            }
            return Ok(services);
        }
    }

    // Fallback: check /etc/rc*.d/ for symlinks
    if let Ok(entries) = fs::read_dir("/etc") {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("rc") && name.ends_with(".d") {
                        if let Ok(entries) = fs::read_dir(&path) {
                            for entry in entries.flatten() {
                                if let Some(svc_name) = entry.file_name().to_str() {
                                    // Symlinks like S01service or K01service
                                    if svc_name.starts_with('S') || svc_name.starts_with('K') {
                                        let service_name = &svc_name[3..]; // Skip S01 or K01
                                        services.insert(service_name.to_string(), svc_name.starts_with('S'));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(services)
}

fn handle_packages_block(
    blocks: &[AstBlock],
    to_install: &[String],
    to_remove: &[String],
    _dry_run: bool,
) -> Result<(Vec<Vec<String>>, Vec<SourceBuildInfo>), String> {
    let mut commands = Vec::new();
    let mut source_builds = Vec::new();

    // Extract package details (version, build, useflags) from the PACKAGES block
    let mut package_versions: HashMap<String, String> = HashMap::new();
    let mut package_builds: HashMap<String, String> = HashMap::new();
    let mut package_useflags: HashMap<String, Vec<String>> = HashMap::new();

    for block in blocks {
        if block.name.to_uppercase() == "PACKAGES" {
            for (key, value) in &block.entries {
                if let AstValue::Block(pkg_block) = value {
                    if let Some(AstValue::String(version)) = pkg_block.get("version") {
                        package_versions.insert(key.clone(), version.clone());
                    }
                    if let Some(AstValue::String(build)) = pkg_block.get("build") {
                        package_builds.insert(key.clone(), build.clone());
                    }
                    if let Some(AstValue::Array(flags)) = pkg_block.get("useflags") {
                        package_useflags.insert(key.clone(), flags.clone());
                    }
                }
            }
        }
    }

    // Handle installs
    for pkg in to_install {
        let build_type = package_builds.get(pkg).cloned().unwrap_or_default();
        let version = package_versions.get(pkg).cloned().unwrap_or_default();
        let useflags = package_useflags.get(pkg).cloned().unwrap_or_default();

        if build_type == "source" {
            // Source build - collect info for actual execution
            let useflags_for_display = useflags.clone();
            source_builds.push(SourceBuildInfo {
                package: pkg.clone(),
                version,
                useflags,
            });

            // For dry-run: show the sequence of commands that would be run
            let flags_str = if useflags_for_display.is_empty() {
                "none".to_string()
            } else {
                useflags_for_display.join(" ")
            };
            commands.push(vec!["#".to_string(), format!("=== SOURCE BUILD: {} ===", pkg)]);
            commands.push(vec!["#".to_string(), format!("Compiling {} from source with flags: {}. This may take a while...", pkg, flags_str)]);
            commands.push(vec!["apt-get".to_string(), "install".to_string(), "-y".to_string(), "build-essential".to_string(), "devscripts".to_string(), "dpkg-dev".to_string()]);
            commands.push(vec!["mkdir".to_string(), "-p".to_string(), "/tmp/astralos-build".to_string()]);
            commands.push(vec!["cd".to_string(), "/tmp/astralos-build".to_string(), "&&".to_string(), "apt-get".to_string(), "source".to_string(), pkg.clone()]);
            if !useflags_for_display.is_empty() {
                let flags_for_export = useflags_for_display.join(" ");
                commands.push(vec!["#".to_string(), format!("Injecting USE flags into debian/rules: {}", flags_for_export)]);
                commands.push(vec!["echo".to_string(), format!("export DEB_BUILD_MAINT_OPTIONS += {}", flags_for_export), ">>".to_string(), "debian/rules".to_string()]);
            }
            commands.push(vec!["dpkg-buildpackage".to_string(), "-us".to_string(), "-uc".to_string(), "-b".to_string()]);
            commands.push(vec!["dpkg".to_string(), "-i".to_string(), format!("../{}_*.deb", pkg)]);
        } else if version == "latest" {
            commands.push(vec![
                "apt-get".to_string(),
                "install".to_string(),
                "-t".to_string(),
                "daedalus-backports".to_string(),
                "-y".to_string(),
                pkg.clone(),
            ]);
        } else {
            commands.push(vec![
                "apt-get".to_string(),
                "install".to_string(),
                "-y".to_string(),
                pkg.clone(),
            ]);
        }
    }

    // Handle removals
    for pkg in to_remove {
        commands.push(vec![
            "apt-get".to_string(),
            "remove".to_string(),
            "-y".to_string(),
            pkg.clone(),
        ]);
    }

    Ok((commands, source_builds))
}

#[derive(Debug, Clone)]
struct SourceBuildInfo {
    package: String,
    version: String,
    useflags: Vec<String>,
}

fn execute_source_builds(source_builds: &[SourceBuildInfo]) -> Result<(), String> {
    for build in source_builds {
        let flags_str = if build.useflags.is_empty() {
            "none".to_string()
        } else {
            build.useflags.join(" ")
        };
        println!("\x1b[34m[BUILD] Compiling {} from source with flags: {}. This may take a while...\x1b[0m", build.package, flags_str);

        // Ensure build tools are installed
        let status = Command::new("apt-get")
            .args(["install", "-y", "build-essential", "devscripts", "dpkg-dev"])
            .status()
            .map_err(|e| format!("Failed to install build tools: {}", e))?;
        if !status.success() {
            return Err("Failed to install build tools".to_string());
        }

        // Create build directory
        let build_dir = format!("/tmp/astralos-build/{}", build.package);
        fs::create_dir_all(&build_dir)
            .map_err(|e| format!("Failed to create build directory: {}", e))?;

        // Download source
        println!("\x1b[34m[BUILD] Downloading source for {}...\x1b[0m", build.package);
        let status = Command::new("apt-get")
            .args(["source", &build.package])
            .current_dir(&build_dir)
            .status()
            .map_err(|e| format!("Failed to download source: {}", e))?;
        if !status.success() {
            return Err(format!("Failed to download source for {}", build.package));
        }

        // Find the source directory (apt-get source creates a directory like package-version)
        let source_dirs: Vec<_> = fs::read_dir(&build_dir)
            .map_err(|e| format!("Failed to read build directory: {}", e))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        if source_dirs.is_empty() {
            return Err(format!("No source directory found for {}", build.package));
        }

        let source_dir = &source_dirs[0].path();

        // Inject USE flags into debian/rules
        if !build.useflags.is_empty() {
            let flags_for_export = build.useflags.join(" ");
            let rules_path = source_dir.join("debian/rules");
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&rules_path)
                .map_err(|e| format!("Failed to open debian/rules: {}", e))?;
            use std::io::Write;
            writeln!(file, "export DEB_BUILD_MAINT_OPTIONS += {}", flags_for_export)
                .map_err(|e| format!("Failed to write to debian/rules: {}", e))?;
            println!("\x1b[34m[BUILD] Injected USE flags into debian/rules: {}\x1b[0m", flags_for_export);
        }

        // Build the package
        println!("\x1b[34m[BUILD] Building package...\x1b[0m");
        let status = Command::new("dpkg-buildpackage")
            .args(["-us", "-uc", "-b"])
            .current_dir(source_dir)
            .status()
            .map_err(|e| format!("Failed to run dpkg-buildpackage: {}", e))?;
        if !status.success() {
            return Err(format!("dpkg-buildpackage failed for {}", build.package));
        }

        // Install the resulting .deb files
        println!("\x1b[34m[BUILD] Installing built packages...\x1b[0m");
        let parent_dir = source_dir.parent().unwrap_or(Path::new(&build_dir));
        let deb_files: Vec<_> = fs::read_dir(parent_dir)
            .map_err(|e| format!("Failed to read parent directory: {}", e))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("deb"))
            .map(|e| e.path())
            .collect();

        if deb_files.is_empty() {
            return Err(format!("No .deb files found after building {}", build.package));
        }

        for deb in deb_files {
            let status = Command::new("dpkg")
                .args(["-i", deb.to_str().unwrap()])
                .status()
                .map_err(|e| format!("Failed to install .deb: {}", e))?;
            if !status.success() {
                return Err(format!("Failed to install {}", deb.display()));
            }
        }

        println!("\x1b[32m✓ Successfully built and installed {} from source\x1b[0m", build.package);
    }

    Ok(())
}

fn compute_diff_from_packages(packages: &HashMap<String, State>) -> Result<(Vec<String>, Vec<String>, Vec<String>, Vec<String>), String> {
    let mut to_install = Vec::new();
    let mut to_remove = Vec::new();
    let mut correct = Vec::new();
    let mut missing = Vec::new();

    for (name, declared_state) in packages {
        let system_state = check_package_installed(name);

        match (declared_state, system_state) {
            (State::Enabled, SystemState::Installed) => {
                correct.push(name.clone());
            }
            (State::Enabled, SystemState::NotInstalled) => {
                to_install.push(name.clone());
            }
            (State::Enabled, SystemState::Unknown) => {
                missing.push(name.clone());
            }
            (State::Disabled, SystemState::Installed) => {
                to_remove.push(name.clone());
            }
            (State::Disabled, SystemState::NotInstalled) => {
                correct.push(name.clone());
            }
            (State::Disabled, SystemState::Unknown) => {
                missing.push(name.clone());
            }
        }
    }

    Ok((to_install, to_remove, correct, missing))
}

fn sync_file(file_path: &str, dry_run: bool, quiet: bool) -> Result<(), String> {
    // Read and parse the file
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let mut blocks: Vec<AstBlock> = parse(&content)?;

    // Get packages already declared in the file
    let declared_packages = get_declared_package_names(&blocks)?;

    // Get manually installed packages from the system
    let manual_packages = get_manual_packages()?;

    // Find packages that are installed but not declared
    let mut new_packages = Vec::new();
    for pkg in manual_packages {
        if !declared_packages.contains_key(&pkg) {
            new_packages.push(pkg);
        }
    }

    if new_packages.is_empty() {
        if !quiet {
            println!("No new packages to sync. All manually installed packages are already in '{}'.", file_path);
        }
        return Ok(());
    }

    // Add new packages to the PACKAGES block
    let added_count = add_packages_to_block(&mut blocks, &new_packages)?;

    // Serialize the updated AST
    let new_content = serialize_blocks(&blocks);

    if dry_run {
        if !quiet {
            println!("\x1b[33m[DRY RUN] The following content would be written to '{}':\x1b[0m", file_path);
            println!();
            println!("{}", new_content);
            println!("\x1b[33m[DRY RUN] File was not modified.\x1b[0m");
        }
    } else {
        fs::write(file_path, new_content)
            .map_err(|e| format!("Failed to write file '{}': {}", file_path, e))?;
        if !quiet {
            println!("✓ Synced {} new package(s) to '{}':", added_count, file_path);
            for pkg in &new_packages {
                println!("  + {}", pkg);
            }
        }
    }

    Ok(())
}

fn get_declared_package_names(blocks: &[AstBlock]) -> Result<HashMap<String, bool>, String> {
    let mut packages = HashMap::new();

    for block in blocks {
        if block.name.to_uppercase() == "PACKAGES" {
            for (key, value) in &block.entries {
                match value {
                    AstValue::State(_) => {
                        packages.insert(key.clone(), true);
                    }
                    AstValue::Block(_block) => {
                        packages.insert(key.clone(), true);
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(packages)
}

fn get_manual_packages() -> Result<Vec<String>, String> {
    // Try apt-mark showmanual first (cleaner)
    let output = Command::new("apt-mark")
        .args(["showmanual"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let packages: Vec<String> = stdout
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            return Ok(packages);
        }
        _ => {
            // Fallback: try reading /var/log/apt/history.log
        }
    }

    Err("Could not determine manually installed packages".to_string())
}

fn add_packages_to_block(blocks: &mut Vec<AstBlock>, new_packages: &[String]) -> Result<usize, String> {
    for block in &mut *blocks {
        if block.name.to_uppercase() == "PACKAGES" {
            let mut added = 0;
            for pkg in new_packages {
                if !block.entries.contains_key(pkg) {
                    block.insert(pkg.clone(), AstValue::State(State::Enabled));
                    added += 1;
                }
            }
            return Ok(added);
        }
    }

    // If no PACKAGES block exists, create one
    let mut packages_block = AstBlock::new("PACKAGES");
    for pkg in new_packages {
        packages_block.insert(pkg.clone(), AstValue::State(State::Enabled));
    }
    blocks.push(packages_block);
    Ok(new_packages.len())
}

fn serialize_blocks(blocks: &[AstBlock]) -> String {
    let mut output = String::new();

    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        serialize_block(block, 0, &mut output);
    }

    output
}

fn serialize_block(block: &AstBlock, indent: usize, output: &mut String) {
    let indent_str = "  ".repeat(indent);
    output.push_str(&format!("{} {{\n", block.name));

    // Sort keys for consistent output
    let mut keys: Vec<&String> = block.entries.keys().collect();
    keys.sort();

    for key in keys {
        if let Some(value) = block.entries.get(key) {
            serialize_entry(key, value, indent + 1, output);
        }
    }

    output.push_str(&format!("{}}}\n", indent_str));
}

fn serialize_entry(key: &str, value: &AstValue, indent: usize, output: &mut String) {
    let indent_str = "  ".repeat(indent);

    match value {
        AstValue::String(s) => {
            output.push_str(&format!("{}{} = \"{}\"\n", indent_str, key, escape_string(s)));
        }
        AstValue::Boolean(b) => {
            output.push_str(&format!("{}{} = {}\n", indent_str, key, if *b { "true" } else { "false" }));
        }
        AstValue::State(s) => {
            output.push_str(&format!("{}{} = {}\n", indent_str, key, match s {
                State::Enabled => "enabled",
                State::Disabled => "disabled",
            }));
        }
        AstValue::Array(items) => {
            let items_str: Vec<String> = items.iter()
                .map(|s| format!("\"{}\"", escape_string(s)))
                .collect();
            output.push_str(&format!("{}{} = [{}]\n", indent_str, key, items_str.join(", ")));
        }
        AstValue::Block(block) => {
            serialize_block(block, indent, output);
        }
    }
}

fn extract_packages(blocks: &[AstBlock]) -> Result<HashMap<String, State>, String> {
    let mut packages = HashMap::new();

    for block in blocks {
        if block.name.to_uppercase() == "PACKAGES" {
            for (key, value) in &block.entries {
                match value {
                    AstValue::State(state) => {
                        packages.insert(key.clone(), *state);
                    }
                    AstValue::Block(block) => {
                        if let Some(AstValue::State(state)) = block.get("state") {
                            packages.insert(key.clone(), *state);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(packages)
}

fn check_package_installed(package_name: &str) -> SystemState {
    let output = Command::new("dpkg-query")
        .args(["-W", "-f=${Status}\n", package_name])
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !output.status.success() {
                if stderr.contains("no packages found") || stdout.trim().is_empty() {
                    SystemState::NotInstalled
                } else {
                    SystemState::Unknown
                }
            } else if stdout.contains("install ok installed") {
                SystemState::Installed
            } else {
                SystemState::NotInstalled
            }
        }
        Err(_) => SystemState::Unknown,
    }
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn print_diff_summary(
    to_install: &[String],
    to_remove: &[String],
    correct: &[String],
    missing: &[String],
) {
    if !to_install.is_empty() {
        println!("\x1b[32m[+] Packages to INSTALL ({}):\x1b[0m", to_install.len());
        for pkg in to_install {
            println!("  \x1b[32m{}\x1b[0m", pkg);
        }
    }

    if !to_remove.is_empty() {
        println!("\x1b[31m[-] Packages to REMOVE ({}):\x1b[0m", to_remove.len());
        for pkg in to_remove {
            println!("  \x1b[31m{}\x1b[0m", pkg);
        }
    }

    if !correct.is_empty() {
        println!("\x1b[34m[=] Packages CORRECT ({}):\x1b[0m", correct.len());
        for pkg in correct {
            println!("  \x1b[34m{}\x1b[0m", pkg);
        }
    }

    if !missing.is_empty() {
        println!("\x1b[33m[?] Packages UNKNOWN ({}):\x1b[0m", missing.len());
        for pkg in missing {
            println!("  \x1b[33m{}\x1b[0m", pkg);
        }
    }

    println!();
    println!(
        "Summary: {} to install, {} to remove, {} correct, {} unknown",
        to_install.len(),
        to_remove.len(),
        correct.len(),
        missing.len()
    );
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemState {
    Installed,
    NotInstalled,
    Unknown,
}