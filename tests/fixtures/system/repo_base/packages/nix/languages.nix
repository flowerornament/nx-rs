# nx: language runtimes and toolchains
{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    python3
  ];
}
