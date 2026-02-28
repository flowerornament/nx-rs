{
  description = "Morgan's Nix Darwin configuration";

  inputs = {
    # -------------------------------------------------------------------------
    # Base Inputs
    # -------------------------------------------------------------------------
    # Package set and module framework
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    nix-darwin = {
      url = "github:LnL7/nix-darwin/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    home-manager = {
      url = "github:nix-community/home-manager/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Determinate Nix module (required for proper integration)
    determinate = {
      url = "https://flakehub.com/f/DeterminateSystems/determinate/3";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Security
    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # -------------------------------------------------------------------------
    # Editors Inputs
    # -------------------------------------------------------------------------
    neovim-nightly-overlay = {
      url = "github:nix-community/neovim-nightly-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # -------------------------------------------------------------------------
    # Agents Inputs
    # -------------------------------------------------------------------------
    # Beads Viewer is pinned to f25a827 (newer commits have go vendoring bug)
    beads-viewer = {
      url = "github:Dicklesworthstone/beads_viewer/f25a827";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # nx: Nix configuration management tool (Rust)
    nx-rs = {
      url = "github:flowerornament/nx-rs";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # sp: Storage planning and purchase decision CLI (Rust)
    storage-planner = {
      url = "git+ssh://git@github.com/flowerornament/storage-planner";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # torrent-getter: Music acquisition pipeline CLI (Go)
    torrent-getter = {
      url = "git+ssh://git@github.com/flowerornament/torrent-getter";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Third-party Rust skill packs (pinned in flake.lock, updated via nx upgrade)
    actionbook-rust-skills = {
      url = "github:actionbook/rust-skills";
      flake = false;
    };

    leonardomso-rust-skills = {
      url = "github:leonardomso/rust-skills";
      flake = false;
    };

    # GSD installer source (pinned in flake.lock, updated via nx upgrade)
    gsd = {
      url = "github:glittercowboy/get-shit-done";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, nix-darwin, home-manager, determinate, sops-nix, neovim-nightly-overlay, beads-viewer, ... }@inputs: {
    darwinConfigurations."Ishikawa" = nix-darwin.lib.darwinSystem {
      system = "aarch64-darwin";
      specialArgs = { inherit inputs; };
      modules = [
        # Determinate Nix integration (must come first)
        determinate.darwinModules.default
        # home-manager as nix-darwin module
        home-manager.darwinModules.home-manager
        # Host-specific configuration
        ./hosts/macbook-pro-m4.nix
      ];
    };
  };
}
