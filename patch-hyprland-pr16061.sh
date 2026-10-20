#!/usr/bin/env bash
set -Eeuo pipefail

PR_COMMIT="eb4edf1cd20ae02cae8c83b819cd8e59100572c2"
PR_PATCH_URL="https://github.com/hyprwm/Hyprland/commit/${PR_COMMIT}.patch"
BACKUP_BASE="${XDG_STATE_HOME:-$HOME/.local/state}/hyprland-pr16061/backups"

die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: patch-hyprland-pr16061.sh /path/to/Hyprland-source

Checks the installed/repository Hyprland package versions, verifies the source
checkout matches the installed upstream version, backs up only files touched
by the pinned commit from PR #16061, and applies it as a line-based git patch.
The PR is closed and was not merged. This Arch/pacman-only helper does not
build, install, or activate Hyprland; the patched compositor must be built,
installed, and restarted separately. The PR keeps the black-box fallback for
mirrored monitors, where Hyprland cannot re-render the captured scene.

Run a full system update first (`sudo pacman -Syu`), then use a clean Hyprland
source checkout at the matching release tag. The script never runs pacman with
sudo and never updates the system itself.
EOF
}

[[ $# -eq 1 ]] || { usage; exit 2; }
SOURCE_DIR=$(realpath -e -- "$1") || die "source directory does not exist: $1"
[[ -d "$SOURCE_DIR/.git" || -f "$SOURCE_DIR/.git" ]] || die "not a Git checkout: $SOURCE_DIR"

for command_name in pacman vercmp git curl realpath awk; do
    command -v "$command_name" >/dev/null 2>&1 || die "required command is missing: $command_name"
done

INSTALLED_LINE=$(pacman -Q hyprland 2>/dev/null) || die "Hyprland is not installed according to pacman"
INSTALLED_VERSION=${INSTALLED_LINE##* }
REPO_INFO=$(pacman -Si hyprland 2>/dev/null) || die "Hyprland package is not available in the currently configured/synced repositories"
REPO_VERSION=$(awk '$1 == "Version" && $2 == ":" { print $3; exit }' <<<"$REPO_INFO")
[[ -n "$REPO_VERSION" ]] || die "could not read the repository Hyprland version"

printf 'Installed Hyprland: %s\n' "$INSTALLED_VERSION"
printf 'Repository candidate: %s\n' "$REPO_VERSION"
if (( $(vercmp "$INSTALLED_VERSION" "$REPO_VERSION") < 0 )); then
    die "Hyprland is behind the repository candidate. Run 'sudo pacman -Syu', reboot into the updated Hyprland, then rerun this script."
fi

PKGVER=${INSTALLED_VERSION#*:}
UPSTREAM_VERSION=${PKGVER%-*}
EXPECTED_TAG="v${UPSTREAM_VERSION}"
SOURCE_TAG=$(git -C "$SOURCE_DIR" describe --tags --exact-match HEAD 2>/dev/null || true)
[[ "$SOURCE_TAG" == "$EXPECTED_TAG" ]] || die "source HEAD must be the installed release tag $EXPECTED_TAG (found '${SOURCE_TAG:-an untagged commit}')"

if [[ -n $(git -C "$SOURCE_DIR" status --porcelain --untracked-files=normal) ]]; then
    die "source checkout has local changes or untracked files; commit or move them before patching"
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf -- "$TMP_DIR"' EXIT
PATCH_FILE="$TMP_DIR/pr-16061.patch"
curl --fail --location --silent --show-error "$PR_PATCH_URL" --output "$PATCH_FILE" || die "could not download the PR patch"
[[ -s "$PATCH_FILE" ]] || die "downloaded PR patch is empty"

git -C "$SOURCE_DIR" apply --check "$PATCH_FILE" || die "PR #16061 does not apply cleanly to $EXPECTED_TAG; no source files were changed"
mapfile -t PATCH_FILES < <(git -C "$SOURCE_DIR" apply --numstat "$PATCH_FILE" | awk -F '\t' 'NF == 3 { print $3 }')
((${#PATCH_FILES[@]} > 0)) || die "could not determine the files changed by the PR patch"

STAMP=$(date +%Y%m%d-%H%M%S)
BACKUP_DIR="$BACKUP_BASE/$STAMP"
for relative_path in "${PATCH_FILES[@]}"; do
    case "$relative_path" in
        /*|../*|*/../*|*/..)
            die "unsafe path in downloaded patch: $relative_path"
            ;;
    esac
    [[ -f "$SOURCE_DIR/$relative_path" ]] || die "patch target is not a regular source file: $relative_path"
done

mkdir -p -- "$BACKUP_DIR"
for relative_path in "${PATCH_FILES[@]}"; do
    mkdir -p -- "$BACKUP_DIR/$(dirname -- "$relative_path")"
    cp -p -- "$SOURCE_DIR/$relative_path" "$BACKUP_DIR/$relative_path"
done

if ! git -C "$SOURCE_DIR" apply "$PATCH_FILE"; then
    for relative_path in "${PATCH_FILES[@]}"; do
        cp -p -- "$BACKUP_DIR/$relative_path" "$SOURCE_DIR/$relative_path"
    done
    die "patch application failed; original target files were restored from $BACKUP_DIR"
fi

printf '\nApplied PR #16061 as a line-based patch.\n'
printf 'Backups of the %d modified source files: %s\n' "${#PATCH_FILES[@]}" "$BACKUP_DIR"
printf 'Review the changes with: git -C %q diff\n' "$SOURCE_DIR"
printf 'Build/package the source with your distribution packaging workflow; this script did not install anything.\n'
