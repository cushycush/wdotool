# Flathub manifest

The manifest, .desktop file, and AppStream metainfo here are the source of truth for the Flatpak build. They live in our repo so they stay in sync with the rest of the project; submitting them to Flathub is a separate step the maintainer does manually.

## Why a sandboxed build at all

Flatpak ships well to Steam Deck, the immutable Fedora variants (Silverblue, Bazzite, Kinoite), GNOME OS, and any user who's standardized on Flathub for desktop apps. The DEB and RPM artifacts attached to GitHub releases cover everyone else.

The Flatpak build excludes the uinput backend at compile time. The sandbox cannot reach `/dev/uinput`, so including the backend would just produce a runtime error. The libei, wlroots, kde, and gnome backends all work inside the sandbox via the portal and Wayland sockets.

## How submission works

The manifest in this directory is *almost* ready to submit. Two things have to happen before you open the Flathub PR:

### 1. Generate `cargo-sources.json`

Flathub requires offline, reproducible builds. The standard way to make a Cargo project reproducible inside a Flatpak builder is to vendor every transitive dependency into a JSON manifest the builder fetches up front.

```sh
# from the repo root
curl -L -O https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json
```

That `cargo-sources.json` does NOT live in this repo (it's regenerated from `Cargo.lock` whenever deps change). It lives next to the manifest in the Flathub fork.

### 2. Pin the source archive

The `sources:` block currently has `PLACEHOLDER_FILL_AT_SUBMISSION_TIME` for the sha256. Before submitting:

```sh
tag=v0.2.0  # whatever you're submitting
url="https://github.com/cushycush/wdotool/archive/refs/tags/${tag}.tar.gz"
sha=$(curl -sL "$url" | sha256sum | cut -d' ' -f1)
echo "url: $url"
echo "sha256: $sha"
```

Patch the manifest with the real values.

## Local testing before submission

```sh
# from the repo root
flatpak install --user flathub org.freedesktop.Platform//24.08 org.freedesktop.Sdk//24.08 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08
flatpak-builder --user --install --force-clean build-dir \
  packaging/flatpak/io.github.cushycush.wdotool.yml
flatpak run io.github.cushycush.wdotool diag
```

For dev iteration without the URL+sha dance, swap the `archive` source for a local path:

```yaml
sources:
  - type: dir
    path: ../..
  - cargo-sources.json
```

(Keep that change local; it should not land in the Flathub PR.)

## Submitting to Flathub

1. Fork https://github.com/flathub/flathub.
2. Create a new branch with a single commit adding the three files (manifest, .desktop, metainfo) plus `cargo-sources.json` under a fresh app folder.
3. Open a PR following the checklist at https://docs.flathub.org/docs/for-app-authors/submission.
4. Reviewers will ask questions about finish-args, the metainfo, and the build process. Plan for two or three revision rounds. The whole cycle typically runs four to eight weeks.

Once it lands, releases stay in sync via Flathub's bot: when our GitHub releases ship a new tag, Flathub's builder picks it up automatically as long as the manifest's `url` field is updated to the new tag (which is the maintainer's only ongoing chore).
