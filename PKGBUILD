if [[ -n ${startdir:-} && -f "$startdir/src/main.rs" ]]; then
    project_dir=$(realpath -e -- "$startdir") || {
        printf '%s\n' "Error: could not resolve the project directory: $startdir" >&2
        exit 1
    }
    for path_var in BUILDDIR SRCDEST PKGDEST; do
        case "$path_var" in
            BUILDDIR) path_value=${BUILDDIR:-$startdir} ;;
            SRCDEST) path_value=${SRCDEST:-$startdir} ;;
            PKGDEST) path_value=${PKGDEST:-$startdir} ;;
        esac
        resolved_path=$(realpath -m -- "$path_value") || {
            printf '%s\n' "Error: could not resolve makepkg's $path_var directory: $path_value" >&2
            exit 1
        }
        case "$resolved_path" in
            "$project_dir"|"$project_dir/"*)
                printf '%s\n' "Error: makepkg's $path_var directory is inside the project checkout. Run ./install-arch.sh to use the user cache." >&2
                exit 1
                ;;
        esac
    done
fi

pkgname=hyprly-git
pkgver=3.1.0.r3.g96b07e3
pkgrel=1
pkgdesc='GTK4 AI meeting assistant with a mobile client'
arch=('x86_64')
options=('!lto')
url='https://github.com/patermars/hyprly'
license=('MIT')
provides=('hyprly')
conflicts=('hyprly')
depends=('gtk4' 'pipewire' 'wireplumber')
optdepends=('poppler: PDF upload support'
            'unzip: DOCX upload support')
makedepends=('rust' 'cargo' 'git' 'pkgconf')
source=("git+${url}.git")
sha256sums=('SKIP')

pkgver() {
    cd hyprly
    # `git describe` can return an empty result when the source snapshot has
    # no usable tag. makepkg rejects an empty pkgver, so always provide a
    # valid fallback.
    local version
    version=$(git describe --long --tags 2>/dev/null || true)
    if [[ -n "$version" ]]; then
        printf '%s\n' "$version" | sed 's/^v//;s/\([^-]*-g\)/r\1/;s/-/./g'
    else
        printf '0.1.0.r%s\n' "$(git rev-list --count HEAD)"
    fi
}

build() {
    cd hyprly
    cargo build --release
}

package() {
    cd hyprly
    install -Dm755 target/release/hyprly "$pkgdir/usr/bin/hyprly"
    install -Dm644 data/hyprly.desktop "$pkgdir/usr/share/applications/hyprly.desktop"
    install -Dm644 assets/logo.png "$pkgdir/usr/share/icons/hicolor/256x256/apps/logo.png"
    install -Dm644 assets/icons/*.svg -t "$pkgdir/usr/share/hyprly/icons"
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
    install -Dm644 assets/icons/LICENSE "$pkgdir/usr/share/licenses/$pkgname/LUCIDE-LICENSE"
}
