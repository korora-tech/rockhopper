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
| `↓` `↑`, `ctrl-n` `ctrl-p`, `tab` `shift-tab` | Move the selection |
| `enter` | Open the app, switch to the window or run the command |
| `ctrl-k` | Show every action for the selected result |
| `esc` | Close the actions, clear the search, leave the clipboard history, or close |
| click outside | Close |

### Results

| Result | `enter` | More with `ctrl-k` |
|---|---|---|
| Application | Open it | Its desktop actions (such as New Private Window), Copy Command, Reset Ranking |
| Window | Switch to it (niri) | Open a new window of its app, Close Window, Copy Title |
| Lock Screen, Suspend | Run it | |
| Log Out, Restart, Shut Down | Press `enter` twice: the first press asks for confirmation | |
| Clipboard History | Open the clipboard history | |
| Search Emoji | Open the emoji picker | |

Windows come from niri's IPC and show which app and workspace they belong to. On other compositors, no windows are listed.

Lock Screen runs `loginctl lock-session`, which your locker must listen for (for example through `swayidle` or `hypridle`). Log Out uses niri's quit action, or `loginctl terminate-session` elsewhere. Both can be replaced in the config.

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

To drive the launcher in an automated test without touching the real keyboard, start the daemon with `ROCKHOPPER_TEST=1`. Then `rockhopper test-type <text>` replaces the search text, and `rockhopper test-key <key>` presses `down`, `up`, `left`, `right`, `tab`, `shift-tab`, `enter`, `escape` or `ctrl-k` in the open window. A daemon started without the variable refuses these. Test keys have no key release, so they can never confirm Log Out, Restart, Shut Down or Delete All.

GPUI reports layer-shell surfaces as client-decorated, so gpui-kit wraps them in its window frame. Rockhopper makes its overlay 21px larger than the output on each side and moves it back with negative margins, so that frame sits off-screen. See `FRAME_INSET` in `src/ui/mod.rs`.
