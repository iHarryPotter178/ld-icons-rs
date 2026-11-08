#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"

package_name="ld-icons"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
architecture="$(dpkg --print-architecture)"
build_dir="$root_dir/target/deb"
package_dir="$build_dir/${package_name}_${version}_${architecture}"

if [[ -z "$version" ]]; then
    echo "unable to determine package version" >&2
    exit 1
fi

rm -rf "$package_dir"
mkdir -p \
    "$package_dir/DEBIAN" \
    "$package_dir/usr/bin" \
    "$package_dir/usr/share/doc/$package_name" \
    "$package_dir/usr/share/$package_name"

cargo build --release --locked
install -m 0755 target/release/ld-icons "$package_dir/usr/bin/ldicons"
install -m 0644 ldicons.conf "$package_dir/usr/share/$package_name/ldicons.conf"
install -m 0644 README.md "$package_dir/usr/share/doc/$package_name/README.md"
install -m 0644 LICENSE "$package_dir/usr/share/doc/$package_name/LICENSE"

cat >"$package_dir/DEBIAN/control" <<EOF
Package: $package_name
Version: $version
Section: utils
Priority: optional
Architecture: $architecture
Maintainer: ld-icons contributors
Description: Resource-friendly Wayland desktop icons
 A lightweight desktop-icons service for wlroots-based Wayland compositors.
Depends: libc6, libgcc-s1
EOF

dpkg-deb --root-owner-group --build "$package_dir" \
    "$build_dir/${package_name}_${version}_${architecture}.deb"
