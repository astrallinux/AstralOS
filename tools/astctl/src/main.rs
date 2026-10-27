use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::process;
use std::process::Command;

use ast_parser::{parse, AstBlock, AstValue, State};
use clap::{Parser, Subcommand};

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
        Commands::Sync { file_path, dry_run } => {
            if let Err(e) = sync_file(&file_path, dry_run) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }
}

fn diff_file(file_path: &str) -> Result<(), String> {
    let (to_install, to_remove, correct, missing) = compute_diff(file_path)?;

    // If nothing to do, print short message
    if to_install.is_empty() && to_remove.is_empty() && missing.is_empty() {
        println!("\x1b[32m✓ System is up to date. No changes needed.\x1b[0m");
        return Ok(());
    }

    print_diff_summary(&to_install, &to_remove, &correct, &missing);
    Ok(())
}

fn apply_file(file_path: &str, dry_run: bool) -> Result<(), String> {
    let (to_install, to_remove, correct, missing) = compute_diff(file_path)?;

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

    // If nothing to do, print short message
    if to_install.is_empty() && to_remove.is_empty() {
        println!("\x1b[32m✓ System is up to date. No changes needed.\x1b[0m");
        return Ok(());
    }

    print_diff_summary(&to_install, &to_remove, &correct, &missing);

    if dry_run {
        println!("\n\x1b[33m[DRY RUN] The following commands would be executed:\x1b[0m");
        if !to_install.is_empty() {
            println!("  apt-get install -y {}", to_install.join(" "));
        }
        if !to_remove.is_empty() {
            println!("  apt-get remove -y {}", to_remove.join(" "));
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

    if !to_install.is_empty() {
        println!("\nInstalling packages: {}", to_install.join(", "));
        let status = Command::new("apt-get")
            .args(["install", "-y"])
            .args(&to_install)
            .status()
            .map_err(|e| format!("Failed to run apt-get install: {}", e))?;

        if !status.success() {
            return Err("apt-get install failed".to_string());
        }
    }

    if !to_remove.is_empty() {
        println!("\nRemoving packages: {}", to_remove.join(", "));
        let status = Command::new("apt-get")
            .args(["remove", "-y"])
            .args(&to_remove)
            .status()
            .map_err(|e| format!("Failed to run apt-get remove: {}", e))?;

        if !status.success() {
            return Err("apt-get remove failed".to_string());
        }
    }

    println!("\n\x1b[32m✓ Successfully applied changes.\x1b[0m");

    Ok(())
}

fn sync_file(file_path: &str, dry_run: bool) -> Result<(), String> {
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
        println!("No new packages to sync. All manually installed packages are already in '{}'.", file_path);
        return Ok(());
    }

    // Add new packages to the PACKAGES block
    let added_count = add_packages_to_block(&mut blocks, &new_packages)?;

    // Serialize the updated AST
    let new_content = serialize_blocks(&blocks);

    if dry_run {
        println!("\x1b[33m[DRY RUN] The following content would be written to '{}':\x1b[0m", file_path);
        println!();
        println!("{}", new_content);
        println!("\x1b[33m[DRY RUN] File was not modified.\x1b[0m");
    } else {
        fs::write(file_path, new_content)
            .map_err(|e| format!("Failed to write file '{}': {}", file_path, e))?;
        println!("✓ Synced {} new package(s) to '{}':", added_count, file_path);
        for pkg in &new_packages {
            println!("  + {}", pkg);
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
            // This is more complex, so we'll just return empty for now
            // In a real implementation, you'd parse the history log
        }
    }

    // If both fail, return empty (will be handled as Unknown in dry-run)
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

fn compute_diff(file_path: &str) -> Result<(Vec<String>, Vec<String>, Vec<String>, Vec<String>), String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let blocks = parse(&content)?;
    let packages = extract_packages(&blocks)?;

    if packages.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new(), Vec::new()));
    }

    let mut to_install = Vec::new();
    let mut to_remove = Vec::new();
    let mut correct = Vec::new();
    let mut missing = Vec::new();

    for (name, declared_state) in packages {
        let system_state = check_package_installed(&name);

        match (declared_state, system_state) {
            (State::Enabled, SystemState::Installed) => {
                correct.push(name);
            }
            (State::Enabled, SystemState::NotInstalled) => {
                to_install.push(name);
            }
            (State::Enabled, SystemState::Unknown) => {
                missing.push(name);
            }
            (State::Disabled, SystemState::Installed) => {
                to_remove.push(name);
            }
            (State::Disabled, SystemState::NotInstalled) => {
                correct.push(name);
            }
            (State::Disabled, SystemState::Unknown) => {
                missing.push(name);
            }
        }
    }

    Ok((to_install, to_remove, correct, missing))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemState {
    Installed,
    NotInstalled,
    Unknown,
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