# Containerized wdotool

Minimal Debian-slim image with the `wdotool` binary installed from a release `.deb`. The container has the binary; the events go to the host kernel or the host compositor, depending on what you mount in.

## Why a container at all

Two reasons people might want this:

1. **Driving the host's Wayland session from inside a container.** Useful in test rigs that already run automation in containers, or CI jobs that bring up a screen and want to script input without installing host packages.
2. **No system install.** Try wdotool without `sudo dpkg -i`. Pull the image, run it, throw it away.

If you just want wdotool on a Linux desktop, install it natively (the AUR / .deb / .rpm / shell installer paths in the main README are simpler).

## Build

```sh
podman build -t wdotool:latest -f packaging/container/Containerfile .
# or
docker build -t wdotool:latest -f packaging/container/Containerfile .
```

Pin a different release with the build arg:

```sh
podman build --build-arg WDOTOOL_VERSION=0.5.1 -t wdotool:0.5.1 \
    -f packaging/container/Containerfile .
```

The default version tracks whichever release was current when the Containerfile was last updated; bump the arg if you want a newer one without editing the file.

## Run: wlroots backend (Sway, Hyprland, river, Wayfire)

Pass the host's Wayland socket and runtime dir into the container:

```sh
podman run --rm \
    --userns=keep-id \
    -e WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
    -e XDG_RUNTIME_DIR=/tmp \
    -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY:/tmp/$WAYLAND_DISPLAY" \
    wdotool:latest --backend wlroots info
```

`--userns=keep-id` is Podman-specific. It remaps the container UID to the host UID so the Wayland socket permissions match. Docker users either run as root inside the container (default) or set `--user $(id -u):$(id -g)`; in either case the socket needs to be readable by the container's effective UID.

## Run: uinput backend (any compositor, including X11)

uinput synthesizes events at the kernel layer, so the compositor doesn't need to expose anything:

```sh
podman run --rm \
    --device /dev/uinput \
    wdotool:latest --backend uinput key ctrl+c
```

The host kernel's uinput device must be writable by the container. On most setups that means the host user is in the `uinput` (or `input`) group and `/dev/uinput` has group-write permissions. See the main README's uinput section for the udev rule.

## What does NOT work in a container

The libei, kde, and gnome backends all rely on `xdg-desktop-portal` running on a session bus the binary can reach. Forwarding the host's session bus into a container is doable but fiddly: bind-mount `$XDG_RUNTIME_DIR/bus` plus the right env vars, ensure the portal frontend is awake on the host, and hope the container's UID can reach it. I haven't shipped a working recipe yet because it's the kind of thing that breaks across distros and I'd rather not document something that mostly fails. If you have a working setup, file a PR.

The Flatpak build at `packaging/flatpak/` already has the portal-forwarding wired up for sandboxed installs. That's a more reliable path for the libei / kde / gnome cases than container forwarding.

## What's actually in the image

Just `wdotool` and its runtime deps:

- `libwayland-client0`: Wayland client library used by the wlroots backend
- `libxkbcommon0` + `xkb-data`: keymap handling for `wdotool type`
- `ca-certificates`: TLS trust roots (apt complains without them)

Build-time `curl` is purged after the `.deb` install. Image lands around 140MB (verified on a 2026-05-08 build).

## Caveats

- **No portal cache.** The wlroots backend doesn't need it, but if you ever wire up libei in a container, bind-mount `~/.local/state/wdotool/` to persist the portal token across runs.
- **x86_64 only.** The release `.deb` is x86_64. Building for arm64 means cross-compiling wdotool itself; I can add a multi-stage build that does that if anyone files a need.
- **Tracks releases, not main.** The Containerfile installs a tagged release. For bleeding edge, swap the `dpkg -i` step for `git clone` + `cargo build --release` and copy the binary in.
