self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.missnix;

  defaultPkg =
    if pkgs ? missnix then pkgs.missnix else self.packages.${pkgs.stdenv.hostPlatform.system}.default;

  themeEnv =
    lib.optionalAttrs (cfg.defaultDarkMode != null) {
      MISSNIX_DEFAULT_DARK_MODE = lib.boolToString cfg.defaultDarkMode;
    }
    // lib.optionalAttrs (cfg.defaultAccentHex != null) {
      MISSNIX_DEFAULT_ACCENT_HEX = lib.toUpper cfg.defaultAccentHex;
    };

  escapeSystemdArg =
    arg:
    "\"${
      lib.replaceStrings
        [
          "\\"
          "\""
          "$"
          "`"
          "%"
        ]
        [
          "\\\\"
          "\\\""
          "\\$"
          "\\`"
          "%%"
        ]
        (toString arg)
    }\"";

  checkArgs = [
    (lib.getExe cfg.package)
  ]
  ++ lib.concatMap (dir: [
    "--scan"
    dir
  ]) cfg.musicDirs
  ++ [ "--check" ]
  ++ lib.optional cfg.timer.notify "--notify";
in
{
  options.programs.missnix = {
    enable = lib.mkEnableOption "MissNix local music release radar";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPkg;
      defaultText = lib.literalExpression "pkgs.missnix or inputs.missnix.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The MissNix package to install and use for scheduled headless checks.";
    };

    musicDirs = lib.mkOption {
      type = lib.types.listOf lib.types.nonEmptyStr;
      default = [ ];
      example = [ "/home/alice/Music" ];
      description = ''
        Local music library folders passed via `--scan <dir>` before scheduled
        headless release checks.
      '';
    };

    defaultDarkMode = lib.mkOption {
      type = lib.types.nullOr lib.types.bool;
      default = null;
      description = ''
        Optional initial dark/light mode default (`MISSNIX_DEFAULT_DARK_MODE`)
        used until changed in MissNix's Settings UI.
      '';
    };

    defaultAccentHex = lib.mkOption {
      type = lib.types.nullOr (lib.types.strMatching "#[0-9A-Fa-f]{6}");
      default = null;
      example = "#E9B143";
      description = ''
        Optional initial custom `#RRGGBB` Material 3 seed color
        (`MISSNIX_DEFAULT_ACCENT_HEX`, preset `7`) used until changed in
        MissNix's Settings UI.
      '';
    };

    timer = {
      enable = lib.mkEnableOption "scheduled headless `missnix --check` systemd user timer";

      onCalendar = lib.mkOption {
        type = lib.types.nonEmptyStr;
        default = "daily";
        description = "Systemd `OnCalendar` schedule for `missnix-check.timer`.";
      };

      persistent = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Catch up missed scheduled checks after the next login (`Persistent = true`).";
      };

      randomizedDelaySec = lib.mkOption {
        type = lib.types.nonEmptyStr;
        default = "20m";
        description = "Random jitter added to `missnix-check.timer`.";
      };

      notify = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Pass `--notify` so new drops trigger a desktop notification.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    home.sessionVariables = themeEnv;

    systemd.user.services.missnix-check =
      lib.mkIf (pkgs.stdenv.hostPlatform.isLinux && cfg.timer.enable)
        {
          Unit = {
            Description = "MissNix headless local music scan and new-release radar check";
            Documentation = "https://github.com/Irgendeinwer/MissNix";
            After = [ "network-online.target" ];
            Wants = [ "network-online.target" ];
          };
          Service = {
            Type = "oneshot";
            Environment = lib.mapAttrsToList (k: v: "${k}=${v}") themeEnv;
            ExecStart = lib.concatMapStringsSep " " escapeSystemdArg checkArgs;
            Nice = 10;
            IOSchedulingClass = "idle";
          };
        };

    systemd.user.timers.missnix-check =
      lib.mkIf (pkgs.stdenv.hostPlatform.isLinux && cfg.timer.enable)
        {
          Unit = {
            Description = "Run MissNix headless release check on schedule";
            Documentation = "https://github.com/Irgendeinwer/MissNix";
          };
          Timer = {
            OnCalendar = cfg.timer.onCalendar;
            Persistent = cfg.timer.persistent;
            RandomizedDelaySec = cfg.timer.randomizedDelaySec;
          };
          Install.WantedBy = [ "timers.target" ];
        };
  };
}
