#!/bin/bash
echo "deb [trusted=yes] https://raw.githubusercontent.com/astrallinux/AstralOS/gh-pages bookworm main" > /etc/apt/sources.list.d/astralos.list
apt update
apt install -y astctl
