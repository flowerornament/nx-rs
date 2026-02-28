# nx: CLI tools and utilities (ripgrep, fd, bat, jq, htop, curl, wget)
{ pkgs, inputs, ... }:

{
  # CLI packages migrated from Homebrew to nixpkgs
  home.packages = with pkgs; [
    # === Batch 1: Core CLI tools ===

    # File/text utilities
    bat           # cat replacement with syntax highlighting
    bindfs        # FUSE filesystem for mounting a directory
    eza           # ls replacement with icons
    fd            # find replacement
    fswatch       # Cross-platform file change monitor
    fzf           # fuzzy finder
    glow          # markdown viewer in terminal
    jq            # JSON processor
    ripgrep       # grep replacement (provides 'rg')
    tree          # Directory tree viewer

    # Navigation
    zoxide        # smart cd (learns your habits)
    yazi          # terminal file manager

    # Dev tools
    lazygit       # git TUI
    home-manager  # Home Manager CLI
    sops          # secrets management

    # Download/media
    rclone        # Command line program to sync files and directories
    wget          # HTTP/FTP downloader

    # Web/search CLI
    lynx          # Text-mode web browser (HTML to text)

    # Fonts
    nerd-fonts.hack  # Fixes starship prompt glyph rendering

    # === Batch 2: Specialized tools ===
    # (Languages/runtimes moved to languages.nix)

    # Terminals/shells
    nushell       # Modern shell with structured data (nu)
    zellij        # Terminal multiplexer (tmux alternative)
    mosh          # Mobile shell (persistent SSH)

    # Document tools
    pandoc        # Universal document converter
    tectonic      # Modern LaTeX engine
    ghostscript   # PostScript/PDF interpreter

    # Media/graphics
    chromaprint   # Audio fingerprinting (provides fpcalc for beets chroma plugin)
    ffmpeg        # Complete, cross-platform solution to rec
    optipng       # PNG optimizer
    viu           # Terminal image viewer
    imagemagick   # Image manipulation

    # Editors
    neovim        # Main editor (config in editors.nix)

    # Code tools
    ast-grep      # AST-based code search
    elan          # Small tool to manage your installations
    helix         # Modal editor (alternative)
    hyperfine     # Command-line benchmarking tool
    mermaid-cli   # Generation of diagrams from text in a si
    ruff          # Extremely fast Python linter and code formatter

    # AI Tools
    claude-chill      # PTY proxy for smoother Claude Code output
    codex-acp
    claude-code-acp   

    # System monitoring
    gdu           # Disk usage analyzer with console interface
    htop          # Interactive process viewer
    ncdu          # Disk usage analyzer with an ncurses interface
    smartmontools # Tools for monitoring the health of hard drives
    watch         # Execute a program periodically, showing output fullscreen

    # Misc
    chafa         # Image to terminal
    qrencode      # QR code generator
    terminal-notifier
    # === Migrated from Homebrew ===
    gcalcli           # Google Calendar CLI
    google-cloud-sdk  # Tools for the google cloud platform
    llm               # LLM CLI tool
    mpd               # Music Player Daemon
    obsidian-cli      # Obsidian CLI (open -a wrapper)
    pi-coding-agent   # Pi AI coding agent (npx wrapper)
    repomix           # Repo packer for AI
    rmpc              # MPD client (TUI)
    ueberzugpp        # Terminal image display
    _1password-cli    # 1Password CLI
    # AI tools (claude-code, codex) via Homebrew cask for faster updates

    # === External flake packages ===
    inputs.beads-viewer.packages.aarch64-darwin.default     # Beads TUI (bv)
    inputs.nx-rs.packages.aarch64-darwin.default             # nx (Rust)
    inputs.storage-planner.packages.aarch64-darwin.default   # sp (Rust)
    inputs.torrent-getter.packages.aarch64-darwin.default    # torrent-getter CLI
  ];
}
