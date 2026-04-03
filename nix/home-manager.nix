{ src, nxVersion }:
{ config, lib, pkgs, ... }:
let
  defaultPackage = import ./package.nix {
    inherit pkgs src;
    version = nxVersion;
  };
  cfg = config.programs.nx;
  sessionVariables =
    lib.optionalAttrs (cfg.repoRoot != null) {
      NX_REPO_ROOT = cfg.repoRoot;
    }
    // lib.optionalAttrs (!cfg.autoRefresh) {
      NX_RS_AUTO_REFRESH = "0";
    };
in
{
  options.programs.nx = {
    enable = lib.mkEnableOption "nx Nix configuration helper";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "nx package from this flake";
      description = "The nx package to install.";
    };

    repoRoot = lib.mkOption {
      type = lib.types.nullOr (lib.types.addCheck lib.types.str (value: value != ""));
      default = null;
      example = "/Users/alice/code/nix-config";
      description = ''
        Optional repository root exported as `NX_REPO_ROOT` for nx commands
        that should target a managed config repo outside the current working
        directory.
      '';
    };

    autoRefresh = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether nx may auto-refresh a local cargo-installed binary before
        rebuild and upgrade flows. When disabled, the module exports
        `NX_RS_AUTO_REFRESH=0`.
      '';
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      home.packages = [ cfg.package ];
      home.sessionVariables = sessionVariables;
    })
  ];
}
