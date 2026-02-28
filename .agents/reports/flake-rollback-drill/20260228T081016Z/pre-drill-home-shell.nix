# nx: shell configuration (zsh, starship, direnv, aliases)
{ pkgs, lib, config, configDir, ... }:

{
  # PATH configuration (prepended to Nix paths)
  # Order: selected user paths → Nix paths → appended user paths → Homebrew → system paths
  home.sessionPath = [
    "$HOME/.local/bin"
    "$HOME/.local/share/bun/bin"
    "$HOME/.nimble/bin"  # Nim packages (nimlangserver, etc.)
  ];

  # Zsh configuration
  programs.zsh = {
    enable = true;
    enableCompletion = true;
    autosuggestion.enable = true;
    syntaxHighlighting.enable = true;

    # History settings (XDG-compliant path)
    history = {
      size = 10000;
      save = 20000;
      path = "$HOME/.local/state/zsh/history";
      share = true;
      ignoreDups = true;
      ignoreAllDups = true;
      expireDuplicatesFirst = true;
    };

    # History options (additional setopt flags)
    historySubstringSearch.enable = true;

    # Shell aliases
    shellAliases = {
      # Navigation
      ".." = "cd ..";
      "..." = "cd ../..";
      "...." = "cd ../../..";

      # File listing (eza from nxs)
      ll = "ls -laF";
      la = "ls -A";
      l = "ls -CF";
      ls = "eza --icons=always";

      # Editor
      vim = "nvim";

      # zoxide
      j = "z";

      # Cheatsheet
      cheat = "glow -p ~/.cheatsheet.md";
      cheate = "nvim ~/.nix-config/configs/cheatsheet.md";

      # Network
      ip = "dig +short myip.opendns.com @resolver1.opendns.com";
      ips = "ifconfig -a | perl -nle'/(\d+\.\d+\.\d+\.\d+)/ && print $1'";
      flushdns = "sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder";
      scpresume = "rsync --partial --progress --rsh=ssh";
      tailscale = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

      # macOS
      showfiles = "defaults write com.apple.finder AppleShowAllFiles YES && killall Finder";
      hidefiles = "defaults write com.apple.finder AppleShowAllFiles NO && killall Finder";

      # SuperCollider
      sclang = "/Applications/SuperCollider.app/Contents/MacOS/sclang";
      SuperCollider = "/Applications/SuperCollider.app/Contents/MacOS/SuperCollider";

      # mpd (launchd service via home-manager)
      mpd-service-start = "launchctl load ~/Library/LaunchAgents/org.musicpd.mpd.plist";
      mpd-service-restart = "launchctl kickstart -k gui/$(id -u)/org.musicpd.mpd";
      mpd-service-stop = "launchctl unload ~/Library/LaunchAgents/org.musicpd.mpd.plist";
      mpd-check = "launchctl list | grep mpd";

      # vault-switcher (launchd service)
      vault-status = "~/.nix-config/scripts/vault-switcher --status";
      vault-switch = "~/.nix-config/scripts/vault-switcher";
      vault-unmount = "~/.nix-config/scripts/vault-switcher --unmount";
      vault-logs = "tail -50 ~/.local/state/vault-switcher/switcher.log";
      vault-service-check = "launchctl list | grep vault-switcher";
      vault-service-start = "launchctl load ~/Library/LaunchAgents/com.morgan.vault-switcher.plist";
      vault-service-restart = "launchctl kickstart -k gui/$(id -u)/com.morgan.vault-switcher";
      vault-service-stop = "launchctl unload ~/Library/LaunchAgents/com.morgan.vault-switcher.plist";

    };

    # Env extra (sourced in .zshenv — every zsh invocation, including non-interactive)
    envExtra = ''
      # ------------- Secrets (sops-nix) -------------
      # In .zshenv so non-interactive shells (Claude Code Bash tool, scripts,
      # background processes) also get secrets. Guards against double-sourcing
      # with HERALD_SECRETS_LOADED.
      if [[ -z "$HERALD_SECRETS_LOADED" && -f ${config.sops.templates."secrets-env".path} ]]; then
        set -a
        source ${config.sops.templates."secrets-env".path}
        set +a
        export HERALD_SECRETS_FILE="${config.sops.templates."secrets-env".path}"
        export HERALD_SECRETS_LOADED=1
      fi
      unset OPENAI_LOG HTTPX_LOG_LEVEL
    '';

    # Profile extra (sourced in .zprofile)
    profileExtra = ''
      # Keep cargo-installed tools available, but after Nix PATH entries so
      # flake-provided binaries (for example nx) win command resolution.
      export PATH="$PATH:$HOME/.local/share/cargo/bin"

      # Homebrew - append (not prepend) so Nix packages take precedence
      if [[ -x /opt/homebrew/bin/brew ]]; then
        export PATH="$PATH:/opt/homebrew/bin:/opt/homebrew/sbin"
        export MANPATH="/opt/homebrew/share/man''${MANPATH:+:$MANPATH}"
        export HOMEBREW_PREFIX="/opt/homebrew"
        export HOMEBREW_CELLAR="/opt/homebrew/Cellar"
        export HOMEBREW_REPOSITORY="/opt/homebrew"
        export HOMEBREW_NO_ENV_HINTS=1
      fi
    '';

    # Init content (sourced in .zshrc after other config)
    initContent = builtins.readFile (configDir + "/zsh/zshrc") + ''

      # ------------- Tailscale Funnel (Herald ingress) -------------
      # Ensures Herald is exposed at https://ishikawa.tail1869cf.ts.net
      # Required for Telegram webhook delivery. Config persists in Tailscale daemon;
      # this just re-applies if it ever gets cleared. Fast no-op when already active.
      if ! /Applications/Tailscale.app/Contents/MacOS/Tailscale funnel status 2>/dev/null \
           | grep -q "proxy http://127.0.0.1:4000"; then
        /Applications/Tailscale.app/Contents/MacOS/Tailscale funnel --bg 4000 &>/dev/null &
      fi
    '';
  };

  # Starship prompt
  programs.starship = {
    enable = true;
    enableZshIntegration = true;
  };

  # Use starship preset directly (Nix TOML parsing mangles Unicode glyphs)
  xdg.configFile."starship.toml".source = configDir + "/starship/starship.toml";

  # Cheatsheet (personal command reference)
  home.file.".cheatsheet.md".source = configDir + "/cheatsheet.md";

  # Wget config (XDG-compliant HSTS cache location)
  xdg.configFile."wget/wgetrc".source = configDir + "/wget/wgetrc";
}
