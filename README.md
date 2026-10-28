# Rockhopper

A native launcher for Wayland, built with GPUI and [gpui-kit](https://github.com/longbridge/gpui-kit). Press a key, and a large search box appears over the focused output. Type, and your apps, open windows and system commands appear ranked as you go. Rockhopper learns what you open, so it rises in the results. It also keeps an encrypted clipboard history.

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
| `rockhopper clipboard` | Open on the clipboard history. Starts Rockhopper if it is not running. |
| `rockhopper emoji` | Open on the emoji picker. Starts Rockhopper if it is not running. |
| `rockhopper daemon` | Run without opening, for autostart. Exits quietly if Rockhopper is already running. |
| `rockhopper quit` | Stop Rockhopper. |

### niri

```kdl
spawn-at-startup "rockhopper" "daemon"

binds {
    Mod+Space { spawn "rockhopper"; }
    Mod+Shift+V { spawn "rockhopper" "clipboard"; }
    Mod+Period { spawn "rockhopper" "emoji"; }
}
```

Starting Rockhopper from the compositor gives it your session's `PATH` and `XDG_DATA_DIRS`, so it finds every app you can run.

## Using it

| Key | Action |
|---|---|
| type | Search apps, open windows and commands |
| `>` | Search only commands (see [Prefixes](#prefixes)) |
| `e` then space | Search emoji |
| `backspace` in an empty search | Leave the mode a prefix opened |
| `↓` `↑`, `ctrl-n` `ctrl-p`, `tab` `shift-tab` | Move the selection |
| `enter` | Open the app, switch to the window or run the command |
| `ctrl-k` | Show every action for the selected result |
| `esc` | Close the actions, clear the search, leave the clipboard history, or close |
| click outside | Close |

### Results

| Result | `enter` | More with `ctrl-k` |
|---|---|---|
| Application | Open it | Its desktop actions (such as New Private Window), Copy Command, Reset Ranking |
| Window | Switch to it (niri) | Open a new window of its app, Close Window, Copy Title, and every [window command](#commands) |
| Command | Run it | |
| Lock Screen, Suspend | Run it | |
| Log Out, Restart, Shut Down | Press `enter` twice: the first press asks for confirmation | |
| Clipboard History | Open the clipboard history | |
| Search Emoji | Open the emoji picker | |

Windows come from niri's IPC and show which app and workspace they belong to. On other compositors, no windows are listed.

Lock Screen runs `loginctl lock-session`, which your locker must listen for (for example through `swayidle` or `hypridle`). Log Out uses niri's quit action, or `loginctl terminate-session` elsewhere. Both can be replaced in the config.

### Prefixes

Typing a prefix at the start of the search narrows it to one kind of result. The prefix disappears from the search box and the mode's name shows beside it. Each mode lists everything it holds before you type, most used first. `esc` or `backspace` in an empty search goes back to the full search.

| Mode | Default prefix |
|---|---|
| `commands`: Lock Screen, Log Out and the rest, your own commands, and window commands | `>` |
| `emoji`: the emoji picker | `e ` (`e` then a space) |
| `apps` | off |
| `windows` | off |
| `clipboard`: the clipboard history | off |

A prefix that is a letter needs something after it, such as the space in `e `, so that searching for apps that start with `e` still works. Set prefixes under `prefixes` in the config. The longest matching prefix wins, so `>` and `>>` can both be used.

### Commands

Commands are command lines listed alongside everything else. `{window_id}` in a command is replaced by the id of the window that had focus when you opened the launcher. A command that uses it is only listed while some window has focus, which Rockhopper knows only on niri. The same commands appear under `ctrl-k` on any window result, acting on that window instead.

The built-in commands act on the focused window through `niri msg action`:

| Key | Name | Runs |
|---|---|---|
| `niri-fullscreen` | Toggle Fullscreen | `fullscreen-window` |
| `niri-windowed-fullscreen` | Toggle Windowed Fullscreen | `toggle-windowed-fullscreen` |
| `niri-maximize` | Maximize to Edges | `maximize-window-to-edges` |
| `niri-floating` | Toggle Floating | `toggle-window-floating` |
| `niri-center` | Center Window | `center-window` |
| `niri-screenshot` | Screenshot Window | `screenshot-window` |
| `niri-close` | Close Window | `close-window` |

Each runs with `--id {window_id}`. The launcher closes before a command runs, so commands that act on whatever has focus (such as `niri msg action maximize-column`) work too. The defaults live in [`src/defaults.json`](src/defaults.json), which both Rockhopper and the Home Manager module read.

Ranking is fuzzy, using the [nucleo](https://github.com/helix-editor/nucleo) matcher. Names that start with what you typed rank higher. Apps you open often or recently get a boost that fades over weeks. The history lives in `$XDG_STATE_HOME/rockhopper/history.json`.

## Apps

Rockhopper reads the desktop entries in `$XDG_DATA_HOME/applications` and every `$XDG_DATA_DIRS/*/applications`, following the [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/latest/). Entries that are hidden, marked `NoDisplay`, limited to other desktops by `OnlyShowIn`/`NotShowIn`, or whose `TryExec` is missing are skipped. Names, comments and keywords use your locale.

Each time the launcher opens, Rockhopper checks whether those directories changed, and rescans in the background if they did. Installing an app, or switching a Nix profile generation, shows up the next time you open it.

Apps start detached from Rockhopper. When `systemd-run` is available, each app runs in its own user scope named `app-rockhopper-<app id>-<random>.scope`, so stopping or restarting Rockhopper never closes apps you launched. Apps marked `Terminal=true` start through the `terminal` command from the config.

Icons come from your icon theme: the configured `icon_theme`, otherwise the GTK setting, then `hicolor`. Apps without an icon get a coloured initial.

## Emoji

**Search Emoji** in the launcher, or `rockhopper emoji`, opens a grid of every Unicode emoji. Type to search by name or GitHub shortcode (`tada` finds 🎉). The arrow keys move around the grid, and the selected emoji's name and shortcode show below it. `enter` copies the emoji; `ctrl-k` also offers Copy Shortcode (`:tada:`). Emoji you use often come first when the search is empty. Set `emoji.skin_tone` to use one skin tone for every emoji that has them.

Emoji are drawn from a colour emoji font's own images (Noto Color Emoji or JoyPixels, or the font set in `emoji.font`), so they look the same as in other apps. Emoji newer than that font are left out of the picker, since they would show as empty boxes. Without such a font, emoji are drawn as text.

## Clipboard history

Rockhopper watches the clipboard through the wlr data-control protocol (niri, Sway, Hyprland and KDE support it) and keeps the last 200 copies: text, and PNG images. Open it with **Clipboard History** in the launcher or with `rockhopper clipboard`. It lists your copies newest first, with a preview of the selected one; typing searches them. `enter` puts the selected entry back on the clipboard, ready to paste. `ctrl-k` offers Delete Entry and Delete All Entries; Delete All needs a second `enter`.

- **Secrets are skipped.** Copies that password managers mark as secret (the `x-kde-passwordManagerHint` type that KeePassXC and others set) are never recorded.
- **Encrypted at rest.** Each entry is stored in `$XDG_STATE_HOME/rockhopper/clipboard` encrypted with XChaCha20-Poly1305. The 256-bit key is random, created on first use, and kept only in your keyring (the freedesktop Secret Service: GNOME Keyring, KWallet, KeePassXC…) as **Rockhopper: clipboard history key**. Files are readable only by you.
- **Without a keyring, nothing is written.** If the keyring cannot be reached, the history stays in memory and is gone when Rockhopper stops.
- **A lost key does not silently delete history.** A `key.check` file records which key wrote the history. If the keyring hands back a different key (or a new one after the old was deleted), Rockhopper leaves the history on disk unread, keeps new copies in memory only, and says so on stderr. Restore the key, or delete `$XDG_STATE_HOME/rockhopper/clipboard` (or use Delete All Entries) to start over.
- **Delete All Entries removes everything on disk**, including history that has not been loaded yet because the keyring is still locked.
- Repeated copies move to the top instead of appearing twice. Text over 1 MiB and images over 32 MiB are not recorded.

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
launcher:
  focus_existing: true # on niri, Enter focuses an open window for the app
launch:
  systemd_scope: true
icon_theme: Papirus-Dark
system:
  lock: [loginctl, lock-session]     # replace the Lock Screen command
  logout: [niri, msg, action, quit]  # replace the Log Out command
emoji:
  skin_tone: default   # default, light, medium-light, medium, medium-dark or dark
  font: Noto Color Emoji   # a colour emoji font with PNG glyphs; default: the first installed
clipboard:           # read when Rockhopper starts
  enabled: true
  max_entries: 200
  images: true
prefixes:            # null turns one off
  commands: ">"
  emoji: "e "
  apps: null
  windows: "w "
  clipboard: null
commands:            # merged with the built-in ones by key
  niri-fullscreen:
    keywords: [big]  # override one field of a built-in command
  niri-close:
    enabled: false   # hide a built-in command
  half-width:        # add your own; name and run are required
    name: Half Width
    keywords: [resize]
    run: [niri, msg, action, set-window-width, --id, "{window_id}", "50%"]
```

On niri, Enter on an app focuses its most recently focused window by default. Use `shift-enter` to launch another instance, or select an existing window and choose `Open New <App>` from `ctrl-k`. Set `launcher.focus_existing: false` to always launch on Enter. On Sway and Hyprland, where Rockhopper does not list windows, Enter continues to launch the app.

On niri, Rockhopper opens on the focused output, which it asks niri for over `$NIRI_SOCKET`. On other compositors it opens on the primary output unless `appearance.output` is set.

## Installing with Nix

The flake provides:

| Output | What it is |
|---|---|
| `packages.<system>.default` (also `.rockhopper`) | Rockhopper, built with this flake's nixpkgs |
| `overlays.default` | Adds `pkgs.rockhopper`, built with *your* nixpkgs |
| `homeModules.default` | The Home Manager module (`homeManagerModules.default` is the same, under the older name) |
| `apps.<system>.default` | `nix run github:korora-tech/rockhopper` |

Rockhopper needs Rust 1.92 or newer, so a nixpkgs from nixos-26.05 or nixos-unstable. To try it without installing:

```sh
nix run github:korora-tech/rockhopper
```

### Home Manager

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rockhopper = {
      url = "github:korora-tech/rockhopper";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, home-manager, rockhopper, ... }: {
    homeConfigurations.you = home-manager.lib.homeManagerConfiguration {
      # The overlay builds Rockhopper with this nixpkgs; without it, the
      # module uses the flake's own package.
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ rockhopper.overlays.default ];
      };
      modules = [
        rockhopper.homeModules.default
        {
          programs.rockhopper = {
            enable = true;
            settings = {
              appearance.theme = "dark";
              terminal = [ "ghostty" "-e" ];
            };
          };
        }
      ];
    };
  };
}
```

Prefixes and commands have their own options, with the built-in ones as defaults:

```nix
programs.rockhopper = {
  prefixes = {
    emoji = ":";      # instead of "e "
    windows = "w ";
    commands = null;  # turn off
  };
  commands = {
    niri-fullscreen.keywords = [ "big" ];  # change one field of a built-in
    niri-close.enable = false;              # hide one
    half-width = {
      name = "Half Width";
      run = [ "niri" "msg" "action" "set-window-width" "--id" "{window_id}" "50%" ];
    };
  };
};
```

`settings` holds everything else and is written to `config.yaml` along with them, in the format described above; setting `prefixes` or `commands` inside `settings` is an error. `programs.rockhopper.package` picks the package; it defaults to `pkgs.rockhopper` when the overlay is applied. `programs.rockhopper.systemd.enable` runs `rockhopper daemon` as a user service bound to `graphical-session.target`. That service only sees the systemd user environment, so make sure `PATH` and `XDG_DATA_DIRS` are imported into it, or use your compositor's autostart instead.

## Development

```sh
nix develop -c cargo test
nix develop -c cargo run -- show
```

To drive the launcher in an automated test without touching the real keyboard, start the daemon with `ROCKHOPPER_TEST=1`. Then `rockhopper test-type <text>` replaces the search text, and `rockhopper test-key <key>` presses `down`, `up`, `left`, `right`, `tab`, `shift-tab`, `enter`, `shift-enter`, `escape` or `ctrl-k` in the open window. A daemon started without the variable refuses these. Test keys have no key release, so they can never confirm Log Out, Restart, Shut Down or Delete All.

GPUI reports layer-shell surfaces as client-decorated, so gpui-kit wraps them in its window frame. Rockhopper makes its overlay 21px larger than the output on each side and moves it back with negative margins, so that frame sits off-screen. See `FRAME_INSET` in `src/ui/mod.rs`.
