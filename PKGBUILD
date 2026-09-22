pkgname=hyprly-git
pkgver=0.1.0
pkgrel=1
pkgdesc='AI overlay for Hyprland'
arch=('x86_64')
url='https://github.com/patermars/hyprly'
license=('MIT')
depends=('gtk4' 'libadwaita' 'gtk4-layer-shell' 'grim' 'slurp' 'tesseract' 'tesseract-data-eng' 'wl-clipboard')
makedepends=('rust' 'cargo' 'git')
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
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
    install -Dm644 assets/hyprly.desktop "$pkgdir/usr/share/applications/hyprly.desktop"
}
