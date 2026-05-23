## systemd user unit for `wdotool prime`

`wdotool-prime.service` is a systemd user unit that keeps the
wlr-protocols `virtual_keyboard` and `virtual_pointer` devices alive
for the lifetime of the graphical session. Long-lived devices cut
per-call latency for scripts that issue many `wdotool` invocations in
a row, and they keep the seat's capability up across invocations so
the Layer 3 round-trip test suite (and any other tool that watches
the seat from a sibling client) can stay bound.

The unit only matters when the wlr-protocols backend is in use
(Sway, Hyprland, river, Wayfire). `wdotool prime` always picks
wlr-protocols regardless of any default backend selection, so on
compositors that don't speak the wlr protocols (KDE, GNOME) the
prime helper fails to bind virtual devices and the unit exits with
an error. If you only run a non-wlr compositor, don't enable the
unit.

### Manual install

```sh
mkdir -p ~/.config/systemd/user
cp wdotool-prime.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now wdotool-prime.service
```

`systemctl --user status wdotool-prime` should report `active
(running)` and the `wdotool prime` process should print `ready` once
the devices are up. Stop and start with
`systemctl --user stop wdotool-prime` and
`systemctl --user start wdotool-prime`; disable with
`systemctl --user disable --now wdotool-prime`.

### Distro install

Distros bundling wdotool should drop the unit under
`/usr/lib/systemd/user/wdotool-prime.service` so users can enable it
with one `systemctl --user enable` and pick up package upgrades
automatically.

Drift ships and enables this unit as part of the Drift Settings
Automation toggle.
