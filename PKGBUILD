if [[ -n ${startdir:-} && -n ${BUILDDIR:-} && "$BUILDDIR" -ef "$startdir" && -f "$startdir/src/main.rs" ]]; then
    printf '%s\n' "Error: run ./install-arch.sh from the repository root; makepkg's default srcdir would overlap the project's src/ directory." >&2
    exit 1
fi

pkgname=hyprly-git
pkgver=2.0.1.r1.g3b31fcc
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
    install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
