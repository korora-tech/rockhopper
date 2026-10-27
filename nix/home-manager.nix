# Home Manager module: `programs.rockhopper`.
#
# `settings`, `prefixes` and `commands` are written to
# $XDG_CONFIG_HOME/rockhopper/config.yaml. Rockhopper re-reads it each time it
# opens, so a switch applies without a restart. The default prefixes and
# commands come from src/defaults.json, which Rockhopper builds in as well.
self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.rockhopper;
  yaml = pkgs.formats.yaml { };
  defaults = lib.importJSON ../src/defaults.json;

  commandType = lib.types.submodule {
    options = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Whether to list this command.";
      };
      name = lib.mkOption {
        type = lib.types.str;
        description = "The name shown and searched in the launcher.";
      };
      keywords = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        description = "More words the command is found by.";
      };
      run = lib.mkOption {
        type = lib.types.nonEmptyListOf lib.types.str;
        description = ''
          The program and its arguments. `{window_id}` is replaced by the id
          of the window that had focus when the launcher opened, or of the
          window chosen with `ctrl-k`; a command that uses it is listed only
          when there is such a window (on niri).
        '';
      };
    };
  };

  settings = cfg.settings // {
    prefixes = cfg.prefixes;
    commands = lib.mapAttrs (_: c: {
      enabled = c.enable;
      inherit (c) name keywords run;
    }) cfg.commands;
  };
in
{
  options.programs.rockhopper = {
    enable = lib.mkEnableOption "Rockhopper, a GPUI launcher for Wayland";

    package = lib.mkOption {
      type = lib.types.package;
      # With the overlay applied, built with the configuration's own nixpkgs.
      default = pkgs.rockhopper or self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "pkgs.rockhopper or rockhopper.packages.\${system}.default";
      description = "The Rockhopper package.";
    };

    settings = lib.mkOption {
      inherit (yaml) type;
      default = { };
      example = lib.literalExpression ''
        {
          appearance = {
            theme = "dark";
            width = 760;
            top = 0.25;
            dim = true;
          };
          terminal = [ "ghostty" "-e" ];
          icon_theme = "Papirus-Dark";
        }
      '';
      description = ''
        Written to {file}`$XDG_CONFIG_HOME/rockhopper/config.yaml`. Unknown keys
        are rejected and Rockhopper falls back to its defaults, so check the
        daemon's log after changing them.
      '';
    };

    prefixes = lib.mapAttrs (
      scope: prefix:
      lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = prefix;
        description = ''
          Typed at the start of the search, narrows it to ${scope}. `null`
          turns it off. A prefix that is a letter needs something after it,
          such as a space, so searches that start with that letter still work.
        '';
      }
    ) defaults.prefixes;

    commands = lib.mkOption {
      type = lib.types.attrsOf commandType;
      default = { };
      example = lib.literalExpression ''
        {
          # Change one field of a built-in command.
          niri-fullscreen.keywords = [ "big" ];
          # Turn one off.
          niri-close.enable = false;
          # Add your own.
          half-width = {
            name = "Half Width";
            run = [ "niri" "msg" "action" "set-window-width" "--id" "{window_id}" "50%" ];
          };
          edit-config = {
            name = "Edit niri Config";
            run = [ "xdg-open" "''${config.xdg.configHome}/niri/config.kdl" ];
          };
        }
      '';
      description = ''
        Commands listed in the launcher, by key. The built-in ones act on
        the focused window through niri: ${lib.concatStringsSep ", " (lib.attrNames defaults.commands)}.
        Setting a field of one of them overrides just that field.
      '';
    };

    systemd.enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Run `rockhopper daemon` as a systemd user service bound to
        `graphical-session.target`. The service sees the systemd user
        environment, so `PATH` and `XDG_DATA_DIRS` must be imported into it
        for every app to be found; otherwise prefer the compositor's
        autostart, such as niri's `spawn-at-startup "rockhopper" "daemon"`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # The built-in commands, each field a default the user can override.
    programs.rockhopper.commands = lib.mapAttrs (_: lib.mapAttrs (_: lib.mkDefault)) defaults.commands;

    assertions = [
      {
        assertion = !(cfg.settings ? prefixes || cfg.settings ? commands);
        message = "programs.rockhopper: set prefixes and commands with programs.rockhopper.prefixes and programs.rockhopper.commands, not settings.";
      }
    ];

    home.packages = [ cfg.package ];

    xdg.configFile."rockhopper/config.yaml".source = yaml.generate "rockhopper-config.yaml" settings;

    systemd.user.services.rockhopper = lib.mkIf cfg.systemd.enable {
      Unit = {
        Description = "Rockhopper launcher";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = "${lib.getExe cfg.package} daemon";
        ExecStop = "${lib.getExe cfg.package} quit";
        Restart = "on-failure";
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
