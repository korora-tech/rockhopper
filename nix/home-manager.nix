# Home Manager module: `programs.rockhopper`.
#
# `settings` is written to $XDG_CONFIG_HOME/rockhopper/config.yaml. Rockhopper
# re-reads it each time it opens, so a switch applies without a restart.
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
in
{
  options.programs.rockhopper = {
    enable = lib.mkEnableOption "Rockhopper, a GPUI launcher for Wayland";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "rockhopper.packages.\${system}.default";
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
    home.packages = [ cfg.package ];

    xdg.configFile."rockhopper/config.yaml" = lib.mkIf (cfg.settings != { }) {
      source = yaml.generate "rockhopper-config.yaml" cfg.settings;
    };

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
