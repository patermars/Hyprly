#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
user_cache_root="${XDG_CACHE_HOME:-${HOME:?HOME must be set}/.cache}/hyprly"
cache_root="$user_cache_root/makepkg"
hyprland_cache_root="$user_cache_root/hyprland-makepkg"

HYPRLAND_PATCH_MARKER='/usr/share/hyprland/hyprland-pr16061.patch.sha256'
HYPRLAND_RECIPE_TMP=''

cleanup_temporary_recipe() {
    if [[ -n "$HYPRLAND_RECIPE_TMP" ]]; then
        rm -rf -- "$HYPRLAND_RECIPE_TMP"
    fi
}
trap cleanup_temporary_recipe EXIT

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

run_makepkg() {
    local pkgbuild_dir="$1" selected_cache_root="$2"
    local project_dir resolved_cache_root build_dir source_cache package_cache arg
    local install_after_build=1
    shift 2

    if [[ "${1:-}" == '--build-only' ]]; then
        install_after_build=0
        shift
    fi

    command -v makepkg >/dev/null 2>&1 || die 'makepkg is required; install base-devel first'
    command -v realpath >/dev/null 2>&1 || die 'realpath is required'
    for arg in "$@"; do
        case "$arg" in
            BUILDDIR=*|BUILDDIR+=*|SRCDEST=*|SRCDEST+=*|PKGDEST=*|PKGDEST+=*)
                die "makepkg path override '$arg' would put files outside the selected cache; these paths are controlled by install-arch.sh"
                ;;
        esac
    done
    [[ "$selected_cache_root" == /* ]] || die 'XDG_CACHE_HOME must be an absolute path'
    project_dir="$(realpath -e -- "$repo_root")"
    resolved_cache_root="$(realpath -m -- "$selected_cache_root")"
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

    cd -- "$pkgbuild_dir"
    if (( install_after_build )); then
        BUILDDIR="$build_dir" SRCDEST="$source_cache" PKGDEST="$package_cache" makepkg -si "$@"
    else
        BUILDDIR="$build_dir" SRCDEST="$source_cache" PKGDEST="$package_cache" makepkg -s "$@"
    fi
}

install_patched_hyprland() {
    local installed_line installed_version repo_info repo_version installed_pkgver repo_pkgver
    local upstream_version installed_pkgrel split_package installed_split_line installed_split_version installed_split_pkgver installed_split_pkgrel
    local patched_pkgrel patch_file patch_hash installed_patch_hash='' installed_debug_line=''
    local repo_pkgrel recipe_patch_file
    local recipe_dir recipe_cache_root project_dir command_name architecture package_name package_path package_version package_record
    local marker_contents actual_version
    local -a install_packages=() install_package_names=(hyprland hyprpm)

    for command_name in awk bsdtar grep install mktemp pacman realpath sed sha256sum sudo uname vercmp; do
        command -v "$command_name" >/dev/null 2>&1 || die "required command is missing: $command_name"
    done

    installed_line="$(pacman -Q hyprland 2>/dev/null)" || die 'Hyprland is not installed according to pacman'
    installed_version="${installed_line##* }"
    repo_info="$(pacman -Si hyprland 2>/dev/null)" || die 'Hyprland is not available in the currently configured/synced repositories'
    repo_version="$(awk '$1 == "Version" && $2 == ":" { print $3; exit }' <<<"$repo_info")"
    [[ -n "$repo_version" ]] || die 'could not read the repository Hyprland version'

    printf 'Installed Hyprland: %s\n' "$installed_version"
    printf 'Repository candidate: %s\n' "$repo_version"
    if (( $(vercmp "$installed_version" "$repo_version") < 0 )); then
        die "Hyprland is behind the repository candidate. Run 'sudo pacman -Syu', reboot into the updated Hyprland, then rerun this installer."
    fi

    installed_pkgver="${installed_version#*:}"
    repo_pkgver="${repo_version#*:}"
    upstream_version="${installed_pkgver%-*}"
    case "$upstream_version" in
        0.56.2|0.57.0) ;;
        *) die "no screen-sharing patch is approved for Hyprland $upstream_version; supported versions are 0.56.2 and 0.57.0" ;;
    esac
    patch_file="$repo_root/packaging/arch/hyprland-pr16061/hyprland-pr16061-v${upstream_version}.patch"
    [[ -s "$patch_file" ]] || die "the PR #16061 patch for Hyprland $upstream_version is missing: $patch_file"
    patch_hash="$(sha256sum -- "$patch_file" | awk '{ print $1 }')" || die 'could not calculate the Hyprland patch checksum'

    if [[ -f "$HYPRLAND_PATCH_MARKER" && ! -L "$HYPRLAND_PATCH_MARKER" ]]; then
        IFS= read -r installed_patch_hash < "$HYPRLAND_PATCH_MARKER" || true
        if [[ "$installed_patch_hash" == "$patch_hash" ]]; then
            printf 'The patched Hyprland package is already installed.\n'
            printf 'Log out and start a new Hyprland session to activate it if you have not already restarted.\n'
            return 0
        fi
    fi

    for command_name in grep install mktemp realpath sed; do
        command -v "$command_name" >/dev/null 2>&1 || die "required command is missing: $command_name"
    done

    installed_pkgrel="${installed_pkgver##*-}"
    repo_pkgrel="${repo_pkgver##*-}"
    for split_package in hyprpm hyprland-debug; do
        installed_split_line="$(pacman -Q "$split_package" 2>/dev/null || true)"
        [[ -n "$installed_split_line" ]] || continue

        installed_split_version="${installed_split_line##* }"
        installed_split_pkgver="${installed_split_version#*:}"
        installed_split_pkgrel="${installed_split_pkgver##*-}"
        if (( $(vercmp "$installed_pkgrel" "$installed_split_pkgrel") < 0 )); then
            installed_pkgrel="$installed_split_pkgrel"
        fi
    done
    if [[ "$installed_pkgrel" == *.* ]]; then
        local installed_pkgrel_base installed_pkgrel_subrelease
        installed_pkgrel_base="${installed_pkgrel%%.*}"
        installed_pkgrel_subrelease="${installed_pkgrel#*.}"
        [[ "$installed_pkgrel_base" =~ ^[0-9]+$ && "$installed_pkgrel_subrelease" =~ ^[0-9]+$ ]] || \
            die "cannot derive a safe patched package release from $installed_version"
        patched_pkgrel="${installed_pkgrel_base}.$((10#$installed_pkgrel_subrelease + 1))"
    else
        [[ "$installed_pkgrel" =~ ^[0-9]+$ ]] || die "cannot derive a safe patched package release from $installed_version"
        patched_pkgrel="${installed_pkgrel}.1"
    fi

    [[ "$hyprland_cache_root" == /* ]] || die 'XDG_CACHE_HOME must be an absolute path'
    project_dir="$(realpath -e -- "$repo_root")"
    recipe_cache_root="$(realpath -m -- "$hyprland_cache_root")" || die 'could not resolve the Hyprland package cache path'
    case "$recipe_cache_root" in
        "$project_dir"|"$project_dir/"*)
            die "the Hyprland package cache is inside the project checkout: $recipe_cache_root; set XDG_CACHE_HOME to a location outside the repository"
            ;;
    esac
    mkdir -p -- "$recipe_cache_root" || die "could not create the Hyprland package cache: $recipe_cache_root"
    [[ ! -L "$recipe_cache_root" ]] || die "Hyprland package cache path is a symlink: $recipe_cache_root"

    HYPRLAND_RECIPE_TMP="$(mktemp -d "$recipe_cache_root/.recipe.XXXXXX")" || die 'could not create a temporary Hyprland package recipe'
    recipe_dir="$HYPRLAND_RECIPE_TMP"
    install -m 0644 -- "$repo_root/packaging/arch/hyprland-pr16061/PKGBUILD" "$recipe_dir/PKGBUILD" || \
        die 'could not prepare the Hyprland package recipe'
    recipe_patch_file="$recipe_dir/hyprland-pr16061-v${upstream_version}.patch"
    install -m 0644 -- "$patch_file" "$recipe_patch_file" || \
        die 'could not prepare the Hyprland source patch'

    [[ "$repo_pkgrel" =~ ^[0-9]+$ ]] || die "cannot derive the repository package release from $repo_version"
    [[ "$(grep -Ec '^pkgver=' "$recipe_dir/PKGBUILD")" == 1 ]] || die 'the bundled Hyprland package recipe has an unexpected pkgver'
    [[ "$(grep -Ec '^pkgrel=' "$recipe_dir/PKGBUILD")" == 1 ]] || die 'the bundled Hyprland package recipe has an unexpected pkgrel'
    [[ "$(grep -Fxc -- "            'PATCH_SHA256_PLACEHOLDER')" "$recipe_dir/PKGBUILD")" == 1 ]] || \
        die 'the bundled Hyprland recipe is missing its patch checksum placeholder'
    sed -i "s/^pkgver=.*/pkgver=$upstream_version/; s/^pkgrel=.*/pkgrel=$repo_pkgrel.1/; s/^sha256sums=('[^']*'/sha256sums=('SKIP'/" "$recipe_dir/PKGBUILD" || \
        die 'could not adapt the Hyprland recipe to the installed release'
    sed -i "s/^pkgrel=$repo_pkgrel\\.1$/pkgrel=$patched_pkgrel/" "$recipe_dir/PKGBUILD" || \
        die 'could not set the patched Hyprland package release'
    sed -i "s/PATCH_SHA256_PLACEHOLDER/$patch_hash/" "$recipe_dir/PKGBUILD" || \
        die 'could not set the Hyprland patch checksum'

    printf 'Building patched Hyprland %s-%s as Arch packages.\n' "$upstream_version" "$patched_pkgrel"
    run_makepkg "$recipe_dir" "$recipe_cache_root" --build-only --cleanbuild --noconfirm

    architecture="$(uname -m)"
    package_version="$upstream_version-$patched_pkgrel"
    for package_name in hyprland hyprpm; do
        package_path="$recipe_cache_root/packages/$package_name-$package_version-$architecture.pkg.tar.zst"
        [[ -s "$package_path" ]] || die "makepkg did not create the expected package: $package_path"
        package_record="$(pacman -Qp "$package_path")" || die "could not inspect the built package: $package_path"
        [[ "$package_record" == "$package_name $package_version" ]] || \
            die "unexpected package metadata in $package_path: $package_record"
        install_packages+=("$package_path")
    done

    installed_debug_line="$(pacman -Q hyprland-debug 2>/dev/null || true)"
    if [[ -n "$installed_debug_line" ]]; then
        package_name='hyprland-debug'
        package_path="$recipe_cache_root/packages/$package_name-$package_version-$architecture.pkg.tar.zst"
        [[ -s "$package_path" ]] || die "makepkg did not create the installed debug package: $package_path"
        package_record="$(pacman -Qp "$package_path")" || die "could not inspect the built package: $package_path"
        [[ "$package_record" == "$package_name $package_version" ]] || \
            die "unexpected package metadata in $package_path: $package_record"
        install_packages+=("$package_path")
        install_package_names+=("$package_name")
    fi

    marker_contents="$(bsdtar -xOf "${install_packages[0]}" usr/share/hyprland/hyprland-pr16061.patch.sha256)" || \
        die 'the built Hyprland package is missing its PR #16061 patch marker'
    [[ "$marker_contents" == "$patch_hash" ]] || die 'the built Hyprland package marker does not match the patch file'

    printf '\nInstalling these package files with pacman:\n'
    printf '  %s\n' "${install_packages[@]}"
    sudo pacman -U "${install_packages[@]}"

    for package_name in "${install_package_names[@]}"; do
        installed_line="$(pacman -Q "$package_name" 2>/dev/null)" || die "$package_name was not installed"
        actual_version="${installed_line##* }"
        actual_version="${actual_version#*:}"
        [[ "$actual_version" == "$package_version" ]] || \
            die "$package_name installed as $actual_version instead of $package_version"
    done

    IFS= read -r installed_patch_hash < "$HYPRLAND_PATCH_MARKER" || true
    [[ "$installed_patch_hash" == "$patch_hash" ]] || die 'the installed Hyprland patch marker does not match the source patch'

    printf '\nInstalled patched Hyprland %s.\n' "$package_version"
    printf 'Log out and start a new Hyprland session to activate the screen-sharing change.\n'
}

install_hyprly() {
    run_makepkg "$repo_root" "$cache_root" "$@"
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

        if [[ -f "$config_file" ]] && grep -Fq -- "$marker" "$config_file"; then
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

    printf 'Reload the active Hyprland config; a new session is required if you just installed the patched package.\n'
}

main() {
    [[ -t 0 ]] || die 'this installer needs an interactive terminal for its confirmations'
    print_banner
    (( EUID != 0 )) || die 'run this installer as your regular user, not with sudo; makepkg will request sudo through pacman when needed'
    cat <<'EOF'
Optional Hyprland change:
PR #16061 was closed. It changes screen captures for
windows and layers using Hyprland's `no_screen_share` rule: the marked view is
skipped so the content behind it is captured instead of a black box. It applies
to tiled and floating views; it does not change which windows float. Mirrored
monitor captures keep the existing black-box fallback. Choosing Yes builds a
patched Hyprland package from the official source archive, checks the
patch marker, and installs the matching hyprland and hyprpm packages with
pacman. If hyprland-debug is installed, it updates that package too. The current
Hyprland session must be restarted before the change takes effect. The included
patch ports support Hyprland 0.56.2 and 0.57.0; choose No to leave Hyprland
unchanged on any other release.

Hyprly package installation:
This builds Hyprly and uses pacman to install the package and any missing
runtime or build dependencies. It writes system package files and changes the
pacman database; makepkg may ask for sudo access. Run this script as your
regular user, not with sudo. Hyprland and Hyprly build files, downloaded
sources, and package archives are kept under the user cache, outside this
project checkout. The Hyprly package is always built and installed after the
patch question; there is no separate confirmation for it.
EOF
    printf '\n'

    if ask_yes_no 'Apply the screen-sharing patch?'; then
        install_patched_hyprland
        add_screen_test_window_rules
    fi

    install_hyprly "$@"
}

main "$@"
