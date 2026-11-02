# AstralOS

AstralOS is an experimental, systemd-free Linux distribution based on Devuan. It bridges the gap between the speed of imperative package managers (like apt) and the reproducibility of declarative systems (like NixOS) through a custom Rust-based toolchain.

## Architecture
- Base System: Devuan GNU/Linux (Debian without systemd).
- Init System: sysvinit for standard, sequential booting.
- Package Manager: apt (with custom source-based USE flag support).
- Installer: Calamares graphical installer with Desktop Environment selection.
- Toolchain: Pre-configured with C, Rust, Python, and Git.

## The astctl Toolchain
AstralOS features a custom system management suite written in Rust:
- ast-parser: A custom AST (Abstract Syntax Tree) parser for the .ast configuration format.
- astctl: A CLI for diffing, applying, and syncing system states.
- Declarative-Imperative Hybrid: Normal apt usage is automatically logged into /etc/astralos/packages.ast via an APT hook. Custom compile-time optimizations (USE flags) are declared in the .ast file and built via dpkg-buildpackage.

## Core Principle: Fail-Safe Execution
astctl is designed with safety-critical logic in mind. If it cannot verify the state of the system (e.g., if dpkg-query is missing or fails), it aborts immediately rather than making blind assumptions. 

## Building from Source
To build the AstralOS ISO, you need a Debian/Devuan environment with live-build installed.

    sudo lb build

The resulting image will be generated as live-image-amd64.hybrid.iso.

## Roadmap
- [x] Implement custom .ast parser and astctl CLI in Rust.
- [x] Integrate live-build and custom branding (ASCII art, OS release).
- [x] Implement Calamares graphical installer and DE selection.
- [x] Implement USE flag source compilation pipeline.
- [x] Finalize automated APT hook for seamless packages.ast logging.

## License
MIT License
