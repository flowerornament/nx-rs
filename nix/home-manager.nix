{ src, nxVersion }:
{ config, lib, pkgs, ... }:
let
  defaultPackage = import ./package.nix {
    inherit pkgs src;
    version = nxVersion;
  };
  cfg = config.programs.nx;
  sopsCfg = cfg.sops;
  sessionVariables =
    lib.optionalAttrs (cfg.repoRoot != null) {
      NX_REPO_ROOT = cfg.repoRoot;
    }
    // lib.optionalAttrs (!cfg.autoRefresh) {
      NX_RS_AUTO_REFRESH = "0";
    }
    // lib.optionalAttrs (sopsCfg.bin != null) {
      NX_RS_SOPS_BIN = sopsCfg.bin;
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

    sops = {
      package = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = ''
          Optional `sops` package to install alongside `nx` for secret
          management workflows.
        '';
      };

      bin = lib.mkOption {
        type = lib.types.nullOr (lib.types.addCheck lib.types.str (value: value != ""));
        default = null;
        example = "${pkgs.sops}/bin/sops";
        description = ''
          Optional path exported as `NX_RS_SOPS_BIN` when `nx secret add`
          should use a specific `sops` binary.
        '';
      };
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.enable {
      home.packages = [ cfg.package ] ++ lib.optional (sopsCfg.package != null) sopsCfg.package;
      home.sessionVariables = sessionVariables;
    })
  ];
}
