#!/usr/bin/env bash
# Builds rsync 3.5.1 linked only to macOS system libraries (libSystem, libiconv).
# Optional checksum and compression libraries are turned off so the binary does
# not depend on Homebrew. ACLs, extended attributes, and creation times stay on.
# Prints nothing on success. The binary is .build/vendor/rsync-3.5.1/bin/rsync.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="3.5.1"
SHA256="c55f9c9dc10fb8bec397b399a0fdded53cc9a2d8e30891bb0d63724d25c37bef"
URL="https://github.com/RsyncProject/rsync/releases/download/v${VERSION}/rsync-${VERSION}.tar.gz"
DEST="$ROOT/.build/vendor/rsync-${VERSION}"
BIN="$DEST/bin/rsync"

if [[ -x "$BIN" ]] && "$BIN" --version | head -1 | grep -q "version ${VERSION} "; then
    if ! otool -L "$BIN" | grep -q '/opt/homebrew\|/usr/local/opt/'; then
        exit 0
    fi
fi

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
tarball="$workdir/rsync.tar.gz"
echo "Downloading rsync ${VERSION}"
curl -fsSL -o "$tarball" "$URL"
echo "${SHA256}  ${tarball}" | shasum -a 256 -c -
tar -xzf "$tarball" -C "$workdir"
src="$workdir/rsync-${VERSION}"

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
unset PKG_CONFIG_PATH PKG_CONFIG_LIBDIR CPATH LIBRARY_PATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH
export CFLAGS="-mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}"
export LDFLAGS="-mmacosx-version-min=${MACOSX_DEPLOYMENT_TARGET}"

cd "$src"
./configure \
    --prefix="$DEST" \
    --disable-debug \
    --disable-md2man \
    --disable-openssl \
    --disable-xxhash \
    --disable-zstd \
    --disable-lz4 \
    --disable-idn \
    --disable-roll-simd \
    --disable-roll-asm \
    --disable-md5-asm \
    --with-included-popt \
    --with-included-zlib
make -j"$(sysctl -n hw.ncpu)"
make install
cp COPYING "$DEST/COPYING"
strip "$BIN"

if otool -L "$BIN" | grep -q '/opt/homebrew\|/usr/local/opt/'; then
    echo "bundled rsync still links to a package-manager library" >&2
    otool -L "$BIN" >&2
    exit 1
fi
caps="$("$BIN" --version)"
echo "$caps" | head -1
for feature in ACLs xattrs crtimes hardlinks; do
    if ! echo "$caps" | grep -q "$feature"; then
        echo "bundled rsync is missing ${feature}" >&2
        echo "$caps" >&2
        exit 1
    fi
done
