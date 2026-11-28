#!/usr/bin/env bash
# Prints the Homebrew formula for a release.
# Usage: homebrew-formula.sh <version> <directory with the archives>
set -euo pipefail

version=$1
dir=$2
base="https://github.com/udondan/fritz/releases/download/fritz-v$version"

# Prints the url and sha256 lines of the archive for a target.
archive() {
  local file="fritz-$version-$1.tar.gz"
  local sum
  sum=$(sha256sum "$dir/$file" | cut -d' ' -f1)
  printf '      url "%s/%s"\n      sha256 "%s"\n' "$base" "$file" "$sum"
}

cat <<EOF
# typed: false
# frozen_string_literal: true

class Fritz < Formula
  desc "CLI and MCP server for FRITZ!Box routers (unofficial)"
  homepage "https://github.com/udondan/fritz"
  version "$version"
  license "Apache-2.0"

  on_macos do
    on_arm do
$(archive aarch64-apple-darwin)
    end
    on_intel do
$(archive x86_64-apple-darwin)
    end
  end

  on_linux do
    on_arm do
$(archive aarch64-unknown-linux-musl)
    end
    on_intel do
$(archive x86_64-unknown-linux-musl)
    end
  end

  def install
    bin.install "fritz"
    bash_completion.install "completions/fritz.bash" => "fritz"
    zsh_completion.install "completions/_fritz"
    fish_completion.install "completions/fritz.fish"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/fritz --version")
  end
end
EOF
