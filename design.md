# AstralOS System Design

## Overview
AstralOS is designed to solve the "Arch User's Year-Old System" problem. Imperative package managers (apt, pacman) are fast but lead to untracked, unreproducible system states over time. Pure declarative systems (NixOS) are reproducible but force users to learn a new language and abandon standard Linux paradigms.

AstralOS provides a middle ground: a semi-declarative system that tracks state automatically while leaving the underlying Linux filesystem intact.

## 1. The .ast Configuration Language
The .ast (AstralOS Text) format is a custom configuration language. It is parsed by a Rust-based Abstract Syntax Tree (AST) parser. 

Example:

PACKAGES {
    # Standard binary package
    firefox = enabled
    
    # Source-based package with USE flags
    ffmpeg {
        state = enabled
        useflags = ["nvenc", "x265"]
    }
}

## 2. The astctl Toolchain
astctl is the engine. It performs three main actions:
1. diff: Compares the .ast file against dpkg-query to see what needs to be installed or removed.
2. apply: Executes apt-get install or apt-get remove to enforce the declared state.
3. sync: Scans /var/log/apt/history.log (or apt-mark showmanual) and rewrites the .ast file to include packages the user installed manually.

## 3. USE Flag Compilation (The Gentoo Bridge)
If a package in packages.ast contains useflags, astctl bypasses the binary repository. It automates the Gentoo-style build process:
1. apt-get source <package>
2. Modifies the debian/rules file with the requested flags.
3. dpkg-buildpackage -us -uc
4. Installs the custom .deb and pins it in APT to prevent accidental overwrites.

## 4. Fail-Safe Engineering
Because astctl executes with root privileges, safety is paramount. If dpkg-query fails or returns an unparseable state, astctl treats the package as UNKNOWN. It does not guess. If any package is UNKNOWN, the apply command aborts with an error before executing any apt-get commands. This prevents the tool from making destructive assumptions about system state.

## 5. The APT Hook
AstralOS uses an APT Post-Invoke hook:

DPkg::Post-Invoke { "/usr/local/bin/astctl sync /etc/astralos/packages.ast --quiet || true"; };

This ensures that whenever the user installs a package normally via apt, the .ast file is updated automatically in the background.
