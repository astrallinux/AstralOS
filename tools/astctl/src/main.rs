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
    /// Validate an .ast configuration file
    Validate {
        /// Path to the .ast file to validate
        #[arg(value_name = "FILE")]
        file_path: String,
    },
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Validate { file_path } => {
            if let Err(e) = validate_file(&file_path) {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
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
    }
}

fn validate_file(file_path: &str) -> Result<(), String> {
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    let blocks = parse(&content)?;

    println!("✓ Successfully validated '{}'", file_path);
    println!();
    print_blocks(&blocks, 0);

    Ok(())
}

fn diff_file(file_path: &str) -> Result<(), String> {
    let (to_install, to_remove, correct, missing) = compute_diff(file_path)?;
    print_diff_summary(&to_install, &to_remove, &correct, &missing);
    Ok(())
}

fn apply_file(file_path: &str, dry_run: bool) -> Result<(), String> {
    let (to_install, to_remove, correct, missing) = compute_diff(file_path)?;

    if to_install.is_empty() && to_remove.is_empty() {
        println!("No changes needed. All packages are in the correct state.");
        return Ok(());
    }

    // Print the planned changes
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

    // Ask for confirmation
    print!("\nProceed with these changes? [y/N] ");
    io::stdout().flush().map_err(|e| format!("Failed to flush stdout: {}", e))?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| format!("Failed to read input: {}", e))?;

    let confirmed = input.trim().eq_ignore_ascii_case("y");

    if !confirmed {
        println!("Aborted.");
        return Ok(());
    }

    // Execute the changes
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

    println!("\n✓ Successfully applied changes.");

    Ok(())
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

fn print_blocks(blocks: &[AstBlock], indent: usize) {
    let indent_str = "  ".repeat(indent);

    for block in blocks {
        println!("{}{} {{", indent_str, block.name);
        print_entries(&block.entries, indent + 1);
        println!("{}}}", indent_str);
    }
}

fn print_entries(entries: &std::collections::HashMap<String, AstValue>, indent: usize) {
    let indent_str = "  ".repeat(indent);

    for (key, value) in entries {
        match value {
            AstValue::String(s) => {
                println!("{}{} = \"{}\"", indent_str, key, escape_string(s));
            }
            AstValue::Boolean(b) => {
                println!("{}{} = {}", indent_str, key, if *b { "true" } else { "false" });
            }
            AstValue::State(s) => {
                println!("{}{} = {}", indent_str, key, match s {
                    State::Enabled => "enabled",
                    State::Disabled => "disabled",
                });
            }
            AstValue::Array(items) => {
                let items_str: Vec<String> = items.iter()
                    .map(|s| format!("\"{}\"", escape_string(s)))
                    .collect();
                println!("{}{} = [{}]", indent_str, key, items_str.join(", "));
            }
            AstValue::Block(block) => {
                println!("{}{} {{", indent_str, key);
                print_entries(&block.entries, indent + 1);
                println!("{}}}", indent_str);
            }
        }
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}