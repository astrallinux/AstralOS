use std::fs;
use std::process;

use ast_parser::{parse, AstBlock, AstValue, State};
use clap::{CommandFactory, Parser, Subcommand};

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
    }
}

fn validate_file(file_path: &str) -> Result<(), String> {
    // Read the file
    let content = fs::read_to_string(file_path)
        .map_err(|e| format!("Failed to read file '{}': {}", file_path, e))?;

    // Parse the content
    let blocks = parse(&content)?;

    // Print success message with parsed data
    println!("✓ Successfully validated '{}'", file_path);
    println!();
    print_blocks(&blocks, 0);

    Ok(())
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