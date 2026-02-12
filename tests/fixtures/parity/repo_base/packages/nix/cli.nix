# nx: CLI tools and utilities
{ pkgs, ... }:
{
  home.packages = with pkgs; [
    ripgrep
    fd
    lua5_4
  ];
}
