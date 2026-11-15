# AUR packages

Both PKGBUILDs are validated with `makepkg` on a clean build.

## Upload (manual, requires AUR account and SSH key)

```bash
git clone ssh://aur@aur.archlinux.org/ffind-rs.git
cp packaging/aur/ffind-rs/PKGBUILD ffind-rs/
cp packaging/aur/ffind-rs/.SRCINFO ffind-rs/  # generate first with makepkg --printsrcinfo > .SRCINFO
cd ffind-rs && git add -A && git commit -m "Initial import: ffind-rs 0.2.0" && git push
```

Repeat for `ffind-rs-bin`. The `-bin` package points at the GitHub Release
tarball; bump its `pkgver`/`sha256sums` on every release (the release workflow
tarballs are deterministic, the GitHub auto-generated source archives are not,
which is why `ffind-rs` uses a git source with `SKIP`).

## Version bump checklist on a new release

1. Bump `pkgver` and reset `pkgrel=1` in both PKGBUILDs.
2. `ffind-rs`: nothing else, git source tracks the tag.
3. `ffind-rs-bin`: update `sha256sums` from the release asset
   `ffind-v<ver>-x86_64-linux-musl.tar.gz.sha256`.
4. Regenerate `.SRCINFO` for each: `makepkg --printsrcinfo > .SRCINFO`.
