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
    git describe --long --tags 2>/dev/null | sed 's/^v//;s/\([^-]*-g\)/r\1/;s/-/./g' || echo "0.1.0"
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
