# Rockhopper

A native launcher for Wayland, built with GPUI and [gpui-kit](https://github.com/longbridge/gpui-kit). Press a key, and a large search box appears over the focused output. Type, and your installed applications appear ranked as you go. Rockhopper learns which apps you open, so they rise in the results.

Rockhopper needs a compositor that supports wlr layer-shell, such as niri, Sway, Hyprland or KDE. It does not run on GNOME.

## Run

```sh
nix run                                  # build and open the launcher
nix develop -c cargo run --release       # or from the dev shell
```

Rockhopper runs as a small background process, so it opens immediately. The first `rockhopper` command starts that process; later ones talk to it over `$XDG_RUNTIME_DIR/rockhopper.sock`.

| Command | Action |
|---|---|
| `rockhopper` / `rockhopper toggle` | Open the launcher, or close it if it is open. Starts Rockhopper if it is not running. |
| `rockhopper show` | Open the launcher. Starts Rockhopper if it is not running. |
| `rockhopper hide` | Close the launcher. |
| `rockhopper daemon` | Run without opening, for autostart. Exits quietly if Rockhopper is already running. |
| `rockhopper quit` | Stop Rockhopper. |

### niri

```kdl
spawn-at-startup "rockhopper" "daemon"

binds {
    Mod+Space { spawn "rockhopper"; }
}
```

Starting Rockhopper from the compositor gives it your session's `PATH` and `XDG_DATA_DIRS`, so it finds every app you can run.

## Using it

| Key | Action |
|---|---|
| type | Search apps by name, generic name, keywords or program name |
| `↓` `↑`, `ctrl-n` `ctrl-p`, `ctrl-j` `ctrl-k`, `tab` `shift-tab` | Move the selection |
| `enter` | Open the selected app |
| `esc` | Clear the search, or close if it is empty |
| click outside | Close |

Ranking is fuzzy, using the [nucleo](https://github.com/helix-editor/nucleo) matcher. Names that start with what you typed rank higher. Apps you open often or recently get a boost that fades over weeks. The history lives in `$XDG_STATE_HOME/rockhopper/history.json`.

## Apps

Rockhopper reads the desktop entries in `$XDG_DATA_HOME/applications` and every `$XDG_DATA_DIRS/*/applications`, following the [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/). Entries that are hidden, marked `NoDisplay`, limited to other desktops by `OnlyShowIn`/`NotShowIn`, or whose `TryExec` is missing are skipped. Names, comments and keywords use your locale.

Each time the launcher opens, Rockhopper checks whether those directories changed, and rescans in the background if they did. Installing an app, or switching a Nix profile generation, shows up the next time you open it.

Apps start detached from Rockhopper. When `systemd-run` is available, each app runs in its own user scope named `app-rockhopper-<app id>-<random>.scope`, so stopping or restarting Rockhopper never closes apps you launched. Apps marked `Terminal=true` start through the `terminal` command from the config.

Icons come from your icon theme: the configured `icon_theme`, otherwise the GTK setting, then `hicolor`. Apps without an icon get a coloured initial.

## Configuration

`$XDG_CONFIG_HOME/rockhopper/config.yaml`. Every key is optional, and the file is read again each time the launcher opens. An unknown key or a wrong type is reported on stderr, and Rockhopper uses the defaults instead.

```yaml
appearance:
  theme: system      # system, light or dark
  width: 720         # panel width, logical pixels
  top: 0.2           # distance from the top, as a fraction of the screen height
  dim: true          # dim the rest of the screen while open
  output: DP-1       # always open on this output; default: the focused one (niri)
terminal: [xdg-terminal-exec]   # prefix for Terminal=true apps
launch:
  systemd_scope: true
icon_theme: Papirus-Dark
```

On niri, Rockhopper opens on the focused output, which it asks niri for over `$NIRI_SOCKET`. On other compositors it opens on the primary output unless `appearance.output` is set.

### Home Manager

```nix
{
  inputs.rockhopper.url = "github:korora-tech/rockhopper";

  # in your Home Manager configuration:
  imports = [ inputs.rockhopper.homeManagerModules.default ];

  programs.rockhopper = {
    enable = true;
    settings = {
      appearance.theme = "dark";
      terminal = [ "ghostty" "-e" ];
    };
  };
}
```

`settings` is written to `config.yaml`. `programs.rockhopper.systemd.enable` runs `rockhopper daemon` as a user service bound to `graphical-session.target`. That service only sees the systemd user environment, so make sure `PATH` and `XDG_DATA_DIRS` are imported into it, or use your compositor's autostart instead.

## Development

```sh
nix develop -c cargo test
nix develop -c cargo run -- show
```

GPUI reports layer-shell surfaces as client-decorated, so gpui-kit wraps them in its window frame. Rockhopper makes its overlay 21px larger than the output on each side and moves it back with negative margins, so that frame sits off-screen. See `FRAME_INSET` in `src/ui.rs`.
