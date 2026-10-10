# AUR packages

Two Arch packages live here, each mirrored to its own AUR git repository:

- `calstack/` — builds from the tagged source archive (`v$pkgver`).
- `calstack-bin/` — installs the prebuilt binary and desktop entry from the
  GitHub release archives.

## Publishing a new version

For each package, in its own checkout of the AUR repo:

```sh
# 1. Bump pkgver to the new tag (drop the leading v).
# 2. Fetch sources and fill the real checksums (replaces the 'SKIP' placeholders).
updpkgsums
# 3. Regenerate the metadata AUR reads.
makepkg --printsrcinfo > .SRCINFO
# 4. Verify it builds in a clean chroot, then publish.
makepkg -sri
git commit -am "upgpkg: <pkgname> <version>-<rel>"
git push
```

Initialize the AUR remotes once:

```sh
git clone ssh://aur@aur.archlinux.org/calstack.git
git clone ssh://aur@aur.archlinux.org/calstack-bin.git
```

The release archives (`calstack-<target>.tar.xz`) and the source tag tarball are
produced by the `Release` workflow when a `vX.Y.Z` tag is pushed.
