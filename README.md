# AstralOS

AstralOS is a minimal, security-focused, systemd-free Linux distribution based on Devuan. It is designed for developers, engineers, and gamers who require a stable, predictable environment without the overhead of modern init systems.

## Key Features
- **Systemd-Free:** Built on Devuan and powered by `sysvinit` for a traditional, transparent boot process.
- **Development Ready:** Ships with pre-configured C, Rust, and Python toolchains.
- **Gaming and NVIDIA:** Includes proprietary NVIDIA drivers and Steam out-of-the-box (glibc compatible).
- **Declarative Management:** Features `astctl`, a custom Rust-based tool for validating and applying system configuration via the `.ast` format.

## The `.ast` Configuration Format
AstralOS utilizes a custom, declarative configuration language. The `.ast` format allows users to define system state, installed packages, and security settings in a human-readable format.

```text
SYSTEM {
    name = "AstralOS"
    init = "sysvinit"
    development {
        rust = enabled
        python = enabled
    }
}
