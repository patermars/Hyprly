#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
cache_root="${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}/hyprly/makepkg"

PR_COMMIT="eb4edf1cd20ae02cae8c83b819cd8e59100572c2"
PR_PATCH_URL="https://github.com/hyprwm/Hyprland/commit/${PR_COMMIT}.patch"
BACKUP_BASE="${XDG_STATE_HOME:-${HOME:?HOME must be set}/.local/state}/hyprland-pr16061/backups"
PATCH_TMP_DIR=''

cleanup_patch_tmp() {
    if [[ -n "$PATCH_TMP_DIR" ]]; then
        rm -rf -- "$PATCH_TMP_DIR"
    fi
}
trap cleanup_patch_tmp EXIT

print_banner() {
    cat <<'EOF'
 _   _   __   __   ____    ____    _       __   __
| | | |  \ \ / /  |  _ \  |  _ \  | |      \ \ / /
| |_| |   \ V /   | |_) | | |_) | | |       \ V /
|  _  |    | |    |  __/  |  _ <  | |___     | |
|_| |_|    |_|    |_|     |_| \_\ |_____|    |_|
EOF
    printf '\nHYPRLY installer\n\n'
}
die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

ask_yes_no() {
    local prompt="$1" answer
    while true; do
        printf '%s [y/N] ' "$prompt"
        IFS= read -r answer || return 1
        case "${answer,,}" in
            y|yes) return 0 ;;
            ''|n|no) return 1 ;;
            *) printf 'Please answer y or n.\n' ;;
        esac
    done
}

read_source_path() {
    local source_path
    printf 'Path to the matching Hyprland source checkout: ' >&2
    IFS= read -r source_path || die 'could not read the Hyprland source path'
    [[ -n "$source_path" ]] || die 'a Hyprland source path is required when the patch is selected'
    case "$source_path" in
        '~') source_path="$HOME" ;;
        '~/'*) source_path="$HOME/${source_path:2}" ;;
        '~'*) die 'use an absolute path or a ~/ path for the Hyprland source checkout' ;;
    esac
    printf '%s\n' "$source_path"
}

patch_hyprland() {
    local source_dir installed_line installed_version repo_info repo_version
    local pkgver upstream_version expected_tag source_tag comparison
    local patch_file stamp backup_dir backup_root relative_path resolved_target command_name
    local original_head previous_patch_backup previous_patch_removed=0
    local apply_failed=0 restore_failed=0
    local -a patch_files=()

    restore_previous_patch() {
        local rel current_head

        (( previous_patch_removed )) || return 0
        current_head="$(git -C "$source_dir" rev-parse HEAD)" || return 1
        if [[ "$current_head" != "$original_head" ]]; then
            git -C "$source_dir" checkout --detach "$original_head" || return 1
        fi
        if ! git -C "$source_dir" apply "$patch_file"; then
            for rel in "${patch_files[@]}"; do
                cp -p -- "$previous_patch_backup/$rel" "$source_dir/$rel" || return 1
            done
        fi
        previous_patch_removed=0
    }

    fail_after_release_switch() {
        local message="$*"

        if (( previous_patch_removed )); then
            restore_previous_patch || die "$message; could not restore the previous patched checkout (backup: $previous_patch_backup)"
        fi
        die "$message"
    }

    command -v realpath >/dev/null 2>&1 || die 'realpath is required'
    source_dir="$(realpath -e -- "$1")" || die "Hyprland source directory does not exist: $1"
    [[ -d "$source_dir/.git" || -f "$source_dir/.git" ]] || die "not a Git checkout: $source_dir"
    [[ "$BACKUP_BASE" == /* ]] || die 'XDG_STATE_HOME must be an absolute path'
    backup_root="$(realpath -m -- "$BACKUP_BASE")" || die 'could not resolve the Hyprland backup directory'
    case "$backup_root" in
        "$source_dir"|"$source_dir/"*) die 'the Hyprland backup directory must be outside the source checkout' ;;
    esac

    for command_name in pacman vercmp git curl awk; do
        command -v "$command_name" >/dev/null 2>&1 || die "required command is missing: $command_name"
    done

    installed_line="$(pacman -Q hyprland 2>/dev/null)" || die 'Hyprland is not installed according to pacman'
    installed_version="${installed_line##* }"
    repo_info="$(pacman -Si hyprland 2>/dev/null)" || die 'Hyprland is not available in the currently configured/synced repositories'
    repo_version="$(awk '$1 == "Version" && $2 == ":" { print $3; exit }' <<<"$repo_info")"
    [[ -n "$repo_version" ]] || die 'could not read the repository Hyprland version'

    printf 'Installed Hyprland: %s\n' "$installed_version"
    printf 'Repository candidate: %s\n' "$repo_version"
    comparison="$(vercmp "$installed_version" "$repo_version")" || die 'could not compare installed and repository Hyprland versions'
    if (( comparison < 0 )); then
        die "Hyprland is behind the repository candidate. Run 'sudo pacman -Syu', reboot into the updated Hyprland, then rerun this installer."
    fi

    pkgver="${installed_version#*:}"
    upstream_version="${pkgver%-*}"
    expected_tag="v${upstream_version}"
    PATCH_TMP_DIR="$(mktemp -d)"
    patch_file="$PATCH_TMP_DIR/pr-16061.patch"
    curl --fail --location --silent --show-error "$PR_PATCH_URL" --output "$patch_file" || die 'could not download the pinned PR #16061 patch'
    [[ -s "$patch_file" ]] || die 'downloaded PR #16061 patch is empty'

    while IFS= read -r relative_path; do
        [[ -n "$relative_path" ]] && patch_files+=("$relative_path")
    done < <(git -C "$source_dir" apply --numstat "$patch_file" | awk -F '\t' 'NF == 3 { print $3 }')
    ((${#patch_files[@]} > 0)) || die 'could not determine the files changed by PR #16061'

    original_head="$(git -C "$source_dir" rev-parse HEAD)" || die 'could not read the Hyprland source checkout revision'
    source_tag="$(git -C "$source_dir" describe --tags --exact-match HEAD 2>/dev/null || true)"

    if [[ "$source_tag" == "$expected_tag" ]] && git -C "$source_dir" apply --reverse --check "$patch_file"; then
        printf 'PR #16061 is already applied to %s; no source files changed.\n' "$source_dir"
        return 0
    fi

    if [[ "$source_tag" != "$expected_tag" ]]; then
        if [[ -n $(git -C "$source_dir" status --porcelain --untracked-files=normal) ]]; then
            if git -C "$source_dir" apply --reverse --check "$patch_file"; then
                mkdir -p -- "$backup_root" || die 'could not create the Hyprland backup directory'
                stamp="$(date +%Y%m%d-%H%M%S)"
                previous_patch_backup="$(mktemp -d "$backup_root/${stamp}.pre-update.XXXXXX")" || die 'could not create a backup of the earlier patched source'
                for relative_path in "${patch_files[@]}"; do
                    case "$relative_path" in
                        /*|../*|*/../*|*/..|.) die "unsafe path in downloaded patch: $relative_path" ;;
                    esac
                    resolved_target="$(realpath -e -- "$source_dir/$relative_path")" || die "patch target does not exist in the old checkout: $relative_path"
                    case "$resolved_target" in
                        "$source_dir"/*) ;;
                        *) die "patch target resolves outside the source checkout: $relative_path" ;;
                    esac
                    [[ -f "$resolved_target" && ! -L "$source_dir/$relative_path" ]] || die "patch target is not a regular source file: $relative_path"
                    mkdir -p -- "$previous_patch_backup/$(dirname -- "$relative_path")"
                    cp -p -- "$source_dir/$relative_path" "$previous_patch_backup/$relative_path"
                done

                if ! git -C "$source_dir" apply --reverse "$patch_file"; then
                    for relative_path in "${patch_files[@]}"; do
                        cp -p -- "$previous_patch_backup/$relative_path" "$source_dir/$relative_path" || restore_failed=1
                    done
                    (( restore_failed == 0 )) || die "could not safely remove the earlier PR #16061 patch; backup: $previous_patch_backup"
                    die 'could not safely remove the earlier PR #16061 patch; the previous source was restored'
                fi
                if [[ -n $(git -C "$source_dir" status --porcelain --untracked-files=normal) ]]; then
                    if ! git -C "$source_dir" apply "$patch_file"; then
                        for relative_path in "${patch_files[@]}"; do
                            cp -p -- "$previous_patch_backup/$relative_path" "$source_dir/$relative_path" || restore_failed=1
                        done
                        (( restore_failed == 0 )) || die 'other local changes remain, and restoring the earlier patch was incomplete'
                    fi
                    die 'source checkout has changes beyond the earlier PR #16061 patch; those changes were preserved'
                fi
                previous_patch_removed=1
            else
                die 'source checkout has local changes or untracked files; commit or move them before updating it'
            fi
        fi

        printf 'Updating the clean Hyprland source checkout to installed release %s.\n' "$expected_tag"
        if ! git -C "$source_dir" fetch --no-tags https://github.com/hyprwm/Hyprland.git "refs/tags/$expected_tag:refs/tags/$expected_tag"; then
            restore_previous_patch || die 'could not fetch the matching release, and the previous source patch could not be restored'
            die "could not fetch Hyprland release $expected_tag"
        fi
        if ! git -C "$source_dir" checkout --detach "$expected_tag"; then
            restore_previous_patch || die 'could not switch releases, and the previous source patch could not be restored'
            die "could not switch the source checkout to Hyprland release $expected_tag"
        fi
        source_tag="$(git -C "$source_dir" describe --tags --exact-match HEAD 2>/dev/null || true)"
        if [[ "$source_tag" != "$expected_tag" ]]; then
            fail_after_release_switch "source checkout did not resolve to the expected release $expected_tag"
        fi
    fi

    if [[ -n $(git -C "$source_dir" status --porcelain --untracked-files=normal) ]]; then
        fail_after_release_switch 'source checkout has local changes or untracked files; commit or move them before patching'
    fi

    if git -C "$source_dir" apply --reverse --check "$patch_file"; then
        printf 'PR #16061 is already applied to %s; no source files changed.\n' "$source_dir"
        return 0
    fi

    for relative_path in "${patch_files[@]}"; do
        case "$relative_path" in
            /*|../*|*/../*|*/..|.)
                die "unsafe path in downloaded patch: $relative_path"
                ;;
        esac
        resolved_target="$(realpath -e -- "$source_dir/$relative_path")" || fail_after_release_switch "patch target does not exist in $expected_tag: $relative_path"
        case "$resolved_target" in
            "$source_dir"/*) ;;
            *) fail_after_release_switch "patch target resolves outside the source checkout: $relative_path" ;;
        esac
        [[ -f "$resolved_target" && ! -L "$source_dir/$relative_path" ]] || fail_after_release_switch "patch target is not a regular source file in $expected_tag: $relative_path"
    done

    stamp="$(date +%Y%m%d-%H%M%S)"
    mkdir -p -- "$backup_root" || fail_after_release_switch 'could not create the Hyprland backup directory'
    backup_dir="$(mktemp -d "$backup_root/${stamp}.XXXXXX")" || fail_after_release_switch 'could not create a backup of the current Hyprland source'
    for relative_path in "${patch_files[@]}"; do
        mkdir -p -- "$backup_dir/$(dirname -- "$relative_path")" || fail_after_release_switch "could not prepare the backup for $relative_path"
        cp -p -- "$source_dir/$relative_path" "$backup_dir/$relative_path" || fail_after_release_switch "could not back up $relative_path"
    done

    if git -C "$source_dir" apply --check "$patch_file"; then
        if git -C "$source_dir" apply "$patch_file"; then
            printf '\nApplied PR #16061 to %s.\n' "$expected_tag"
        else
            apply_failed=1
        fi
    else
        printf 'Patch context changed; trying a three-way apply against %s.\n' "$expected_tag"
        git -C "$source_dir" fetch --no-tags https://github.com/hyprwm/Hyprland.git refs/pull/16061/head >/dev/null 2>&1 || true
        if git -C "$source_dir" apply --3way "$patch_file"; then
            if git -C "$source_dir" reset --mixed HEAD; then
                printf '\nReapplied PR #16061 to %s with a three-way merge.\n' "$expected_tag"
            else
                apply_failed=1
                restore_failed=1
            fi
        else
            git -C "$source_dir" reset --mixed HEAD || restore_failed=1
            apply_failed=1
        fi
    fi

    if (( apply_failed )); then
        git -C "$source_dir" reset --mixed HEAD || restore_failed=1
        for relative_path in "${patch_files[@]}"; do
            cp -p -- "$backup_dir/$relative_path" "$source_dir/$relative_path" || restore_failed=1
        done
        restore_previous_patch || restore_failed=1
        (( restore_failed == 0 )) || die "patch application failed and automatic restore was incomplete; backups are at $backup_dir"
        die "PR #16061 no longer applies cleanly to $expected_tag; original target files were restored from $backup_dir. The patch needs review for this Hyprland release."
    fi

    printf 'Backups of the %d modified source files: %s\n' "${#patch_files[@]}" "$backup_dir"
    if [[ -n "$previous_patch_backup" ]]; then
        printf 'Backup of the earlier patched checkout: %s\n' "$previous_patch_backup"
    fi
    printf 'Review the changes with: git -C %q diff\n' "$source_dir"
    printf 'This edited only the source checkout. Build, install, and restart Hyprland separately to activate it.\n'
}

install_hyprly() {
    local project_dir resolved_cache_root build_dir source_cache package_cache arg

    command -v makepkg >/dev/null 2>&1 || die 'makepkg is required; install base-devel first'
    command -v realpath >/dev/null 2>&1 || die 'realpath is required'
    for arg in "$@"; do
        case "$arg" in
            BUILDDIR=*|BUILDDIR+=*|SRCDEST=*|SRCDEST+=*|PKGDEST=*|PKGDEST+=*)
                die "makepkg path override '$arg' would put files outside the selected cache; these paths are controlled by install-arch.sh"
                ;;
        esac
    done
    [[ "$cache_root" == /* ]] || die 'XDG_CACHE_HOME must be an absolute path'
    project_dir="$(realpath -e -- "$repo_root")"
    resolved_cache_root="$(realpath -m -- "$cache_root")"
    [[ "$resolved_cache_root" != / ]] || die 'the makepkg cache directory cannot be the filesystem root'
    case "$resolved_cache_root" in
        "$project_dir"|"$project_dir/"*)
            die "the configured cache is inside the project checkout: $resolved_cache_root; set XDG_CACHE_HOME to a location outside the repository"
            ;;
    esac

    build_dir="$resolved_cache_root/build"
    source_cache="$resolved_cache_root/sources"
    package_cache="$resolved_cache_root/packages"
    mkdir -p -- "$build_dir" "$source_cache" "$package_cache"

    printf 'Build files: %s\n' "$build_dir"
    printf 'Downloaded sources: %s\n' "$source_cache"
    printf 'Built packages: %s\n\n' "$package_cache"

    cd -- "$repo_root"
    BUILDDIR="$build_dir" SRCDEST="$source_cache" PKGDEST="$package_cache" makepkg -si "$@"
}

add_screen_test_window_rules() {
    local config_dir format config_file marker

    command -v realpath >/dev/null 2>&1 || die 'realpath is required'
    config_dir="${XDG_CONFIG_HOME:-${HOME:?HOME must be set}/.config}/hypr"
    [[ "$config_dir" == /* ]] || die 'XDG_CONFIG_HOME must be an absolute path'
    config_dir="$(realpath -m -- "$config_dir")" || die 'could not resolve the Hyprland config directory'

    mkdir -p -- "$config_dir" || die "could not create the Hyprland config directory: $config_dir"
    for format in conf lua; do
        if [[ "$format" == conf ]]; then
            config_file="$config_dir/hyprland.conf"
            marker='# >>> Hyprly Screen Test window rules >>>'
        else
            config_file="$config_dir/hyprland.lua"
            marker='-- >>> Hyprly Screen Test window rules >>>'
        fi
        [[ ! -L "$config_file" || -e "$config_file" ]] || die "Hyprland config path is a dangling symlink: $config_file"
        [[ ! -e "$config_file" || -f "$config_file" ]] || die "Hyprland config path is not a regular file: $config_file"
    done

    for format in conf lua; do
        if [[ "$format" == conf ]]; then
            config_file="$config_dir/hyprland.conf"
            marker='# >>> Hyprly Screen Test window rules >>>'
        else
            config_file="$config_dir/hyprland.lua"
            marker='-- >>> Hyprly Screen Test window rules >>>'
        fi

        if [[ -f "$config_file" ]] && grep -Fq "$marker" "$config_file"; then
            printf 'Hyprly Screen Test rules already exist in %s; skipped.\n' "$config_file"
            continue
        fi

        if [[ "$format" == conf ]]; then
            cat >> "$config_file" <<'EOF'

# >>> Hyprly Screen Test window rules >>>
windowrule = float on, match:class ^(io.github.patermars.HyprlyScreenTest)$
windowrule = no_screen_share on, match:class ^(io.github.patermars.HyprlyScreenTest)$
windowrule = match:class ^(io.github.patermars.HyprlyScreenTest)$, border_size 0
# <<< Hyprly Screen Test window rules <<<
EOF
        else
            cat >> "$config_file" <<'EOF'

-- >>> Hyprly Screen Test window rules >>>
hl.window_rule({
    name = "hyprly-screen-test",
    match = { class = "^io.github.patermars.HyprlyScreenTest$" },
    float = true,
    no_screen_share = true,
})
-- <<< Hyprly Screen Test window rules <<<
EOF
        fi

        printf 'Added Hyprly Screen Test rules to %s.\n' "$config_file"
    done

    printf 'Reload Hyprland for the rules to take effect.\n'
}

main() {
    local patch_requested=0 install_requested=0 window_rules_requested=0
    local hyprland_source=''

    [[ -t 0 ]] || die 'this installer needs an interactive terminal for its confirmations'
    print_banner
    (( EUID != 0 )) || die 'run this installer as your regular user, not with sudo; makepkg will request sudo through pacman when needed'
    cat <<'EOF'
Optional Hyprland change:
PR #16061 is unmerged. It changes screen captures for windows and
layers using Hyprland's `no_screen_share` rule: the marked view is skipped so
the content behind it is captured instead of a black box. This applies to
tiled and floating views; it does not change which windows float. Mirrored
monitor captures keep the existing black-box fallback. Applying the patch only
edits a local Hyprland source checkout; it does not build, install, or restart
Hyprland. On a rerun after a Hyprland update, a clean checkout or one carrying
only the earlier patch is moved to the installed release and patched again. If
upstream changes prevent a safe merge, the installer reports the backup and
stops for review.

Hyprly Screen Test window:
Optionally add rules to your Hyprland config so this window floats and is
excluded from screen sharing. The installer appends the Hyprland syntax to
hyprland.conf and the Lua syntax to hyprland.lua in your Hypr config directory.
Reload Hyprland to apply the rules from whichever config is active.

Hyprly package installation:
This builds Hyprly and uses pacman to install the package and any missing
runtime or build dependencies. It writes system package files and changes the
pacman database; makepkg may ask for sudo access. Run this script as your
regular user, not with sudo. Build files, downloaded sources, and package
archives are kept under the user cache, outside this project checkout.
EOF
    printf '\n'

    if ask_yes_no 'Apply the optional Hyprland PR #16061 source patch?'; then
        patch_requested=1
        hyprland_source="$(read_source_path)"
    fi
    if ask_yes_no 'Configure the Hyprly Screen Test window to float and stay out of screen shares?'; then
        window_rules_requested=1
    fi
    if ask_yes_no 'Build and install the Hyprly system package?'; then
        install_requested=1
    fi

    if (( patch_requested == 0 && window_rules_requested == 0 && install_requested == 0 )); then
        printf '\nNo changes selected.\n'
        return 0
    fi

    if (( patch_requested )); then
        patch_hyprland "$hyprland_source"
    fi
    if (( window_rules_requested )); then
        add_screen_test_window_rules
    fi
    if (( install_requested )); then
        install_hyprly "$@"
    fi
}

main "$@"
