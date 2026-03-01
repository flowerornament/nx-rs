# nx: services and daemons
{ ... }:
{
  launchd.agents.test-agent = {
    command = "/usr/bin/true";
  };
}
