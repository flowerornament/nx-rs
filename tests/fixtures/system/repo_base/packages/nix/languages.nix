# nx: language runtimes and toolchains
{ pkgs, ... }:
{
  environment.systemPackages = with pkgs; [
    python3
    (pkgs.python3.withPackages (ps: with ps; [ requests ]))
  ];
}
