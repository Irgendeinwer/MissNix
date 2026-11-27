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
in
{
  options.programs.missnix = {
    enable = lib.mkEnableOption "MissNix local music release radar desktop application";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPkg;
      defaultText = lib.literalExpression "pkgs.missnix or inputs.missnix.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The MissNix package to install system-wide.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
