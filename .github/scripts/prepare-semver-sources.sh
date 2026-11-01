#!/usr/bin/env bash
set -euo pipefail

: "${CARGO_HOME:?}" "${RUNNER_TEMP:?}" "${GITHUB_PATH:?}"

# cargo-semver-checks builds fresh consumer projects and ignores the package lockfile.
# Restrict those consumers to the same dependency sources as the release binary.
# https://doc.rust-lang.org/cargo/commands/cargo-vendor.html
semver_home="$RUNNER_TEMP/semver-cargo-home"
semver_bin="$RUNNER_TEMP/semver-bin"
mkdir -p "$semver_home" "$semver_bin"
cargo vendor --locked --versioned-dirs "$RUNNER_TEMP/semver-vendor" > "$semver_home/config.toml"

# release-plz/action installs the real checker in CARGO_HOME/bin. Scope the source
# replacement to that checker so release-plz can still find the published baseline
# on crates.io. Its installer and publisher retain their normal Cargo configuration.
cat > "$semver_bin/cargo-semver-checks" <<'CHECKER'
#!/usr/bin/env bash
set -euo pipefail
checker="${CARGO_HOME:?}/bin/cargo-semver-checks"
export CARGO_HOME="${RUNNER_TEMP:?}/semver-cargo-home"
exec "$checker" "$@"
CHECKER
chmod +x "$semver_bin/cargo-semver-checks"
printf '%s\n' "$semver_bin" >> "$GITHUB_PATH"
