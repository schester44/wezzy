#!/bin/sh
# "Set up <host>" (hosts.rs), run on the Mac in the background: send the host the
# Linux build of `wezzy` this app carries, over the ssh connection you already
# have (the host needs no internet), then run install-server.sh there. With no
# build for the host's system, install-server.sh downloads or builds one.
#
# usage: set-up-host.sh BUILDS INSTALLER DONE_FILE NAME PIN SSH_ARGS...
#   BUILDS     folder with wezzy-linux-<arch>.tar.gz ("" for none)
#   DONE_FILE  where to add NAME once it's set up
set -u

builds=$1 installer=$2 done_file=$3 name=$4 pin=$5
shift 5

say() { printf '==> %s\n' "$*"; }

# one ssh connection (and one login) for every step
ctl="$HOME/.local/share/wezzy/hosts/ssh-%C"
mkdir -p "$HOME/.local/share/wezzy/hosts"
remote() { ssh -o ControlMaster=auto -o ControlPath="$ctl" -o ControlPersist=30 "$@"; }

set_up() {
  os=$(remote "$@" uname -sm </dev/null) || return 1
  case $os in
    "Linux x86_64" | "Linux amd64") build=$builds/wezzy-linux-x86_64.tar.gz ;;
    "Linux aarch64" | "Linux arm64") build=$builds/wezzy-linux-aarch64.tar.gz ;;
    *) build= ;;
  esac
  if [ -n "$builds" ] && [ -n "$build" ] && [ -f "$build" ]; then
    say "sending wezzy for $os"
    # unpack beside it and rename over: a running server keeps its old file
    remote "$@" "set -e; d=\$(mktemp -d); tar -xzf - -C \"\$d\"; b=\"\$HOME/.local/share/wezzy/server/$pin/bin\"; mkdir -p \"\$b\"; mv -f \"\$d/wezzy\" \"\$b/wezzy\"; rm -rf \"\$d\"" <"$build" || return 1
  fi
  remote "$@" 'sh -s' <"$installer" || return 1
  printf '%s\n' "$name" >>"$done_file"
}

set_up "$@"
