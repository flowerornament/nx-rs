{ src, nxVersion }:
{ config, lib, pkgs, ... }:
let
  defaultPackage = import ./package.nix {
    inherit pkgs src;
    version = nxVersion;
  };
  cfg = config.programs.nx;
  sopsCfg = cfg.sops;
  cleanCachesCfg = cfg.cleanCaches;
  sessionVariables =
    {
      NX_CODE_ROOTS = lib.concatStringsSep ":" cleanCachesCfg.codeRoots;
      NX_CLEAN_SCAN_DEPTH = toString cleanCachesCfg.scanDepth;
      NX_CLEAN_SKIP = lib.concatStringsSep "," cleanCachesCfg.skip;
    }
    // lib.optionalAttrs (cfg.repoRoot != null) {
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

    cleanCaches = {
      codeRoots = lib.mkOption {
        type = lib.types.listOf (
          lib.types.addCheck lib.types.str (value: value != "" && !lib.hasInfix ":" value)
        );
        default = [ "${config.home.homeDirectory}/code" ];
        example = [ "/Users/alice/code" "/Volumes/work/code" ];
        description = ''
          Code roots scanned by `nx clean-caches` for build artifacts. Exported
          as colon-separated `NX_CODE_ROOTS`; set to an empty list to disable
          code-root scanning.
        '';
      };

      scanDepth = lib.mkOption {
        type = lib.types.addCheck lib.types.int (value: value >= 0 && value <= 8);
        default = 3;
        example = 5;
        description = ''
          Maximum directory depth searched below each clean-caches code root,
          from 0 to 8.
          Exported as `NX_CLEAN_SCAN_DEPTH`.
        '';
      };

      skip = lib.mkOption {
        type = lib.types.listOf (
          lib.types.addCheck lib.types.str (value: value != "" && !lib.hasInfix "," value)
        );
        default = [ ];
        example = [ "huggingface" ];
        description = ''
          Cache names that `nx clean-caches` should skip. Exported as
          comma-separated `NX_CLEAN_SKIP`.
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
